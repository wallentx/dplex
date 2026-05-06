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
    pub original: Vec<String>,
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

pub fn parse_conflicts(lines: &[String]) -> Result<Vec<Conflict>, String> {
    let mut conflicts = Vec::new();
    let mut index = 0;

    while index < lines.len() {
        let line = &lines[index];
        if !line.starts_with("<<<<<<<") {
            index += 1;
            continue;
        }

        let start = index;
        let current_label = line.trim_start_matches("<<<<<<<").trim().to_string();
        index += 1;

        let mut current = Vec::new();
        while index < lines.len()
            && !lines[index].starts_with("=======")
            && !lines[index].starts_with("|||||||")
        {
            current.push(lines[index].clone());
            index += 1;
        }

        if index < lines.len() && lines[index].starts_with("|||||||") {
            index += 1;
            while index < lines.len() && !lines[index].starts_with("=======") {
                index += 1;
            }
        }

        if index >= lines.len() || !lines[index].starts_with("=======") {
            return Err(format!(
                "unterminated conflict starting at line {}",
                start + 1
            ));
        }
        index += 1;

        let mut incoming = Vec::new();
        while index < lines.len() && !lines[index].starts_with(">>>>>>>") {
            incoming.push(lines[index].clone());
            index += 1;
        }

        if index >= lines.len() {
            return Err(format!(
                "unterminated conflict starting at line {}",
                start + 1
            ));
        }

        let incoming_label = lines[index]
            .trim_start_matches(">>>>>>>")
            .trim()
            .to_string();
        let end = index;
        let original = lines[start..=end].to_vec();
        conflicts.push(Conflict {
            start_line: start + 1,
            end_line: end + 1,
            current_label,
            incoming_label,
            original,
            current,
            incoming,
        });
        index += 1;
    }

    Ok(conflicts)
}

pub fn diff_as_conflicts(
    current_lines: &[String],
    incoming_lines: &[String],
    current_label: String,
    incoming_label: String,
) -> Vec<Conflict> {
    let ops = diff_ops(current_lines, incoming_lines);
    let mut conflicts = Vec::new();
    let mut index = 0;
    let mut current_cursor = 0;

    while index < ops.len() {
        match &ops[index] {
            DiffOp::Equal => {
                current_cursor += 1;
                index += 1;
            }
            DiffOp::Delete(_) | DiffOp::Insert(_) => {
                let start_cursor = current_cursor;
                let mut current = Vec::new();
                let mut incoming = Vec::new();

                while let Some(op) = ops.get(index) {
                    match op {
                        DiffOp::Equal => break,
                        DiffOp::Delete(line) => {
                            current.push(line.clone());
                            current_cursor += 1;
                        }
                        DiffOp::Insert(line) => incoming.push(line.clone()),
                    }
                    index += 1;
                }

                conflicts.push(Conflict {
                    start_line: start_cursor + 1,
                    end_line: start_cursor + current.len(),
                    current_label: current_label.clone(),
                    incoming_label: incoming_label.clone(),
                    original: current.clone(),
                    current,
                    incoming,
                });
            }
        }
    }

    conflicts
}

pub fn apply_decisions(
    lines: &[String],
    conflicts: &[Conflict],
    decisions: &HashMap<usize, Decision>,
    line_ending: &str,
) -> String {
    let mut output = Vec::new();
    let mut cursor = 0;

    for (index, conflict) in conflicts.iter().enumerate() {
        let start = conflict.start_line - 1;
        let end_exclusive = conflict.end_line;
        output.extend_from_slice(&lines[cursor..start]);
        match decisions.get(&index) {
            Some(Decision::Incoming) => output.extend(conflict.incoming.iter().cloned()),
            Some(Decision::Current) => output.extend(conflict.current.iter().cloned()),
            None => output.extend(conflict.original.iter().cloned()),
        }
        cursor = end_exclusive;
    }

    output.extend_from_slice(&lines[cursor..]);
    output.join(line_ending)
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum DiffOp {
    Equal,
    Delete(String),
    Insert(String),
}

fn diff_ops(current_lines: &[String], incoming_lines: &[String]) -> Vec<DiffOp> {
    let rows = current_lines.len() + 1;
    let columns = incoming_lines.len() + 1;
    let mut lcs = vec![0_usize; rows * columns];

    for current_index in (0..current_lines.len()).rev() {
        for incoming_index in (0..incoming_lines.len()).rev() {
            let cell = current_index * columns + incoming_index;
            if current_lines[current_index] == incoming_lines[incoming_index] {
                lcs[cell] = 1 + lcs[(current_index + 1) * columns + incoming_index + 1];
            } else {
                lcs[cell] = lcs[(current_index + 1) * columns + incoming_index]
                    .max(lcs[current_index * columns + incoming_index + 1]);
            }
        }
    }

    let mut ops = Vec::new();
    let mut current_index = 0;
    let mut incoming_index = 0;

    while current_index < current_lines.len() && incoming_index < incoming_lines.len() {
        if current_lines[current_index] == incoming_lines[incoming_index] {
            ops.push(DiffOp::Equal);
            current_index += 1;
            incoming_index += 1;
        } else if lcs[(current_index + 1) * columns + incoming_index]
            >= lcs[current_index * columns + incoming_index + 1]
        {
            ops.push(DiffOp::Delete(current_lines[current_index].clone()));
            current_index += 1;
        } else {
            ops.push(DiffOp::Insert(incoming_lines[incoming_index].clone()));
            incoming_index += 1;
        }
    }

    while current_index < current_lines.len() {
        ops.push(DiffOp::Delete(current_lines[current_index].clone()));
        current_index += 1;
    }
    while incoming_index < incoming_lines.len() {
        ops.push(DiffOp::Insert(incoming_lines[incoming_index].clone()));
        incoming_index += 1;
    }

    ops
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

pub fn split_lines_preserving_empty_tail(source: &str) -> Vec<String> {
    let mut lines: Vec<String> = source
        .split('\n')
        .map(|line| line.trim_end_matches('\r').to_string())
        .collect();
    if source.ends_with('\n') {
        lines.pop();
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_conflicts_with_current_and_incoming_blocks() {
        let lines = split_lines_preserving_empty_tail(
            "a\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> branch\nz\n",
        );
        let conflicts = parse_conflicts(&lines).unwrap();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].start_line, 2);
        assert_eq!(conflicts[0].end_line, 6);
        assert_eq!(conflicts[0].current, vec!["ours"]);
        assert_eq!(conflicts[0].incoming, vec!["theirs"]);
    }

    #[test]
    fn applies_reviewed_decisions_and_preserves_unreviewed_conflicts() {
        let lines = split_lines_preserving_empty_tail(
            "a\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> branch\nb\n<<<<<<< HEAD\nold\n=======\nnew\n>>>>>>> branch\nz\n",
        );
        let conflicts = parse_conflicts(&lines).unwrap();
        let mut decisions = HashMap::new();
        decisions.insert(0, Decision::Incoming);
        let output = apply_decisions(&lines, &conflicts, &decisions, "\n");
        assert!(output.contains("theirs"));
        assert!(!output.contains("ours"));
        assert!(output.contains("<<<<<<< HEAD\nold\n=======\nnew\n>>>>>>> branch"));
    }

    #[test]
    fn preserves_requested_line_ending_when_applying_decisions() {
        let lines = split_lines_preserving_empty_tail(
            "a\r\n<<<<<<< HEAD\r\nours\r\n=======\r\ntheirs\r\n>>>>>>> branch\r\nz\r\n",
        );
        let conflicts = parse_conflicts(&lines).unwrap();
        let mut decisions = HashMap::new();
        decisions.insert(0, Decision::Current);
        let output = apply_decisions(&lines, &conflicts, &decisions, "\r\n");
        assert_eq!(output, "a\r\nours\r\nz");
    }

    #[test]
    fn builds_diff_conflicts_for_file_pairs() {
        let current = split_lines_preserving_empty_tail("a\nold\nkeep\nremove\nz\n");
        let incoming = split_lines_preserving_empty_tail("a\nnew\nkeep\nz\nadded\n");
        let conflicts = diff_as_conflicts(
            &current,
            &incoming,
            "configA.yaml".to_string(),
            "configB.yaml".to_string(),
        );

        assert_eq!(conflicts.len(), 3);
        assert_eq!(conflicts[0].current, vec!["old"]);
        assert_eq!(conflicts[0].incoming, vec!["new"]);
        assert_eq!(conflicts[1].current, vec!["remove"]);
        assert!(conflicts[1].incoming.is_empty());
        assert!(conflicts[2].current.is_empty());
        assert_eq!(conflicts[2].incoming, vec!["added"]);
    }

    #[test]
    fn unreviewed_diff_conflicts_keep_current_file_content() {
        let current = split_lines_preserving_empty_tail("a\nz\n");
        let incoming = split_lines_preserving_empty_tail("a\ninserted\nz\n");
        let conflicts = diff_as_conflicts(
            &current,
            &incoming,
            "ours".to_string(),
            "theirs".to_string(),
        );

        let output = apply_decisions(&current, &conflicts, &HashMap::new(), "\n");

        assert_eq!(output, "a\nz");
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
            let lines = split_lines_preserving_empty_tail(&source);
            let conflicts = parse_conflicts(&lines).unwrap();

            assert_eq!(
                conflicts.len(),
                expected_conflicts,
                "{} should have {expected_conflicts} parseable conflict marker block(s)",
                fixture_path.display()
            );
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
        let current_lines = split_lines_preserving_empty_tail(&current);
        let incoming_lines = split_lines_preserving_empty_tail(&incoming);
        let conflicts = diff_as_conflicts(
            &current_lines,
            &incoming_lines,
            "configA.yaml".to_string(),
            "configB.yaml".to_string(),
        );

        assert_eq!(conflicts.len(), 6);
    }
}
