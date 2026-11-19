use similar::algorithms::{myers, Capture, Replace};
use similar::DiffTag;

use crate::parser::Conflict;

pub(crate) fn diff_as_conflicts(
    current_lines: &[String],
    incoming_lines: &[String],
    current_label: String,
    incoming_label: String,
) -> Vec<Conflict> {
    // Myers sizes its scratch buffers from the input ranges. Exclude shared
    // edges first so a small change in a large file needs little scratch space.
    let prefix = current_lines
        .iter()
        .zip(incoming_lines)
        .take_while(|(a, b)| a == b)
        .count();
    let suffix = current_lines[prefix..]
        .iter()
        .rev()
        .zip(incoming_lines[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();

    let mut changes = Replace::new(Capture::new());
    myers::diff(
        &mut changes,
        current_lines,
        prefix..current_lines.len() - suffix,
        incoming_lines,
        prefix..incoming_lines.len() - suffix,
    )
    .expect("capturing diff operations is infallible");
    let mut conflicts = Vec::new();
    for change in changes.into_inner().into_ops() {
        let (tag, ours, theirs) = change.as_tag_tuple();
        if tag == DiffTag::Equal {
            continue;
        }
        let current = current_lines[ours.clone()].to_vec();
        conflicts.push(Conflict {
            start_line: ours.start + 1,
            end_line: ours.end,
            current_label: current_label.clone(),
            incoming_label: incoming_label.clone(),
            current,
            incoming: incoming_lines[theirs].to_vec(),
        });
    }
    conflicts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applying_edits_around_repeated_lines_reproduces_incoming() {
        let mut current = vec![String::from("one\n"), String::from("zero\n")];
        let incoming = vec!["zero\n".into(), "zero\n".into(), "one\n".into()];
        let changes = diff_as_conflicts(&current, &incoming, "ours".into(), "theirs".into());
        for change in changes.into_iter().rev() {
            current.splice(change.start_line - 1..change.end_line, change.incoming);
        }
        assert_eq!(current, incoming);
    }

    #[test]
    fn sparse_edits_in_a_large_file_keep_their_source_coordinates() {
        let current: Vec<_> = (0..20_000).map(|i| format!("line {i}\n")).collect();
        let mut incoming = current.clone();
        incoming[10] = "early change\n".into();
        incoming[19_990] = "late change\n".into();
        let conflicts = diff_as_conflicts(&current, &incoming, "ours".into(), "theirs".into());

        assert_eq!(conflicts.len(), 2);
        assert_eq!((conflicts[0].start_line, conflicts[0].end_line), (11, 11));
        assert_eq!(conflicts[0].incoming, ["early change\n"]);
        assert_eq!(
            (conflicts[1].start_line, conflicts[1].end_line),
            (19_991, 19_991)
        );
        assert_eq!(conflicts[1].incoming, ["late change\n"]);
    }
}
