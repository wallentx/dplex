use std::borrow::Cow;
use std::collections::HashMap;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};

use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
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
    Undo,
    Finish,
    Quit,
    Next,
    Previous,
    ScrollDown,
    ScrollUp,
    PanLeft,
    PanRight,
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
    saved_decisions: Option<HashMap<usize, Decision>>,
    history: Vec<(usize, Option<Decision>)>,
    current_index: usize,
    column: usize,
    scroll_top: usize,
    save_path: PathBuf,
    status_message: Option<String>,
}

pub enum SaveError {
    Open(String),
    Write(String),
}

pub fn review_conflicts<F>(
    path: &Path,
    source_lines: &[String],
    conflicts: &[Conflict],
    initial_save_path: PathBuf,
    mut save: F,
) -> Result<ReviewOutcome, String>
where
    F: FnMut(&HashMap<usize, Decision>, &Path) -> Result<(), SaveError>,
{
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err("interactive review requires a TTY".to_string());
    }

    let mut state = ReviewState::new(path, source_lines, conflicts, initial_save_path);
    let mut terminal = TerminalSession::enter()?;
    let quit = loop {
        terminal.draw(&state)?;
        let action = read_action()?;
        match action {
            Action::ChooseTheirs => state.choose(Decision::Incoming),
            Action::ChooseOurs => state.choose(Decision::Current),
            Action::Undo => state.undo(),
            Action::Finish if state.decisions.len() == state.conflicts.len() => {
                if state.saved_decisions.as_ref() == Some(&state.decisions)
                    || state.save(None, &mut save)
                {
                    break false;
                }
            }
            Action::Finish => {
                state.status_message = Some("Choose a side for each hunk before finishing.".into());
            }
            Action::Quit => {
                if state.saved_decisions.as_ref() == Some(&state.decisions) {
                    break true;
                }
                match prompt_for_quit(&mut terminal, &state)? {
                    QuitChoice::Discard => break true,
                    QuitChoice::Save if state.save(None, &mut save) => break true,
                    _ => {}
                }
            }
            Action::Next => state.focus((state.current_index + 1) % conflicts.len()),
            Action::Previous => {
                state.focus((state.current_index + conflicts.len() - 1) % conflicts.len());
            }
            Action::ScrollDown => state.scroll_top = state.scroll_top.saturating_add(1),
            Action::ScrollUp => state.scroll_top = state.scroll_top.saturating_sub(1),
            Action::PageDown => state.scroll_top = state.scroll_top.saturating_add(page_height()),
            Action::PageUp => state.scroll_top = state.scroll_top.saturating_sub(page_height()),
            Action::PanLeft | Action::PanRight => {
                let width = render_area(terminal.terminal.get_frame().area()).width as usize;
                let columns = width.saturating_sub(6 + line_number_width(source_lines.len()));
                let step = columns.saturating_sub(2).clamp(1, 8);
                if action == Action::PanLeft {
                    state.column = state.column.saturating_sub(step);
                } else {
                    let longest = build_document_view(&state)
                        .iter()
                        .map(|line| Span::raw(line.display_text()).width())
                        .max()
                        .unwrap_or(0);
                    state.column = state
                        .column
                        .saturating_add(step)
                        .min(longest.saturating_sub(columns));
                }
            }
            Action::Save => {
                state.save(None, &mut save);
            }
            Action::SaveAs => {
                if let Some(path) = prompt_for_save_path(&mut terminal, &state, "Save as: ")? {
                    state.save(Some(path), &mut save);
                } else {
                    state.status_message = Some("save as canceled".to_string());
                }
            }
            Action::None => {}
        }
    };

    Ok(ReviewOutcome {
        decisions: state
            .saved_decisions
            .ok_or("the last write was incomplete")?,
        quit,
        save_path: state.save_path,
    })
}

impl<'a> ReviewState<'a> {
    fn new(
        path: &'a Path,
        source_lines: &'a [String],
        conflicts: &'a [Conflict],
        save_path: PathBuf,
    ) -> Self {
        let mut state = Self {
            path,
            source_lines,
            conflicts,
            decisions: HashMap::new(),
            saved_decisions: Some(HashMap::new()),
            history: Vec::new(),
            current_index: 0,
            column: 0,
            scroll_top: 0,
            save_path,
            status_message: None,
        };
        focus_current_conflict(&mut state);
        state
    }

    fn choose(&mut self, decision: Decision) {
        let old = self.decisions.insert(self.current_index, decision);
        if old != Some(decision) {
            self.history.push((self.current_index, old));
        }
        self.focus(next_pending_index(self).unwrap_or(self.current_index));
    }

    fn undo(&mut self) {
        if let Some((index, old)) = self.history.pop() {
            match old {
                Some(decision) => self.decisions.insert(index, decision),
                None => self.decisions.remove(&index),
            };
            self.focus(index);
        } else {
            self.status_message = Some("There are no choices to undo.".into());
        }
    }

    fn focus(&mut self, index: usize) {
        self.current_index = index;
        self.column = 0;
        self.status_message = None;
        focus_current_conflict(self);
    }

    fn save<F>(&mut self, path: Option<PathBuf>, save: &mut F) -> bool
    where
        F: FnMut(&HashMap<usize, Decision>, &Path) -> Result<(), SaveError>,
    {
        let destination = path.as_deref().unwrap_or(&self.save_path);
        match save(&self.decisions, destination) {
            Ok(()) => {
                self.status_message = Some(format!("saved to {}", destination.display()));
                if let Some(path) = path {
                    self.save_path = path;
                }
                self.saved_decisions = Some(self.decisions.clone());
                true
            }
            Err(error) => {
                self.status_message = Some(match error {
                    SaveError::Open(message) => message,
                    SaveError::Write(message) => {
                        // Save As can name the checkpoint through a symlink or hard link.
                        self.saved_decisions = None;
                        message
                    }
                });
                false
            }
        }
    }
}

enum QuitChoice {
    Save,
    Discard,
    Continue,
}

fn prompt_for_quit(
    terminal: &mut TerminalSession,
    state: &ReviewState<'_>,
) -> Result<QuitChoice, String> {
    loop {
        let prompt = if terminal.terminal.get_frame().area().width >= 65 {
            "Unsaved choices: [s] save and quit  [d] discard  [Esc] continue"
        } else {
            "Unsaved choices\n[s] save and quit\n[d] discard\n[Esc] continue"
        };
        terminal.draw_with_prompt(state, prompt, "")?;
        if let Event::Key(key) = event::read().map_err(|error| error.to_string())? {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match key.code {
                KeyCode::Char('s') => return Ok(QuitChoice::Save),
                KeyCode::Char('d') if key.modifiers.is_empty() => return Ok(QuitChoice::Discard),
                KeyCode::Esc => return Ok(QuitChoice::Continue),
                _ => {}
            }
        }
    }
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
    let choice = match state.decisions.get(&state.current_index) {
        None => "choose ours or theirs",
        Some(Decision::Current) => "chose ours",
        Some(Decision::Incoming) => "chose theirs",
    };
    let finish_hint = if counts.pending == 0 {
        "  Enter saves and finishes"
    } else {
        ""
    };
    let prompt = format!(
        "[{}/{}] {choice}{finish_hint}",
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
        .map(|line| document_line(line, number_width, body_width, state))
        .collect();
    frame.render_widget(Paragraph::new(lines), chunks[2]);

    frame.render_widget(separator(area.width), chunks[3]);

    let visible_end = (scroll_top + body_height).min(document.len());
    let position = format!(
        "Lines {}-{} of {}  Col {}",
        (scroll_top + 1).min(document.len().max(1)),
        visible_end,
        document.len(),
        state.column + 1
    );
    let footer = match &state.status_message {
        Some(message) => message.clone(),
        None => position,
    };
    let controls = Paragraph::new(vec![
        Line::from(vec![
            key("o"),
            label(" ours  "),
            key("t"),
            label(" theirs  "),
            key("u"),
            label(" undo  "),
            key("h/l"),
            label(" pan  "),
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
enum DocumentLine<'a> {
    Context {
        line_number: usize,
        text: &'a str,
    },
    Current {
        conflict_index: usize,
        line_number: usize,
        text: &'a str,
    },
    Incoming {
        conflict_index: usize,
        text: &'a str,
    },
    Resolved {
        conflict_index: usize,
        line_number: Option<usize>,
        text: &'a str,
        dirty: bool,
    },
}

fn build_document_view<'a>(state: &ReviewState<'a>) -> Vec<DocumentLine<'a>> {
    let mut lines = Vec::new();
    let mut cursor = 0;

    for (conflict_index, conflict) in state.conflicts.iter().enumerate() {
        let start = conflict.start_line.saturating_sub(1);
        for line_number in cursor..start {
            if let Some(text) = state.source_lines.get(line_number) {
                lines.push(DocumentLine::Context {
                    line_number: line_number + 1,
                    text,
                });
            }
        }

        let decision = state.decisions.get(&conflict_index);
        let saved = state
            .saved_decisions
            .as_ref()
            .and_then(|saved| saved.get(&conflict_index));
        let dirty = saved != decision;
        match decision.filter(|_| conflict_index != state.current_index) {
            Some(Decision::Current) => {
                for (offset, text) in conflict.current.iter().enumerate() {
                    lines.push(DocumentLine::Resolved {
                        conflict_index,
                        line_number: Some(start + offset + 1),
                        text,
                        dirty,
                    });
                }
            }
            Some(Decision::Incoming) => {
                for text in &conflict.incoming {
                    lines.push(DocumentLine::Resolved {
                        conflict_index,
                        line_number: None,
                        text,
                        dirty,
                    });
                }
            }
            None => {
                for (offset, text) in conflict.current.iter().enumerate() {
                    lines.push(DocumentLine::Current {
                        conflict_index,
                        line_number: start + offset + 1,
                        text,
                    });
                }

                for text in &conflict.incoming {
                    lines.push(DocumentLine::Incoming {
                        conflict_index,
                        text,
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
                text,
            });
        }
    }

    if lines.is_empty() {
        lines.push(DocumentLine::Context {
            line_number: 1,
            text: "(empty file)\n",
        });
    }
    lines
}

fn document_line(
    line: &DocumentLine<'_>,
    number_width: usize,
    width: usize,
    state: &ReviewState<'_>,
) -> Line<'static> {
    let selected = line.conflict_index() == Some(state.current_index);
    let (number, kind) = match line {
        DocumentLine::Context { line_number, .. } => (Some(*line_number), None),
        DocumentLine::Current { line_number, .. } => (Some(*line_number), Some(DiffKind::Current)),
        DocumentLine::Incoming { .. } => (None, Some(DiffKind::Incoming)),
        DocumentLine::Resolved { line_number, .. } => (*line_number, None),
    };
    let (marker, marker_style, gutter_style, content_style) = match kind {
        Some(kind) => (
            if kind == DiffKind::Current {
                " -"
            } else {
                " +"
            },
            diff_marker_style(kind, selected),
            diff_gutter_style(selected, kind),
            diff_content_style(kind, selected),
        ),
        None => {
            let style = if matches!(line, DocumentLine::Resolved { dirty: true, .. }) {
                Style::default()
                    .fg(warm_yellow())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Rgb(226, 226, 226))
            };
            ("  ", neutral_gutter_style(), neutral_gutter_style(), style)
        }
    };
    let number = number.map(|n| n.to_string()).unwrap_or_default();
    let gutter = format!(" {number:>number_width$} | ");
    let content_width = width.saturating_sub(marker.len() + gutter.len());
    let content = visible_columns(&line.display_text(), state.column, content_width);
    let content = if selected {
        pad_to_width(&content, content_width)
    } else {
        content
    };
    Line::from(vec![
        Span::styled(marker, marker_style),
        Span::styled(gutter, gutter_style),
        Span::styled(content, content_style),
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

fn visible_columns(text: &str, offset: usize, width: usize) -> String {
    let span = Span::raw(text);
    let clipped = width > 2 && span.width() > offset.saturating_add(width);
    let right = offset.saturating_add(width - usize::from(clipped));
    let mut column = 0;
    let mut output = String::new();
    for grapheme in span.styled_graphemes(Style::default()) {
        if column >= right {
            break;
        }
        let end = column + Span::raw(grapheme.symbol).width();
        if end > offset && end <= right {
            if column >= offset {
                output.push_str(grapheme.symbol);
            } else {
                // The viewport starts inside a wide grapheme.
                output.push_str(&" ".repeat(end - offset));
            }
        }
        column = end;
    }
    if clipped {
        output.push('…');
    }
    output
}

fn pad_to_width(text: &str, width: usize) -> String {
    let length = Span::raw(text).width();
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

impl DocumentLine<'_> {
    fn display_text(&self) -> Cow<'_, str> {
        let text = match self {
            Self::Context { text, .. }
            | Self::Current { text, .. }
            | Self::Incoming { text, .. }
            | Self::Resolved { text, .. } => text,
        };
        if let Some(content) = text.strip_suffix("\r\n") {
            if self.conflict_index().is_some() {
                return Cow::Owned(format!("{content} [CRLF]"));
            }
            Cow::Borrowed(content)
        } else if let Some(content) = text.strip_suffix('\n') {
            if content.is_empty() && self.conflict_index().is_some() {
                return Cow::Borrowed("[blank line]");
            }
            Cow::Borrowed(content)
        } else {
            Cow::Owned(format!("{text} [no newline]"))
        }
    }

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

fn read_action() -> Result<Action, String> {
    loop {
        match event::read().map_err(|error| error.to_string())? {
            Event::Resize(_, _) => return Ok(Action::None),
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                let action = action_from_key(key);
                if action != Action::None {
                    return Ok(action);
                }
            }
            _ => {}
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
        KeyCode::Char('u') => Action::Undo,
        KeyCode::Enter => Action::Finish,
        KeyCode::Char('h') => Action::PanLeft,
        KeyCode::Char('l') => Action::PanRight,
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
            if key.kind == KeyEventKind::Release {
                continue;
            }
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
    let mut lines: Vec<_> = prompt
        .lines()
        .map(|line| Line::styled(line, Style::default().fg(Color::Yellow)))
        .collect();
    if let Some(last) = lines.last_mut() {
        last.push_span(Span::raw(input).style(Style::default()));
    }
    let height = area.height.min(lines.len() as u16);
    let prompt_area = Rect {
        x: area.x,
        y: area.bottom() - height,
        width: area.width,
        height,
    };
    frame.render_widget(Clear, prompt_area);
    frame.render_widget(Paragraph::new(lines), prompt_area);
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
    fn revisits_and_undoes_past_checkpoints() {
        let source = vec!["delete me\n".to_string(), "old\n".to_string()];
        let incoming = vec!["new\n".to_string()];
        let conflict = |line: usize, theirs: Vec<String>| Conflict {
            start_line: line,
            end_line: line,
            current_label: "ours".into(),
            incoming_label: "theirs".into(),
            current: vec![source[line - 1].clone()],
            incoming: theirs,
        };
        let conflicts = [conflict(1, vec![]), conflict(2, incoming)];
        let path = Path::new("ours");
        let mut state = ReviewState::new(path, &source, &conflicts, path.into());
        state.choose(Decision::Incoming);
        assert_eq!(state.current_index, 1);
        assert!(state.save(None, &mut |_, _| Ok(())));
        state.choose(Decision::Incoming);
        assert_eq!(state.decisions.len(), 2);
        state.focus(0);
        assert!(build_document_view(&state).iter().any(|line| matches!(
            line,
            DocumentLine::Current {
                conflict_index: 0,
                text: "delete me\n",
                ..
            }
        )));
        state.choose(Decision::Current);
        state.undo();
        assert_eq!(state.decisions[&0], Decision::Incoming);
        state.undo();
        assert_eq!(Some(&state.decisions), state.saved_decisions.as_ref());
        state.undo();
        assert!(state.decisions.is_empty());
        assert_eq!(state.saved_decisions.as_ref().unwrap().len(), 1);
        assert!(
            !state.save(Some("bad".into()), &mut |_, _| Err(SaveError::Open(
                "denied".into()
            )))
        );
        assert_eq!(state.save_path, path);
        assert_eq!(state.saved_decisions.as_ref().unwrap().len(), 1);
    }

    #[test]
    fn incomplete_write_invalidates_a_completed_checkpoint_until_retry() {
        let source = crate::parser::split_lines("<<<<<<< ours\na\n=======\nb\n>>>>>>> theirs\n");
        let conflicts = crate::parser::parse_conflicts(&source, None).unwrap();
        let path = Path::new("merged");
        let mut state = ReviewState::new(path, &source, &conflicts, path.into());
        state.choose(Decision::Incoming);
        assert!(state.save(None, &mut |_, _| Ok(())));
        assert!(!state.save(None, &mut |_, _| Err(SaveError::Write("disk full".into()))));
        assert_eq!(state.saved_decisions, None);
        assert!(state.save(None, &mut |_, _| Ok(())));
        assert_eq!(state.saved_decisions.as_ref(), Some(&state.decisions));
    }

    #[test]
    fn displays_distinct_line_endings() {
        let line = |text| DocumentLine::Incoming {
            conflict_index: 0,
            text,
        };
        assert_eq!(line("x\n").display_text(), "x");
        assert_eq!(line("x\r\n").display_text(), "x [CRLF]");
        assert_eq!(line("x\r\r\n").display_text(), "x\r [CRLF]");
        assert_eq!(line("x").display_text(), "x [no newline]");
        assert_eq!(line("\n").display_text(), "[blank line]");
    }

    #[test]
    fn pans_by_terminal_columns_without_splitting_graphemes() {
        let text = "hello界e\u{301}world";
        assert_eq!(visible_columns(text, 0, 5), "hell…");
        assert_eq!(visible_columns(text, 5, 3), "界…");
        assert_eq!(visible_columns(text, 6, 3), " e\u{301}…");
        assert_eq!(visible_columns(text, 8, 5), "world");
        assert_eq!(visible_columns(text, 5, 2), "界");
        assert_eq!(visible_columns(text, 0, 0), "");
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
        assert_eq!(action_from_key(key(KeyCode::Char('u'))), Action::Undo);
        assert_eq!(action_from_key(key(KeyCode::Enter)), Action::Finish);
        assert_eq!(action_from_key(key(KeyCode::Char('h'))), Action::PanLeft);
        assert_eq!(action_from_key(key(KeyCode::Char('l'))), Action::PanRight);
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
