use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Decision {
    Incoming,
    Current,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Conflict {
    pub start_line: usize,
    pub end_line: usize,
    pub current_label: String,
    pub incoming_label: String,
    pub current: Vec<String>,
    pub incoming: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DecisionCounts {
    pub incoming: usize,
    pub current: usize,
    pub reviewed: usize,
    pub pending: usize,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Marker {
    Start,
    Base,
    Separator,
    End,
}

fn marker(line: &str) -> Option<(Marker, usize, &str)> {
    let line = line
        .strip_suffix("\r\n")
        .or_else(|| line.strip_suffix('\n'))
        .unwrap_or(line);
    let first = *line.as_bytes().first()?;
    let kind = match first {
        b'<' => Marker::Start,
        b'|' => Marker::Base,
        b'=' => Marker::Separator,
        b'>' => Marker::End,
        _ => return None,
    };
    let width = line.bytes().take_while(|&byte| byte == first).count();
    let suffix = &line[width..];
    if !suffix.is_empty() && (kind == Marker::Separator || !suffix.starts_with([' ', '\t'])) {
        return None;
    }
    Some((kind, width, suffix.trim()))
}

/// Git attributes disambiguate short markers and literal marker examples in the document.
pub fn parse_conflicts(
    lines: &[String],
    marker_size: Option<usize>,
) -> Result<Vec<Conflict>, String> {
    let mut conflicts = Vec::new();
    let mut rows = lines.iter().enumerate();
    while let Some((start, line)) = rows.next() {
        let Some((Marker::Start, width, label)) = marker(line) else {
            continue;
        };
        if !marker_size.map_or(width >= 7, |size| size == width) {
            continue;
        }

        let mut section = Marker::Start;
        let mut current = Vec::new();
        let mut incoming = Vec::new();
        let (end, incoming_label) = loop {
            let (index, line) = rows
                .next()
                .ok_or_else(|| format!("unterminated conflict starting at line {}", start + 1))?;
            match marker(line).filter(|(_, size, _)| *size == width) {
                Some((Marker::Base, _, _)) if section == Marker::Start => section = Marker::Base,
                Some((Marker::Separator, _, _)) if section != Marker::Separator => {
                    section = Marker::Separator
                }
                Some((Marker::End, _, label)) if section == Marker::Separator => {
                    break (index, label)
                }
                Some(_) => return Err(format!("unexpected conflict marker at line {}", index + 1)),
                None if section == Marker::Start => current.push(line.clone()),
                None if section == Marker::Separator => incoming.push(line.clone()),
                None => {}
            }
        };
        conflicts.push(Conflict {
            start_line: start + 1,
            end_line: end + 1,
            current_label: label.to_owned(),
            incoming_label: incoming_label.to_owned(),
            current,
            incoming,
        });
    }
    Ok(conflicts)
}

pub fn apply_decisions(
    lines: &[String],
    conflicts: &[Conflict],
    decisions: &HashMap<usize, Decision>,
) -> String {
    let mut output = String::new();
    let mut cursor = 0;
    for (index, conflict) in conflicts.iter().enumerate() {
        for line in &lines[cursor..conflict.start_line - 1] {
            output.push_str(line);
        }
        let selected: &[String] = match decisions.get(&index) {
            Some(Decision::Incoming) => &conflict.incoming,
            Some(Decision::Current) => &conflict.current,
            None => &lines[conflict.start_line - 1..conflict.end_line],
        };
        for line in selected {
            output.push_str(line);
        }
        cursor = conflict.end_line;
    }
    for line in &lines[cursor..] {
        output.push_str(line);
    }
    output
}

pub fn decision_counts(total: usize, decisions: &HashMap<usize, Decision>) -> DecisionCounts {
    let incoming = decisions
        .values()
        .filter(|decision| **decision == Decision::Incoming)
        .count();
    let current = decisions
        .values()
        .filter(|decision| **decision == Decision::Current)
        .count();
    DecisionCounts {
        incoming,
        current,
        reviewed: incoming + current,
        pending: total.saturating_sub(incoming + current),
    }
}

pub fn split_lines(source: &str) -> Vec<String> {
    source.split_inclusive('\n').map(str::to_owned).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::diff_as_conflicts;

    #[test]
    fn parses_conflicts_with_current_and_incoming_blocks() {
        let lines = split_lines("a\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> branch\nz\n");
        let conflicts = parse_conflicts(&lines, None).unwrap();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].start_line, 2);
        assert_eq!(conflicts[0].end_line, 6);
        assert_eq!(conflicts[0].current, vec!["ours\n"]);
        assert_eq!(conflicts[0].incoming, vec!["theirs\n"]);
    }

    #[test]
    fn applies_reviewed_decisions_and_preserves_unreviewed_conflicts() {
        let lines = split_lines(
            "a\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> branch\nb\n<<<<<<< HEAD\nold\n=======\nnew\n>>>>>>> branch\nz\n",
        );
        let conflicts = parse_conflicts(&lines, None).unwrap();
        let mut decisions = HashMap::new();
        decisions.insert(0, Decision::Incoming);
        let output = apply_decisions(&lines, &conflicts, &decisions);
        assert!(output.contains("theirs"));
        assert!(!output.contains("ours"));
        assert!(output.contains("<<<<<<< HEAD\nold\n=======\nnew\n>>>>>>> branch"));
    }

    #[test]
    fn preserves_mixed_line_endings_when_applying_decisions() {
        let lines =
            split_lines("a\n<<<<<<< HEAD\r\nours\r\n=======\r\ntheirs\r\n>>>>>>> branch\r\nz\r\n");
        let conflicts = parse_conflicts(&lines, None).unwrap();
        let mut decisions = HashMap::new();
        decisions.insert(0, Decision::Current);
        let output = apply_decisions(&lines, &conflicts, &decisions);
        assert_eq!(output, "a\nours\r\nz\r\n");
    }

    #[test]
    fn builds_diff_conflicts_for_file_pairs() {
        let current = split_lines("a\nold\nkeep\nremove\nz\n");
        let incoming = split_lines("a\nnew\nkeep\nz\nadded\n");
        let conflicts = diff_as_conflicts(
            &current,
            &incoming,
            "configA.yaml".to_string(),
            "configB.yaml".to_string(),
        );

        assert_eq!(conflicts.len(), 3);
        assert_eq!(conflicts[0].current, vec!["old\n"]);
        assert_eq!(conflicts[0].incoming, vec!["new\n"]);
        assert_eq!(conflicts[1].current, vec!["remove\n"]);
        assert!(conflicts[1].incoming.is_empty());
        assert!(conflicts[2].current.is_empty());
        assert_eq!(conflicts[2].incoming, vec!["added\n"]);
    }

    #[test]
    fn unreviewed_diff_conflicts_keep_current_file_content() {
        let current = split_lines("a\nz\n");
        let incoming = split_lines("a\ninserted\nz\n");
        let conflicts = diff_as_conflicts(
            &current,
            &incoming,
            "ours".to_string(),
            "theirs".to_string(),
        );

        let output = apply_decisions(&current, &conflicts, &HashMap::new());

        assert_eq!(output, "a\nz\n");
    }

    #[test]
    fn parses_checked_in_fixtures() {
        let fixtures = [
            ("fixtures/single/simple.txt", 1),
            ("fixtures/single/multiple.txt", 3),
            ("fixtures/single/diff3.txt", 1),
            ("fixtures/mergetool/merged.txt", 2),
        ];

        for (path, expected_conflicts) in fixtures {
            let fixture_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path);
            let source = std::fs::read_to_string(&fixture_path).unwrap();
            let lines = split_lines(&source);
            let conflicts = parse_conflicts(&lines, None).unwrap();

            assert_eq!(
                conflicts.len(),
                expected_conflicts,
                "{} should have {expected_conflicts} parseable conflict marker block(s)",
                fixture_path.display()
            );
        }
    }

    #[test]
    fn delimiters_match_the_opening_width_and_shape() {
        let lines = split_lines("<<<<<<<<< ours\nkeep\n=======\n========= heading\n||||||||| base\nancestor\n=========\ntheirs\n>>>>>>> literal\n>>>>>>>>> branch\n");
        let conflicts = parse_conflicts(&lines, Some(9)).unwrap();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(
            conflicts[0].current.concat(),
            "keep\n=======\n========= heading\n"
        );
        assert_eq!(conflicts[0].incoming.concat(), "theirs\n>>>>>>> literal\n");
        assert_eq!(conflicts[0].incoming_label, "branch");
        assert!(
            parse_conflicts(&split_lines("<<<<<<< ours\n=======\n>>>>> branch\n"), None).is_err()
        );
    }

    #[test]
    fn file_pair_choices_preserve_exact_bytes() {
        for (ours, theirs) in [
            ("abc", "abc\n"),
            ("", "\n"),
            ("old\n", ""),
            ("", "new"),
            ("a\n\n", "a\r\n"),
            ("before\nmiddle\nafter", "changed\nmiddle\nafter\n"),
        ] {
            let lines = split_lines(ours);
            let conflicts =
                diff_as_conflicts(&lines, &split_lines(theirs), "ours".into(), "theirs".into());
            assert!(!conflicts.is_empty());
            assert_eq!(apply_decisions(&lines, &conflicts, &HashMap::new()), ours);
            let incoming = (0..conflicts.len())
                .map(|i| (i, Decision::Incoming))
                .collect();
            assert_eq!(apply_decisions(&lines, &conflicts, &incoming), theirs);
        }
    }

    #[test]
    fn builds_diff_conflicts_for_checked_in_pair_fixture() {
        let current_path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/pair/configA.yaml");
        let incoming_path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/pair/configB.yaml");
        let current = std::fs::read_to_string(current_path).unwrap();
        let incoming = std::fs::read_to_string(incoming_path).unwrap();
        let current_lines = split_lines(&current);
        let incoming_lines = split_lines(&incoming);
        let conflicts = diff_as_conflicts(
            &current_lines,
            &incoming_lines,
            "configA.yaml".to_string(),
            "configB.yaml".to_string(),
        );

        assert_eq!(conflicts.len(), 6);
    }
}
