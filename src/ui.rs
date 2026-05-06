use std::collections::HashMap;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};

use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    self, disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::{Frame, Terminal};

use crate::parser::{decision_counts, Conflict, Decision};

type TuiTerminal = Terminal<CrosstermBackend<io::Stdout>>;

const HEADER_LINES: u16 = 4;
const SEPARATOR_LINES: u16 = 2;
const FOOTER_LINES: u16 = 2;
const FIXED_SCREEN_LINES: u16 = HEADER_LINES + SEPARATOR_LINES + FOOTER_LINES;

#[derive(Clone, Debug)]
pub struct ReviewOutcome {
    pub decisions: HashMap<usize, Decision>,
    pub quit: bool,
    pub save_path: PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Action {
    ChooseTheirs,
    ChooseOurs,
    Quit,
    Next,
    Previous,
    ScrollDown,
    ScrollUp,
    PageDown,
    PageUp,
    Save,
    SaveAs,
    None,
}

#[derive(Clone, Debug)]
struct ReviewState<'a> {
    path: &'a Path,
    source_lines: &'a [String],
    conflicts: &'a [Conflict],
    decisions: HashMap<usize, Decision>,
    saved_decisions: HashMap<usize, Decision>,
    current_index: usize,
    scroll_top: usize,
    save_path: PathBuf,
    status_message: Option<String>,
}

pub fn review_conflicts<F>(
    path: &Path,
    source_lines: &[String],
    conflicts: &[Conflict],
    initial_save_path: PathBuf,
    mut save: F,
) -> Result<ReviewOutcome, String>
where
    F: FnMut(&HashMap<usize, Decision>, &Path) -> Result<(), String>,
{
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err("interactive review requires a TTY".to_string());
    }

    let mut state = ReviewState {
        path,
        source_lines,
        conflicts,
        decisions: HashMap::new(),
        saved_decisions: HashMap::new(),
        current_index: 0,
        scroll_top: 0,
        save_path: initial_save_path,
        status_message: None,
    };
    focus_current_conflict(&mut state);
    let mut terminal = TerminalSession::enter()?;
    let mut quit = false;

    while state.decisions.len() < state.conflicts.len() {
        if state.decisions.contains_key(&state.current_index) {
            state.current_index = next_pending_index(&state).unwrap_or(state.current_index);
            focus_current_conflict(&mut state);
        }

        terminal.draw(&state)?;
        match read_action()? {
            Action::ChooseTheirs => {
                state
                    .decisions
                    .insert(state.current_index, Decision::Incoming);
                state.current_index = next_pending_index(&state).unwrap_or(state.current_index);
                focus_current_conflict(&mut state);
            }
            Action::ChooseOurs => {
                state
                    .decisions
                    .insert(state.current_index, Decision::Current);
                state.current_index = next_pending_index(&state).unwrap_or(state.current_index);
                focus_current_conflict(&mut state);
            }
            Action::Quit => {
                quit = true;
                break;
            }
            Action::Next => {
                state.current_index = next_pending_index(&state).unwrap_or(state.current_index);
                focus_current_conflict(&mut state);
            }
            Action::Previous => {
                state.current_index = previous_pending_index(&state).unwrap_or(state.current_index);
                focus_current_conflict(&mut state);
            }
            Action::ScrollDown => {
                state.scroll_top = state.scroll_top.saturating_add(1);
            }
            Action::ScrollUp => {
                state.scroll_top = state.scroll_top.saturating_sub(1);
            }
            Action::PageDown => {
                state.scroll_top = state.scroll_top.saturating_add(page_height());
            }
            Action::PageUp => {
                state.scroll_top = state.scroll_top.saturating_sub(page_height());
            }
            Action::Save => {
                save(&state.decisions, &state.save_path)?;
                state.saved_decisions = state.decisions.clone();
                state.status_message = Some(format!("saved to {}", state.save_path.display()));
            }
            Action::SaveAs => {
                if let Some(path) = prompt_for_save_path(&mut terminal, &state, "Save as: ")? {
                    save(&state.decisions, &path)?;
                    state.save_path = path;
                    state.saved_decisions = state.decisions.clone();
                    state.status_message = Some(format!("saved to {}", state.save_path.display()));
                } else {
                    state.status_message = Some("save as canceled".to_string());
                }
            }
            Action::None => {}
        }
    }

    Ok(ReviewOutcome {
        decisions: state.decisions,
        quit,
        save_path: state.save_path,
    })
}

fn render_screen(frame: &mut Frame<'_>, state: &ReviewState<'_>) {
    let area = render_area(frame.area());
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(HEADER_LINES),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(FOOTER_LINES),
        ])
        .split(area);

    let conflict = &state.conflicts[state.current_index];
    let counts = decision_counts(state.conflicts.len(), &state.decisions);
    let document = build_document_view(state);
    let body_height = chunks[2].height as usize;
    let max_scroll = document.len().saturating_sub(body_height);
    let scroll_top = state.scroll_top.min(max_scroll);
    let number_width = line_number_width(state.source_lines.len());
    let body_width = chunks[2].width as usize;

    let header_width = chunks[0].width as usize;
    let prompt = format!(
        "[{}/{}] choose ours or theirs",
        state.current_index + 1,
        state.conflicts.len()
    );
    let header = Paragraph::new(vec![
        Line::from(vec![
            Span::styled("dplex: ", Style::default().fg(warm_yellow())),
            Span::styled(
                format!(
                    "{} review hunk{}",
                    state.conflicts.len(),
                    plural(state.conflicts.len())
                ),
                Style::default()
                    .fg(Color::Rgb(232, 232, 232))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" in ", Style::default().fg(status_gray())),
            Span::styled(
                state.path.display().to_string(),
                Style::default().fg(diff_add_fg()),
            ),
        ]),
        Line::styled(
            pad_to_width(&prompt, header_width),
            Style::default()
                .fg(warm_yellow())
                .bg(Color::Rgb(24, 24, 26))
                .add_modifier(Modifier::BOLD),
        ),
        Line::from(vec![
            Span::styled(
                conflict.current_label.clone(),
                Style::default().fg(diff_add_fg()),
            ),
            Span::styled(" -> ", Style::default().fg(status_gray())),
            Span::styled(
                conflict.incoming_label.clone(),
                Style::default().fg(diff_add_fg()),
            ),
        ]),
        Line::from(vec![
            Span::styled(
                state.path.display().to_string(),
                Style::default().fg(diff_add_fg()),
            ),
            Span::styled(
                format!(":{}", conflict.start_line),
                Style::default().fg(Color::Rgb(150, 150, 155)),
            ),
            Span::raw("  "),
            Span::styled("theirs ", Style::default().fg(status_gray())),
            Span::styled(
                counts.incoming.to_string(),
                Style::default().fg(diff_add_fg()),
            ),
            Span::raw("  "),
            Span::styled("ours ", Style::default().fg(status_gray())),
            Span::styled(
                counts.current.to_string(),
                Style::default().fg(diff_del_fg()),
            ),
            Span::raw("  "),
            Span::styled("pending ", Style::default().fg(status_gray())),
            Span::styled(
                counts.pending.to_string(),
                Style::default().fg(warm_yellow()),
            ),
        ]),
    ]);
    frame.render_widget(header, chunks[0]);

    frame.render_widget(separator(area.width), chunks[1]);

    let lines: Vec<Line<'static>> = document
        .iter()
        .skip(scroll_top)
        .take(body_height)
        .map(|line| document_line(line, number_width, body_width, state.current_index))
        .collect();
    frame.render_widget(Paragraph::new(lines), chunks[2]);

    frame.render_widget(separator(area.width), chunks[3]);

    let visible_end = (scroll_top + body_height).min(document.len());
    let position = format!(
        "Lines {}-{} of {}",
        (scroll_top + 1).min(document.len().max(1)),
        visible_end,
        document.len()
    );
    let footer = match &state.status_message {
        Some(message) => format!("{position}  {message}"),
        None => position,
    };
    let controls = Paragraph::new(vec![
        Line::from(vec![
            key("o"),
            label(" ours  "),
            key("t"),
            label(" theirs  "),
            key("C-s"),
            label(" save  "),
            key("S"),
            label(" save as  "),
            key("q"),
            label(" quit  "),
            key("← →"),
            label(" nav  "),
            key("↑ ↓"),
            label(" scroll"),
        ]),
        Line::styled(footer, Style::default().fg(warm_yellow())),
    ]);
    frame.render_widget(controls, chunks[4]);
}

fn separator(width: u16) -> Paragraph<'static> {
    Paragraph::new(Line::styled(
        "─".repeat(width as usize),
        Style::default().fg(Color::Rgb(85, 85, 95)),
    ))
}

fn render_area(area: Rect) -> Rect {
    Rect {
        width: area.width.saturating_sub(1).max(1),
        ..area
    }
}

fn key(text: &'static str) -> Span<'static> {
    Span::styled(
        text,
        Style::default()
            .fg(key_accent())
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
    )
}

fn label(text: &'static str) -> Span<'static> {
    Span::styled(text, Style::default().fg(legend_text()))
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum DocumentLine {
    Context {
        line_number: usize,
        text: String,
    },
    Current {
        conflict_index: usize,
        line_number: usize,
        text: String,
    },
    Incoming {
        conflict_index: usize,
        text: String,
    },
    Resolved {
        conflict_index: usize,
        line_number: Option<usize>,
        text: String,
        dirty: bool,
    },
}

fn build_document_view(state: &ReviewState<'_>) -> Vec<DocumentLine> {
    let mut lines = Vec::new();
    let mut cursor = 0;

    for (conflict_index, conflict) in state.conflicts.iter().enumerate() {
        let start = conflict.start_line.saturating_sub(1);
        for line_number in cursor..start {
            if let Some(text) = state.source_lines.get(line_number) {
                lines.push(DocumentLine::Context {
                    line_number: line_number + 1,
                    text: text.clone(),
                });
            }
        }

        match state.decisions.get(&conflict_index) {
            Some(Decision::Current) => {
                let dirty = state.saved_decisions.get(&conflict_index) != Some(&Decision::Current);
                for (offset, text) in conflict.current.iter().enumerate() {
                    lines.push(DocumentLine::Resolved {
                        conflict_index,
                        line_number: Some(start + offset + 1),
                        text: text.clone(),
                        dirty,
                    });
                }
            }
            Some(Decision::Incoming) => {
                let dirty = state.saved_decisions.get(&conflict_index) != Some(&Decision::Incoming);
                for text in &conflict.incoming {
                    lines.push(DocumentLine::Resolved {
                        conflict_index,
                        line_number: None,
                        text: text.clone(),
                        dirty,
                    });
                }
            }
            None => {
                for (offset, text) in conflict.current.iter().enumerate() {
                    lines.push(DocumentLine::Current {
                        conflict_index,
                        line_number: start + offset + 1,
                        text: text.clone(),
                    });
                }

                for text in &conflict.incoming {
                    lines.push(DocumentLine::Incoming {
                        conflict_index,
                        text: text.clone(),
                    });
                }
            }
        }

        cursor = conflict.end_line;
    }

    for line_number in cursor..state.source_lines.len() {
        if let Some(text) = state.source_lines.get(line_number) {
            lines.push(DocumentLine::Context {
                line_number: line_number + 1,
                text: text.clone(),
            });
        }
    }

    if lines.is_empty() {
        lines.push(DocumentLine::Context {
            line_number: 1,
            text: "(empty file)".to_string(),
        });
    }
    lines
}

fn document_line(
    line: &DocumentLine,
    number_width: usize,
    width: usize,
    current_index: usize,
) -> Line<'static> {
    let selected = line.conflict_index() == Some(current_index);
    match line {
        DocumentLine::Context { line_number, text } => {
            context_line(*line_number, text, number_width, width)
        }
        DocumentLine::Current {
            line_number, text, ..
        } => diff_line(
            DiffKind::Current,
            selected,
            Some(*line_number),
            text,
            number_width,
            width,
        ),
        DocumentLine::Incoming { text, .. } => diff_line(
            DiffKind::Incoming,
            selected,
            None,
            text,
            number_width,
            width,
        ),
        DocumentLine::Resolved {
            line_number,
            text,
            dirty,
            ..
        } => resolved_line(*line_number, text, *dirty, number_width, width),
    }
}

fn context_line(
    line_number: usize,
    text: &str,
    number_width: usize,
    width: usize,
) -> Line<'static> {
    let prefix = format!("   {line_number:>number_width$} | ");
    let content_width = width.saturating_sub(prefix.chars().count());
    Line::from(vec![
        Span::styled(prefix, neutral_gutter_style()),
        Span::styled(
            truncate(text, content_width),
            Style::default().fg(Color::Rgb(226, 226, 226)),
        ),
    ])
}

fn diff_line(
    kind: DiffKind,
    selected: bool,
    line_number: Option<usize>,
    text: &str,
    number_width: usize,
    width: usize,
) -> Line<'static> {
    let marker = match (kind, selected) {
        (DiffKind::Current, _) => " -",
        (DiffKind::Incoming, _) => " +",
    };
    let number = line_number
        .map(|line_number| line_number.to_string())
        .unwrap_or_default();
    let gutter = format!(" {number:>number_width$} | ");
    let marker_width = marker.chars().count();
    let gutter_width = gutter.chars().count();
    let content_width = width.saturating_sub(marker_width + gutter_width);
    let content = if selected {
        pad_to_width(&truncate(text, content_width), content_width)
    } else {
        truncate(text, content_width)
    };

    Line::from(vec![
        Span::styled(marker.to_string(), diff_marker_style(kind, selected)),
        Span::styled(gutter, diff_gutter_style(selected, kind)),
        Span::styled(content, diff_content_style(kind, selected)),
    ])
}

fn resolved_line(
    line_number: Option<usize>,
    text: &str,
    dirty: bool,
    number_width: usize,
    width: usize,
) -> Line<'static> {
    let number = line_number
        .map(|line_number| line_number.to_string())
        .unwrap_or_default();
    let prefix = format!("   {number:>number_width$} | ");
    let content_width = width.saturating_sub(prefix.chars().count());
    let content_style = if dirty {
        Style::default()
            .fg(warm_yellow())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Rgb(226, 226, 226))
    };

    Line::from(vec![
        Span::styled(prefix, neutral_gutter_style()),
        Span::styled(truncate(text, content_width), content_style),
    ])
}

fn line_number_width(max_line: usize) -> usize {
    max_line.to_string().len().max(3)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DiffKind {
    Current,
    Incoming,
}

fn diff_marker_style(kind: DiffKind, selected: bool) -> Style {
    match (kind, selected) {
        (DiffKind::Current, true) => Style::default()
            .fg(diff_del_selected_fg())
            .bg(diff_del_bg())
            .add_modifier(Modifier::BOLD),
        (DiffKind::Current, false) => Style::default().fg(diff_del_fg()),
        (DiffKind::Incoming, true) => Style::default()
            .fg(diff_add_selected_fg())
            .bg(diff_add_bg())
            .add_modifier(Modifier::BOLD),
        (DiffKind::Incoming, false) => Style::default().fg(diff_add_fg()),
    }
}

fn diff_content_style(kind: DiffKind, selected: bool) -> Style {
    match (kind, selected) {
        (DiffKind::Current, true) => Style::default()
            .fg(diff_del_selected_fg())
            .bg(diff_del_bg())
            .add_modifier(Modifier::BOLD),
        (DiffKind::Current, false) => Style::default().fg(diff_del_fg()),
        (DiffKind::Incoming, true) => Style::default()
            .fg(diff_add_selected_fg())
            .bg(diff_add_bg())
            .add_modifier(Modifier::BOLD),
        (DiffKind::Incoming, false) => Style::default().fg(diff_add_fg()),
    }
}

fn neutral_gutter_style() -> Style {
    Style::default().fg(gutter_gray())
}

fn diff_gutter_style(selected: bool, kind: DiffKind) -> Style {
    let style = neutral_gutter_style();
    if selected {
        match kind {
            DiffKind::Current => style.bg(diff_del_bg()),
            DiffKind::Incoming => style.bg(diff_add_bg()),
        }
    } else {
        style
    }
}

fn warm_yellow() -> Color {
    Color::Rgb(232, 192, 78)
}

fn diff_add_fg() -> Color {
    Color::Rgb(0, 178, 160)
}

fn diff_add_selected_fg() -> Color {
    Color::Rgb(142, 211, 194)
}

fn diff_add_bg() -> Color {
    Color::Rgb(16, 54, 47)
}

fn diff_del_fg() -> Color {
    Color::Rgb(198, 78, 104)
}

fn diff_del_selected_fg() -> Color {
    Color::Rgb(218, 166, 177)
}

fn diff_del_bg() -> Color {
    Color::Rgb(61, 30, 35)
}

fn key_accent() -> Color {
    Color::Rgb(186, 154, 234)
}

fn legend_text() -> Color {
    Color::Rgb(139, 180, 232)
}

fn gutter_gray() -> Color {
    Color::Rgb(178, 178, 186)
}

fn status_gray() -> Color {
    Color::Rgb(138, 138, 150)
}

fn truncate(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let mut chars = text.chars();
    let truncated: String = chars.by_ref().take(width).collect();
    if chars.next().is_some() {
        let mut truncated: String = truncated.chars().take(width.saturating_sub(1)).collect();
        truncated.push('…');
        truncated
    } else {
        truncated
    }
}

fn pad_to_width(text: &str, width: usize) -> String {
    let length = text.chars().count();
    if length >= width {
        text.to_string()
    } else {
        format!("{text}{}", " ".repeat(width - length))
    }
}

fn focus_current_conflict(state: &mut ReviewState<'_>) {
    state.scroll_top = focused_scroll_top(state);
}

fn focused_scroll_top(state: &ReviewState<'_>) -> usize {
    build_document_view(state)
        .iter()
        .position(|line| line.conflict_index() == Some(state.current_index))
        .unwrap_or(0)
        .saturating_sub(5)
}

impl DocumentLine {
    fn conflict_index(&self) -> Option<usize> {
        match self {
            DocumentLine::Context { .. } => None,
            DocumentLine::Current { conflict_index, .. }
            | DocumentLine::Incoming { conflict_index, .. }
            | DocumentLine::Resolved { conflict_index, .. } => Some(*conflict_index),
        }
    }
}

fn next_pending_index(state: &ReviewState<'_>) -> Option<usize> {
    for offset in 1..=state.conflicts.len() {
        let index = (state.current_index + offset) % state.conflicts.len();
        if !state.decisions.contains_key(&index) {
            return Some(index);
        }
    }
    None
}

fn previous_pending_index(state: &ReviewState<'_>) -> Option<usize> {
    for offset in 1..=state.conflicts.len() {
        let index = (state.current_index + state.conflicts.len() - offset) % state.conflicts.len();
        if !state.decisions.contains_key(&index) {
            return Some(index);
        }
    }
    None
}

fn read_action() -> Result<Action, String> {
    loop {
        if let Event::Key(key) = event::read().map_err(|error| error.to_string())? {
            let action = action_from_key(key);
            if action != Action::None {
                return Ok(action);
            }
        }
    }
}

fn action_from_key(key: KeyEvent) -> Action {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return match key.code {
            KeyCode::Char('c') | KeyCode::Char('C') => Action::Quit,
            KeyCode::Char('s') => Action::Save,
            KeyCode::Char('S') => Action::SaveAs,
            _ => Action::None,
        };
    }

    match key.code {
        KeyCode::Char('t') | KeyCode::Char('T') | KeyCode::Char('y') | KeyCode::Char('Y') => {
            Action::ChooseTheirs
        }
        KeyCode::Char('o') | KeyCode::Char('O') | KeyCode::Char('n') | KeyCode::Char('N') => {
            Action::ChooseOurs
        }
        KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => Action::Quit,
        KeyCode::Right | KeyCode::Char(']') => Action::Next,
        KeyCode::Left | KeyCode::Char('[') => Action::Previous,
        KeyCode::Down => Action::ScrollDown,
        KeyCode::Up => Action::ScrollUp,
        KeyCode::PageDown => Action::PageDown,
        KeyCode::PageUp => Action::PageUp,
        KeyCode::Char('S') => Action::SaveAs,
        _ => Action::None,
    }
}

fn prompt_for_save_path(
    terminal: &mut TerminalSession,
    state: &ReviewState<'_>,
    prompt: &str,
) -> Result<Option<PathBuf>, String> {
    let mut input = String::new();

    loop {
        terminal.draw_with_prompt(state, prompt, &input)?;
        if let Event::Key(key) = event::read().map_err(|error| error.to_string())? {
            match key.code {
                KeyCode::Enter => {
                    let trimmed = input.trim();
                    if trimmed.is_empty() {
                        return Ok(None);
                    }
                    return Ok(Some(PathBuf::from(trimmed)));
                }
                KeyCode::Esc => return Ok(None),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(None)
                }
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    input.push(character);
                }
                _ => {}
            }
        }
    }
}

fn render_prompt(frame: &mut Frame<'_>, prompt: &str, input: &str) {
    let area = render_area(frame.area());
    let prompt_area = Rect {
        x: area.x,
        y: area.y + area.height.saturating_sub(1),
        width: area.width,
        height: 1,
    };
    frame.render_widget(Clear, prompt_area);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(prompt.to_string(), Style::default().fg(Color::Yellow)),
            Span::raw(input.to_string()),
        ])),
        prompt_area,
    );
}

struct TerminalSession {
    terminal: TuiTerminal,
}

impl TerminalSession {
    fn enter() -> Result<Self, String> {
        enable_raw_mode().map_err(|error| error.to_string())?;

        let mut stdout = io::stdout();
        if let Err(error) = execute!(stdout, EnterAlternateScreen, Hide) {
            let _ = disable_raw_mode();
            return Err(error.to_string());
        }

        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend).map_err(|error| error.to_string())?;
        terminal.clear().map_err(|error| error.to_string())?;
        Ok(Self { terminal })
    }

    fn draw(&mut self, state: &ReviewState<'_>) -> Result<(), String> {
        self.terminal
            .draw(|frame| render_screen(frame, state))
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn draw_with_prompt(
        &mut self,
        state: &ReviewState<'_>,
        prompt: &str,
        input: &str,
    ) -> Result<(), String> {
        self.terminal
            .draw(|frame| {
                render_screen(frame, state);
                render_prompt(frame, prompt, input);
            })
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let mut stdout = io::stdout();
        let _ = execute!(stdout, Show, LeaveAlternateScreen);
    }
}

fn page_height() -> usize {
    terminal::size()
        .map(|(_, rows)| rows.saturating_sub(FIXED_SCREEN_LINES) as usize)
        .unwrap_or(5)
        .max(5)
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

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    #[test]
    fn parses_review_actions() {
        assert_eq!(
            action_from_key(key(KeyCode::Char('t'))),
            Action::ChooseTheirs
        );
        assert_eq!(
            action_from_key(key(KeyCode::Char('y'))),
            Action::ChooseTheirs
        );
        assert_eq!(action_from_key(key(KeyCode::Char('o'))), Action::ChooseOurs);
        assert_eq!(action_from_key(key(KeyCode::Char('n'))), Action::ChooseOurs);
        assert_eq!(action_from_key(key(KeyCode::Char('q'))), Action::Quit);
        assert_eq!(action_from_key(key(KeyCode::Esc)), Action::Quit);
        assert_eq!(action_from_key(ctrl(KeyCode::Char('c'))), Action::Quit);
        assert_eq!(action_from_key(key(KeyCode::Right)), Action::Next);
        assert_eq!(action_from_key(key(KeyCode::Left)), Action::Previous);
        assert_eq!(action_from_key(ctrl(KeyCode::Char('s'))), Action::Save);
        assert_eq!(action_from_key(key(KeyCode::Char('S'))), Action::SaveAs);
        assert_eq!(action_from_key(ctrl(KeyCode::Char('S'))), Action::SaveAs);
        assert_eq!(action_from_key(key(KeyCode::Char('x'))), Action::None);
    }
}
