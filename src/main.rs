mod parser;
mod ui;

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitCode;

use parser::{
    apply_decisions, decision_counts, diff_as_conflicts, parse_conflicts,
    split_lines_preserving_empty_tail, Conflict, Decision,
};

fn main() -> ExitCode {
    match run(env::args().skip(1).collect()) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("dplex: {error}");
            ExitCode::from(1)
        }
    }
}

fn run(args: Vec<String>) -> Result<ExitCode, String> {
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        print_help();
        return Ok(ExitCode::SUCCESS);
    }

    let Some(input) = input_mode_from_args(&args) else {
        print_help();
        return Ok(ExitCode::from(2));
    };

    let document = load_review_document(input)?;

    if document.conflicts.is_empty() {
        println!("{}", document.empty_message);
        return Ok(ExitCode::SUCCESS);
    }

    let save = |decisions: &HashMap<usize, Decision>, path: &Path| {
        save_review(
            path,
            &document.source_lines,
            &document.conflicts,
            decisions,
            &document.line_ending,
            document.has_trailing_newline,
        )
    };
    let review = ui::review_conflicts(
        &document.display_path,
        &document.source_lines,
        &document.conflicts,
        document.default_save_path.clone(),
        save,
    )?;
    save_review(
        &review.save_path,
        &document.source_lines,
        &document.conflicts,
        &review.decisions,
        &document.line_ending,
        document.has_trailing_newline,
    )?;

    let counts = decision_counts(document.conflicts.len(), &review.decisions);
    println!(
        "Resolved {} conflict{} in {}. Chose ours for {}, theirs for {}.",
        counts.reviewed,
        plural(counts.reviewed),
        review.save_path.display(),
        counts.current,
        counts.incoming
    );

    if review.quit && counts.pending > 0 {
        println!(
            "Stopped with {} conflict{} left unresolved.",
            counts.pending,
            plural(counts.pending)
        );
        return Ok(ExitCode::from(130));
    }

    Ok(ExitCode::SUCCESS)
}

fn print_help() {
    println!(
        "dplex\n\nUsage:\n  dplex <conflicted-file>\n  dplex <ours-file> <theirs-file>\n  dplex <base> <local> <remote> <merged>\n\nKeys:\n  o         choose ours\n  t         choose theirs\n  Ctrl+s    save\n  S         save as\n  q/Esc/C-c quit\n  ←/→       previous/next hunk\n  ↑/↓       scroll\n  PgUp/PgDn page"
    );
}

enum InputMode<'a> {
    ConflictFile { path: &'a str },
    FilePair { current: &'a str, incoming: &'a str },
    GitMergetool { merged: &'a str },
}

struct ReviewDocument {
    display_path: PathBuf,
    default_save_path: PathBuf,
    source_lines: Vec<String>,
    conflicts: Vec<Conflict>,
    line_ending: String,
    has_trailing_newline: bool,
    empty_message: String,
}

fn input_mode_from_args(args: &[String]) -> Option<InputMode<'_>> {
    match args {
        [file] => Some(InputMode::ConflictFile {
            path: file.as_str(),
        }),
        [current, incoming] => Some(InputMode::FilePair {
            current: current.as_str(),
            incoming: incoming.as_str(),
        }),
        [_base, _local, _remote, merged] => Some(InputMode::GitMergetool {
            merged: merged.as_str(),
        }),
        _ => None,
    }
}

fn load_review_document(input: InputMode<'_>) -> Result<ReviewDocument, String> {
    match input {
        InputMode::ConflictFile { path } => load_conflicted_file(path),
        InputMode::GitMergetool { merged } => load_conflicted_file(merged),
        InputMode::FilePair { current, incoming } => load_file_pair(current, incoming),
    }
}

fn load_conflicted_file(path: &str) -> Result<ReviewDocument, String> {
    let path = PathBuf::from(path);
    let source = fs::read_to_string(&path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let source_lines = split_lines_preserving_empty_tail(&source);
    let conflicts = parse_conflicts(&source_lines)?;

    Ok(ReviewDocument {
        display_path: path.clone(),
        default_save_path: path.clone(),
        source_lines,
        conflicts,
        line_ending: line_ending_for(&source).to_string(),
        has_trailing_newline: source.ends_with('\n'),
        empty_message: format!("dplex: no conflict markers found in {}", path.display()),
    })
}

fn load_file_pair(current: &str, incoming: &str) -> Result<ReviewDocument, String> {
    let current_path = PathBuf::from(current);
    let incoming_path = PathBuf::from(incoming);
    let current_source = fs::read_to_string(&current_path)
        .map_err(|error| format!("failed to read {}: {error}", current_path.display()))?;
    let incoming_source = fs::read_to_string(&incoming_path)
        .map_err(|error| format!("failed to read {}: {error}", incoming_path.display()))?;
    let current_lines = split_lines_preserving_empty_tail(&current_source);
    let incoming_lines = split_lines_preserving_empty_tail(&incoming_source);
    let conflicts = diff_as_conflicts(
        &current_lines,
        &incoming_lines,
        current_path.display().to_string(),
        incoming_path.display().to_string(),
    );

    Ok(ReviewDocument {
        display_path: current_path.clone(),
        default_save_path: current_path.clone(),
        source_lines: current_lines,
        conflicts,
        line_ending: line_ending_for(&current_source).to_string(),
        has_trailing_newline: current_source.ends_with('\n'),
        empty_message: format!(
            "dplex: no differences found between {} and {}",
            current_path.display(),
            incoming_path.display()
        ),
    })
}

fn save_review(
    path: &Path,
    source_lines: &[String],
    conflicts: &[Conflict],
    decisions: &HashMap<usize, Decision>,
    line_ending: &str,
    has_trailing_newline: bool,
) -> Result<(), String> {
    let mut resolved = apply_decisions(source_lines, conflicts, decisions, line_ending);
    if has_trailing_newline {
        resolved.push_str(line_ending);
    }
    fs::write(path, resolved)
        .map_err(|error| format!("failed to write {}: {error}", path.display()))
}

fn line_ending_for(source: &str) -> &'static str {
    if source.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

fn plural(count: usize) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_single_file_or_git_mergetool_argument_shape() {
        let single = vec!["merged.txt".to_string()];
        let git = vec![
            "base.txt".to_string(),
            "local.txt".to_string(),
            "remote.txt".to_string(),
            "merged.txt".to_string(),
        ];
        let invalid = vec![
            "base.txt".to_string(),
            "local.txt".to_string(),
            "remote.txt".to_string(),
        ];

        assert!(matches!(
            input_mode_from_args(&single),
            Some(InputMode::ConflictFile { path: "merged.txt" })
        ));
        assert!(matches!(
            input_mode_from_args(&git),
            Some(InputMode::GitMergetool {
                merged: "merged.txt"
            })
        ));
        assert!(input_mode_from_args(&invalid).is_none());
    }

    #[test]
    fn accepts_two_file_argument_shape() {
        let pair = vec!["configA.yaml".to_string(), "configB.yaml".to_string()];

        assert!(matches!(
            input_mode_from_args(&pair),
            Some(InputMode::FilePair {
                current: "configA.yaml",
                incoming: "configB.yaml"
            })
        ));
    }
}
