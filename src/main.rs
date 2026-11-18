mod diff;
mod parser;
mod ui;

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

use diff::diff_as_conflicts;

use parser::{apply_decisions, decision_counts, parse_conflicts, split_lines, Conflict, Decision};

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
    if matches!(args.as_slice(), [flag] if flag == "-h" || flag == "--help") {
        print_help();
        return Ok(ExitCode::SUCCESS);
    }

    let Some(input) = input_mode_from_args(&args) else {
        print_help();
        return Ok(ExitCode::from(2));
    };

    let mergetool = matches!(input, InputMode::GitMergetool { .. });
    let document = load_review_document(input)?;

    if document.conflicts.is_empty() {
        if mergetool {
            return Err(format!(
                "Git merge resolution was not confirmed for {} (no conflict markers found)",
                document.display_path.display()
            ));
        }
        println!("{}", document.empty_message);
        return Ok(ExitCode::SUCCESS);
    }

    let save = |decisions: &HashMap<usize, Decision>, path: &Path| {
        let resolved = apply_decisions(&document.source_lines, &document.conflicts, decisions);
        fs::write(path, resolved)
            .map_err(|error| format!("failed to write {}: {error}", path.display()))
    };
    let review = ui::review_conflicts(
        &document.display_path,
        &document.source_lines,
        &document.conflicts,
        document.default_save_path.clone(),
        save,
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

    if mergetool
        && review.save_path != document.default_save_path
        && !matches!((review.save_path.canonicalize(), document.default_save_path.canonicalize()),
            (Ok(saved), Ok(merged)) if saved == merged)
    {
        return Err(format!(
            "Git needs the resolved file at {}",
            document.default_save_path.display()
        ));
    }
    Ok(ExitCode::SUCCESS)
}

fn print_help() {
    println!(
        "dplex\n\nUsage:\n  dplex <conflicted-file>\n  dplex <ours-file> <theirs-file>\n  dplex <base> <local> <remote> <merged>\n\nKeys:\n  o         choose ours\n  t         choose theirs\n  Ctrl+s    save\n  S         save as\n  u         undo last choice\n  Enter     save and finish after reviewing all hunks\n  q/Esc/C-c quit (prompts for unsaved changes)\n  ←/→       previous/next hunk\n  h/l       scroll left/right\n  ↑/↓       scroll\n  PgUp/PgDn page"
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
        InputMode::ConflictFile { path } => load_conflicted_file(path, false),
        InputMode::GitMergetool { merged } => load_conflicted_file(merged, true),
        InputMode::FilePair { current, incoming } => load_file_pair(current, incoming),
    }
}

fn load_conflicted_file(path: &str, mergetool: bool) -> Result<ReviewDocument, String> {
    let path = PathBuf::from(path);
    let source = fs::read_to_string(&path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let source_lines = split_lines(&source);
    let marker_size = git_marker_size(&path).or(mergetool.then_some(7));
    let conflicts = parse_conflicts(&source_lines, marker_size)?;

    Ok(ReviewDocument {
        display_path: path.clone(),
        default_save_path: path.clone(),
        source_lines,
        conflicts,
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
    let current_lines = split_lines(&current_source);
    let incoming_lines = split_lines(&incoming_source);
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
        empty_message: format!(
            "dplex: no differences found between {} and {}",
            current_path.display(),
            incoming_path.display()
        ),
    })
}

fn git_marker_size(path: &Path) -> Option<usize> {
    // Run from the file's directory so absolute and repository-relative inputs
    // resolve the same per-path attributes, including in linked worktrees.
    let parent = path
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let output = Command::new("git")
        .current_dir(parent)
        .args(["check-attr", "-z", "conflict-marker-size", "--"])
        .arg(path.file_name()?)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = output.stdout.split(|&byte| byte == 0).nth(2)?;
    std::str::from_utf8(value)
        .ok()?
        .parse::<usize>()
        .ok()
        .filter(|&size| size > 0)
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
