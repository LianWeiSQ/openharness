//! Terminal UI state for the Rust rewrite.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use openagent_app_server::{
    approval_response_payload, question_dismiss_payload, question_reply_payload,
};
use openagent_app_server_client::{
    RemoteAuth, RemoteRuntimeClient, event_sequence, events_from_payload, session_id_from_payload,
    turn_id_from_payload,
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

pub const CRATE_NAME: &str = env!("CARGO_PKG_NAME");

const BUILTIN_COMMANDS: &[(&str, &str)] = &[
    ("/help", "show TUI commands"),
    ("/connect <url>", "connect to an App Bridge server"),
    ("/sessions [query]", "open/search recent session picker"),
    ("/resume <id>", "resume a session by id or unique prefix"),
    ("/rename <title>", "rename the current session"),
    ("/archive", "archive the current session"),
    ("/unarchive", "unarchive the current session"),
    ("/delete", "delete the current session"),
    (
        "/transcript [limit]",
        "show recent messages from the current session",
    ),
    ("/new", "start a new session"),
    ("/fork", "fork the current session"),
    ("/children", "list child sessions"),
    ("/parent", "navigate to the parent session"),
    ("/share", "share the current session"),
    ("/unshare", "remove the current shared session link"),
    ("/compact", "compact the current session context"),
    ("/details", "show current turn and trace details"),
    ("/models [id]", "list models or set the current model"),
    ("/agents", "list available agent profiles"),
    ("/agent <id>", "set the current agent profile"),
    ("/variant <name>", "set the current model variant"),
    ("/themes", "open theme picker"),
    ("/thinking <level>", "set reasoning effort or visibility"),
    ("/config", "show loaded TUI configuration"),
    ("/keybinds", "show active key bindings"),
    ("/usage", "show token and cost totals"),
    ("/warnings", "show aggregated runtime warnings"),
    ("/tool-details [on|off]", "toggle tool metadata details"),
    ("/editor", "edit prompt in an external editor"),
    ("/stash <draft>", "stash a draft prompt"),
    ("/unstash", "restore the latest stashed prompt"),
    ("/stashes", "list stashed prompt drafts"),
    ("/files [query]", "search workspace files for @ attachments"),
    (
        "/attach <path[:range]>",
        "insert a file/image reference into the prompt",
    ),
    ("/export", "export the current session"),
    ("/init", "initialize OpenAgent project files"),
    ("/undo", "undo the last reversible change"),
    ("/redo", "redo the last undone change"),
    ("/clear", "clear the visible timeline"),
    ("/status", "show current session, turn, and model status"),
    ("/allow [once|always]", "approve the active tool request"),
    (
        "/deny [note]",
        "deny the active tool request with an optional note",
    ),
    ("/answer <text>", "answer the active question request"),
    ("/dismiss [note]", "dismiss the active question request"),
    ("/commands", "list project/global custom commands"),
];

#[must_use]
pub const fn crate_name() -> &'static str {
    CRATE_NAME
}

#[must_use]
pub fn command_name() -> &'static str {
    "openagent-tui"
}

#[must_use]
pub fn client_crate_name() -> &'static str {
    openagent_app_server_client::crate_name()
}

#[must_use]
pub fn server_crate_name() -> &'static str {
    openagent_app_server::crate_name()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalUiOptions {
    pub title: String,
    pub status: String,
}

impl Default for TerminalUiOptions {
    fn default() -> Self {
        Self {
            title: "OpenAgent".to_string(),
            status: "ready".to_string(),
        }
    }
}

pub trait TerminalEventHandler {
    fn initial_lines(&mut self) -> Vec<TimelineLine> {
        Vec::new()
    }

    fn poll_app_events(&mut self) -> Result<Vec<Value>, String> {
        Ok(Vec::new())
    }

    fn poll_control_request(&mut self) -> Result<Option<Value>, String> {
        Ok(None)
    }

    fn record_control_response(&mut self, _payload: &Value) -> Result<(), String> {
        Ok(())
    }

    fn drain_app_events(&mut self) -> Vec<Value> {
        Vec::new()
    }

    fn search_files(&mut self, _query: &str) -> Result<Vec<ComposerFileCandidate>, String> {
        Ok(Vec::new())
    }

    fn search_sessions(&mut self, _query: &str) -> Result<Vec<Value>, String> {
        Ok(Vec::new())
    }

    fn list_models(&mut self) -> Result<Value, String> {
        Ok(json!({"models": []}))
    }

    fn list_agents(&mut self) -> Result<Value, String> {
        Ok(json!({"agents": []}))
    }

    fn handle_submit(&mut self, prompt: &str) -> Result<Vec<TimelineLine>, String>;

    fn handle_command(&mut self, command: &str) -> Result<Vec<TimelineLine>, String>;

    fn handle_approval_response(&mut self, _payload: &Value) -> Result<Vec<TimelineLine>, String> {
        Ok(Vec::new())
    }

    fn handle_question_response(&mut self, _payload: &Value) -> Result<Vec<TimelineLine>, String> {
        Ok(Vec::new())
    }
}

pub fn run_terminal_ui<H: TerminalEventHandler>(
    options: TerminalUiOptions,
    mut handler: H,
) -> Result<(), String> {
    enable_raw_mode().map_err(|error| error.to_string())?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).map_err(|error| {
        let _ = disable_raw_mode();
        error.to_string()
    })?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).map_err(|error| {
        let _ = disable_raw_mode();
        error.to_string()
    })?;
    let result = terminal_ui_loop(&mut terminal, options, &mut handler);
    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = terminal.show_cursor();
    result
}

fn terminal_ui_loop<H: TerminalEventHandler>(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    options: TerminalUiOptions,
    handler: &mut H,
) -> Result<(), String> {
    let mut state = TuiState::new();
    state.status = options.status;
    state.timeline.extend(handler.initial_lines());
    state.timeline.push(TimelineLine::new(
        "status",
        "Type a prompt, /sessions, /resume <id>, /new, /fork, /interrupt, or /exit.",
        true,
    ));
    apply_handler_output(&mut state, handler, Vec::new());
    loop {
        match handler.poll_control_request() {
            Ok(Some(request)) => handle_remote_control_request(&mut state, handler, &request),
            Ok(None) => {}
            Err(error) => {
                state
                    .timeline
                    .push(TimelineLine::new("warning", error, true));
                state.status = "control poll failed".to_string();
            }
        }
        match handler.poll_app_events() {
            Ok(events) => apply_app_event_values(&mut state, events),
            Err(error) => {
                if state.status != "remote poll failed" {
                    state
                        .timeline
                        .push(TimelineLine::new("warning", error, true));
                }
                state.status = "remote poll failed".to_string();
            }
        }
        terminal
            .draw(|frame| draw_terminal_frame(frame, &options.title, &state))
            .map_err(|error| error.to_string())?;
        if !event::poll(Duration::from_millis(250)).map_err(|error| error.to_string())? {
            continue;
        }
        let Event::Key(key) = event::read().map_err(|error| error.to_string())? else {
            continue;
        };
        if handle_key_event(key, &mut state, handler)? {
            break;
        }
    }
    Ok(())
}

fn draw_terminal_frame(frame: &mut ratatui::Frame<'_>, title: &str, state: &TuiState) {
    let area = frame.area();
    let has_interaction = state.active_interaction_focus().is_some();
    let has_file_picker = state.file_picker.is_some();
    let has_session_picker = state.session_picker.is_some();
    let has_model_picker = state.model_picker.is_some();
    let has_agent_picker = state.agent_picker.is_some();
    let has_choice_picker = state.choice_picker.is_some();
    let mut constraints = vec![Constraint::Length(3), Constraint::Min(5)];
    if has_interaction
        || has_file_picker
        || has_session_picker
        || has_model_picker
        || has_agent_picker
        || has_choice_picker
    {
        constraints.push(Constraint::Length(9));
    }
    constraints.push(Constraint::Length(3));
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);
    let header = Paragraph::new(Line::from(vec![
        Span::styled(
            title.to_string(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!("  status: {}", state.status)),
    ]))
    .block(Block::default().borders(Borders::ALL).title("App Bridge"));
    frame.render_widget(header, chunks[0]);

    let visible = state.timeline.iter().rev().take(200).rev().map(|line| {
        let style = timeline_style(&state.config, line.kind.as_str());
        ListItem::new(Line::from(vec![
            Span::styled(
                format!("[{}] ", line.kind),
                style.add_modifier(Modifier::BOLD),
            ),
            Span::raw(line.text.clone()),
        ]))
    });
    let timeline = List::new(visible.collect::<Vec<_>>())
        .block(Block::default().borders(Borders::ALL).title("Timeline"));
    frame.render_widget(timeline, chunks[1]);

    let prompt_index = if has_interaction {
        draw_interaction_dock(frame, chunks[2], state);
        3
    } else if has_choice_picker {
        draw_choice_picker_dock(frame, chunks[2], state);
        3
    } else if has_agent_picker {
        draw_agent_picker_dock(frame, chunks[2], state);
        3
    } else if has_model_picker {
        draw_model_picker_dock(frame, chunks[2], state);
        3
    } else if has_session_picker {
        draw_session_picker_dock(frame, chunks[2], state);
        3
    } else if has_file_picker {
        draw_file_picker_dock(frame, chunks[2], state);
        3
    } else {
        2
    };
    let input = Paragraph::new(state.input_buffer.as_str())
        .block(Block::default().borders(Borders::ALL).title("Prompt"))
        .wrap(Wrap { trim: false });
    frame.render_widget(input, chunks[prompt_index]);
}

fn draw_choice_picker_dock(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    state: &TuiState,
) {
    let title = state
        .choice_picker
        .as_ref()
        .map(|picker| picker.kind.title())
        .unwrap_or("Choices");
    let lines = choice_picker_dock_lines(state);
    let dock = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(dock, area);
}

fn choice_picker_dock_lines(state: &TuiState) -> Vec<Line<'static>> {
    let Some(picker) = state.choice_picker.as_ref() else {
        return Vec::new();
    };
    let mut lines = vec![Line::from(vec![
        Span::styled("Query ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(if picker.query.is_empty() {
            "(all)".to_string()
        } else {
            picker.query.clone()
        }),
        Span::styled(
            "  Type to filter, Enter select, Esc close",
            Style::default().fg(Color::DarkGray),
        ),
    ])];
    if picker.candidates.is_empty() {
        lines.push(Line::from(format!(
            "No matching {}",
            picker.kind.item_label()
        )));
        return lines;
    }
    for (index, choice) in picker.candidates.iter().enumerate().take(6) {
        let marker = if picker.selected == index { ">" } else { " " };
        let suffix = if picker.kind == ChoicePickerKind::Theme && state.config.theme == *choice {
            "  current"
        } else {
            ""
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!("{marker} {}. ", index + 1),
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(format!("{choice}{suffix}")),
        ]));
    }
    lines
}

fn draw_agent_picker_dock(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    state: &TuiState,
) {
    let lines = agent_picker_dock_lines(state);
    let dock = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Agents")
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(dock, area);
}

fn agent_picker_dock_lines(state: &TuiState) -> Vec<Line<'static>> {
    let Some(picker) = state.agent_picker.as_ref() else {
        return Vec::new();
    };
    let mut lines = vec![Line::from(vec![
        Span::styled("Query ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(if picker.query.is_empty() {
            "(all)".to_string()
        } else {
            picker.query.clone()
        }),
        Span::styled(
            "  Type to filter, Enter select, Esc close",
            Style::default().fg(Color::DarkGray),
        ),
    ])];
    if picker.candidates.is_empty() {
        lines.push(Line::from("No matching agents"));
        return lines;
    }
    for (index, agent) in picker.candidates.iter().enumerate().take(6) {
        let marker = if picker.selected == index { ">" } else { " " };
        lines.push(Line::from(vec![
            Span::styled(
                format!("{marker} {}. ", index + 1),
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(agent_picker_label(agent)),
        ]));
    }
    lines
}

fn draw_model_picker_dock(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    state: &TuiState,
) {
    let lines = model_picker_dock_lines(state);
    let dock = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Models")
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(dock, area);
}

fn model_picker_dock_lines(state: &TuiState) -> Vec<Line<'static>> {
    let Some(picker) = state.model_picker.as_ref() else {
        return Vec::new();
    };
    let mut lines = vec![Line::from(vec![
        Span::styled("Query ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(if picker.query.is_empty() {
            "(all)".to_string()
        } else {
            picker.query.clone()
        }),
        Span::styled(
            "  Type to filter, Enter select, Esc close",
            Style::default().fg(Color::DarkGray),
        ),
    ])];
    if picker.candidates.is_empty() {
        lines.push(Line::from("No matching models"));
        return lines;
    }
    for (index, model) in picker.candidates.iter().enumerate().take(6) {
        let marker = if picker.selected == index { ">" } else { " " };
        lines.push(Line::from(vec![
            Span::styled(
                format!("{marker} {}. ", index + 1),
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(model_picker_label(model)),
        ]));
    }
    lines
}

fn draw_session_picker_dock(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    state: &TuiState,
) {
    let lines = session_picker_dock_lines(state);
    let dock = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Sessions")
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(dock, area);
}

fn session_picker_dock_lines(state: &TuiState) -> Vec<Line<'static>> {
    let Some(picker) = state.session_picker.as_ref() else {
        return Vec::new();
    };
    let mut lines = vec![Line::from(vec![
        Span::styled("Query ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(if picker.query.is_empty() {
            "(recent)".to_string()
        } else {
            picker.query.clone()
        }),
        Span::styled(
            "  Type to filter, Enter resume, Esc close",
            Style::default().fg(Color::DarkGray),
        ),
    ])];
    if picker.candidates.is_empty() {
        lines.push(Line::from("No matching sessions"));
        return lines;
    }
    for (index, session) in picker.candidates.iter().enumerate().take(6) {
        let marker = if picker.selected == index { ">" } else { " " };
        lines.push(Line::from(vec![
            Span::styled(
                format!("{marker} {}. ", index + 1),
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(session_picker_label(session)),
        ]));
    }
    lines
}

fn draw_file_picker_dock(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    state: &TuiState,
) {
    let lines = file_picker_dock_lines(state);
    let dock = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Composer: Files")
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(dock, area);
}

fn file_picker_dock_lines(state: &TuiState) -> Vec<Line<'static>> {
    let Some(picker) = state.file_picker.as_ref() else {
        return Vec::new();
    };
    let mut lines = vec![Line::from(vec![
        Span::styled("Query ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(if picker.query.is_empty() {
            "(all)".to_string()
        } else {
            picker.query.clone()
        }),
        Span::styled(
            "  Type to filter, Enter attach, Esc close",
            Style::default().fg(Color::DarkGray),
        ),
    ])];
    if picker.candidates.is_empty() {
        lines.push(Line::from("No matching files"));
        return lines;
    }
    for (index, candidate) in picker.candidates.iter().enumerate().take(6) {
        let marker = if picker.selected == index { ">" } else { " " };
        lines.push(Line::from(vec![
            Span::styled(
                format!("{marker} {}. ", index + 1),
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(candidate.reference.clone()),
            Span::styled(
                format!("  {}", candidate.kind),
                Style::default().fg(Color::DarkGray),
            ),
        ]));
    }
    lines
}

fn draw_interaction_dock(
    frame: &mut ratatui::Frame<'_>,
    area: ratatui::layout::Rect,
    state: &TuiState,
) {
    let (title, lines) = match state.active_interaction_focus() {
        Some(InteractionFocus::Approval) => ("Interaction: Approval", approval_dock_lines(state)),
        Some(InteractionFocus::Question) => ("Interaction: Question", question_dock_lines(state)),
        None => return,
    };
    let dock = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(Style::default().fg(Color::Yellow)),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(dock, area);
}

fn approval_dock_lines(state: &TuiState) -> Vec<Line<'static>> {
    let Some(approval) = state.active_approval.as_ref() else {
        return Vec::new();
    };
    let tool = string_field(approval, "tool_name").if_empty_then(|| "tool".to_string());
    let input = approval
        .get("tool_input")
        .map(compact_json)
        .unwrap_or_else(|| "{}".to_string());
    let mut lines = vec![Line::from(vec![
        Span::styled("Tool ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(format!("{tool} {input}")),
    ])];
    if let Some(preview) = approval.get("preview").filter(|value| value.is_object()) {
        for line in preview_lines(preview).into_iter().take(2) {
            lines.push(Line::from(Span::raw(line)));
        }
    }
    let options = ["Allow once", "Always allow", "Deny"];
    for (index, option) in options.iter().enumerate() {
        let marker = if state.interaction.selected == index {
            ">"
        } else {
            " "
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!("{marker} {}. ", index + 1),
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(*option),
        ]));
    }
    lines.push(Line::from(Span::styled(
        "Enter selects, 1-3 quick-pick, Esc denies",
        Style::default().fg(Color::DarkGray),
    )));
    lines
}

fn question_dock_lines(state: &TuiState) -> Vec<Line<'static>> {
    let Some(question) = state.active_question.as_ref() else {
        return Vec::new();
    };
    let questions = question_items(question);
    if questions.is_empty() {
        return vec![Line::from(
            "No question details. Use /answer or Esc to dismiss.",
        )];
    }
    let index = state.interaction.question_index.min(questions.len() - 1);
    let item = &questions[index];
    let header = item
        .get("header")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or("Question");
    let text = item
        .get("question")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                format!("{} {}/{} ", header, index + 1, questions.len()),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(text.to_string()),
        ]),
        Line::from(Span::styled(
            "Up/Down choose, Enter submit, type custom answer, Esc dismiss",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    let options = question_option_values(item);
    if options.is_empty() {
        lines.push(Line::from("No options. Type an answer, then Enter."));
    } else {
        for (option_index, option) in options.iter().enumerate().take(5) {
            let marker = if state.interaction.selected == option_index {
                ">"
            } else {
                " "
            };
            let label = option
                .get("label")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let description = option
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let suffix = if description.is_empty() {
                String::new()
            } else {
                format!(" - {description}")
            };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{marker} {}. ", option_index + 1),
                    Style::default().fg(Color::Yellow),
                ),
                Span::raw(format!("{label}{suffix}")),
            ]));
        }
    }
    if !state.interaction.custom_answer.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("Custom: ", Style::default().fg(Color::Yellow)),
            Span::raw(state.interaction.custom_answer.clone()),
        ]));
    }
    lines
}

fn handle_key_event<H: TerminalEventHandler>(
    key: KeyEvent,
    state: &mut TuiState,
    handler: &mut H,
) -> Result<bool, String> {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return Ok(true);
    }
    let approval_start = state.approval_responses.len();
    let question_start = state.question_responses.len();
    if state.handle_interaction_key(&key) {
        dispatch_new_interaction_responses(state, handler, approval_start, question_start)?;
        return Ok(false);
    }
    if state.choice_picker.is_some() {
        handle_choice_picker_key(key, state, handler)?;
        return Ok(false);
    }
    if state.agent_picker.is_some() {
        handle_agent_picker_key(key, state, handler)?;
        return Ok(false);
    }
    if state.model_picker.is_some() {
        handle_model_picker_key(key, state, handler)?;
        return Ok(false);
    }
    if state.session_picker.is_some() {
        handle_session_picker_key(key, state, handler)?;
        return Ok(false);
    }
    if state.file_picker.is_some() {
        handle_file_picker_key(key, state, handler)?;
        return Ok(false);
    }
    if keybind_matches(&state.config, &key, "stash", "ctrl+s") {
        state.stash_current_input();
        return Ok(false);
    }
    if keybind_matches(&state.config, &key, "unstash", "ctrl+y") {
        state.restore_latest_stash();
        return Ok(false);
    }
    if keybind_matches(&state.config, &key, "editor", "ctrl+e") {
        if let Err(error) = state.edit_input_with_external_editor() {
            state.timeline.push(TimelineLine::new("error", error, true));
        }
        return Ok(false);
    }
    match key.code {
        KeyCode::Esc => return Ok(true),
        KeyCode::Up => {
            state.history_previous();
        }
        KeyCode::Down => {
            state.history_next();
        }
        KeyCode::Backspace => {
            state.input_buffer.pop();
        }
        KeyCode::Char('@')
            if !key.modifiers.contains(KeyModifiers::CONTROL)
                && !state.input_buffer.trim_start().starts_with('/') =>
        {
            if let Err(error) = open_file_picker_from_handler(state, handler, "") {
                state.input_buffer.push('@');
                state
                    .timeline
                    .push(TimelineLine::new("warning", error, true));
                state.status = "file picker failed".to_string();
            }
        }
        KeyCode::Char(value) => {
            state.input_buffer.push(value);
        }
        KeyCode::Enter => {
            let submitted = state.input_buffer.trim().to_string();
            state.input_buffer.clear();
            if matches!(submitted.as_str(), "/exit" | "/quit") {
                return Ok(true);
            }
            if submitted.is_empty() {
                return Ok(false);
            }
            state.remember_history(&submitted);
            if submitted.starts_with('/') {
                if let Some(kind) = choice_picker_command_kind(&submitted) {
                    open_choice_picker_from_command(state, handler, kind)?;
                    return Ok(false);
                }
                if handle_local_state_command(&submitted, state, handler)? {
                    return Ok(false);
                }
                if agent_picker_command_query(&submitted).is_some() {
                    open_agent_picker_from_handler(state, handler, "")?;
                    return Ok(false);
                }
                if model_picker_command_query(&submitted).is_some() {
                    open_model_picker_from_handler(state, handler, "")?;
                    return Ok(false);
                }
                if let Some(query) = session_picker_command_query(&submitted) {
                    open_session_picker_from_handler(state, handler, query)?;
                    return Ok(false);
                }
                if let Some(query) = file_picker_command_query(&submitted) {
                    open_file_picker_from_handler(state, handler, query)?;
                    return Ok(false);
                }
                match handler.handle_command(&submitted) {
                    Ok(lines) => {
                        state.status = "command completed".to_string();
                        apply_handler_output(state, handler, lines);
                    }
                    Err(error) => {
                        state.status = "command failed".to_string();
                        state.timeline.push(TimelineLine::new("error", error, true));
                    }
                }
            } else {
                state
                    .timeline
                    .push(TimelineLine::new("user", format!("> {submitted}"), true));
                state.status = "running".to_string();
                match handler.handle_submit(&submitted) {
                    Ok(lines) => {
                        state.status = "idle".to_string();
                        apply_handler_output(state, handler, lines);
                    }
                    Err(error) => {
                        state.status = "turn failed".to_string();
                        state.timeline.push(TimelineLine::new("error", error, true));
                    }
                }
            }
        }
        _ => {}
    }
    Ok(false)
}

fn handle_choice_picker_key<H: TerminalEventHandler>(
    key: KeyEvent,
    state: &mut TuiState,
    handler: &mut H,
) -> Result<(), String> {
    match key.code {
        KeyCode::Esc => {
            state.close_choice_picker();
        }
        KeyCode::Enter => {
            select_choice_picker_from_handler(state, handler)?;
        }
        KeyCode::Up => {
            state.choice_picker_previous();
        }
        KeyCode::Down | KeyCode::Tab => {
            state.choice_picker_next();
        }
        KeyCode::Backspace => {
            if let Some(picker) = state.choice_picker.as_mut() {
                picker.query.pop();
            }
            state.filter_choice_picker();
        }
        KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(picker) = state.choice_picker.as_mut() {
                picker.query.push(ch);
            }
            state.filter_choice_picker();
        }
        _ => {}
    }
    Ok(())
}

fn handle_agent_picker_key<H: TerminalEventHandler>(
    key: KeyEvent,
    state: &mut TuiState,
    handler: &mut H,
) -> Result<(), String> {
    match key.code {
        KeyCode::Esc => {
            state.close_agent_picker();
        }
        KeyCode::Enter => {
            select_agent_picker_from_handler(state, handler)?;
        }
        KeyCode::Up => {
            state.agent_picker_previous();
        }
        KeyCode::Down | KeyCode::Tab => {
            state.agent_picker_next();
        }
        KeyCode::Backspace => {
            if let Some(picker) = state.agent_picker.as_mut() {
                picker.query.pop();
            }
            state.filter_agent_picker();
        }
        KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(picker) = state.agent_picker.as_mut() {
                picker.query.push(ch);
            }
            state.filter_agent_picker();
        }
        _ => {}
    }
    Ok(())
}

fn handle_model_picker_key<H: TerminalEventHandler>(
    key: KeyEvent,
    state: &mut TuiState,
    handler: &mut H,
) -> Result<(), String> {
    match key.code {
        KeyCode::Esc => {
            state.close_model_picker();
        }
        KeyCode::Enter => {
            select_model_picker_from_handler(state, handler)?;
        }
        KeyCode::Up => {
            state.model_picker_previous();
        }
        KeyCode::Down | KeyCode::Tab => {
            state.model_picker_next();
        }
        KeyCode::Backspace => {
            if let Some(picker) = state.model_picker.as_mut() {
                picker.query.pop();
            }
            state.filter_model_picker();
        }
        KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(picker) = state.model_picker.as_mut() {
                picker.query.push(ch);
            }
            state.filter_model_picker();
        }
        _ => {}
    }
    Ok(())
}

fn handle_session_picker_key<H: TerminalEventHandler>(
    key: KeyEvent,
    state: &mut TuiState,
    handler: &mut H,
) -> Result<(), String> {
    match key.code {
        KeyCode::Esc => {
            state.close_session_picker();
        }
        KeyCode::Enter => {
            select_session_picker_from_handler(state, handler)?;
        }
        KeyCode::Up => {
            state.session_picker_previous();
        }
        KeyCode::Down | KeyCode::Tab => {
            state.session_picker_next();
        }
        KeyCode::Backspace => {
            if let Some(picker) = state.session_picker.as_mut() {
                picker.query.pop();
            }
            if let Err(error) = refresh_session_picker_from_handler(state, handler) {
                state
                    .timeline
                    .push(TimelineLine::new("warning", error, true));
                state.status = "session picker refresh failed".to_string();
            }
        }
        KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(picker) = state.session_picker.as_mut() {
                picker.query.push(ch);
            }
            if let Err(error) = refresh_session_picker_from_handler(state, handler) {
                state
                    .timeline
                    .push(TimelineLine::new("warning", error, true));
                state.status = "session picker refresh failed".to_string();
            }
        }
        _ => {}
    }
    Ok(())
}

fn handle_file_picker_key<H: TerminalEventHandler>(
    key: KeyEvent,
    state: &mut TuiState,
    handler: &mut H,
) -> Result<(), String> {
    match key.code {
        KeyCode::Esc => {
            state.close_file_picker();
        }
        KeyCode::Enter => {
            state.insert_selected_file_picker_reference();
        }
        KeyCode::Up => {
            state.file_picker_previous();
        }
        KeyCode::Down | KeyCode::Tab => {
            state.file_picker_next();
        }
        KeyCode::Backspace => {
            if let Some(picker) = state.file_picker.as_mut() {
                picker.query.pop();
            }
            if let Err(error) = refresh_file_picker_from_handler(state, handler) {
                state
                    .timeline
                    .push(TimelineLine::new("warning", error, true));
                state.status = "file picker refresh failed".to_string();
            }
        }
        KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(picker) = state.file_picker.as_mut() {
                picker.query.push(ch);
            }
            if let Err(error) = refresh_file_picker_from_handler(state, handler) {
                state
                    .timeline
                    .push(TimelineLine::new("warning", error, true));
                state.status = "file picker refresh failed".to_string();
            }
        }
        _ => {}
    }
    Ok(())
}

fn open_choice_picker_from_handler<H: TerminalEventHandler>(
    state: &mut TuiState,
    handler: &mut H,
    kind: ChoicePickerKind,
) -> Result<(), String> {
    let payload = handler.list_models()?;
    let choices = choice_picker_values_from_models(&payload, kind);
    state.open_choice_picker(kind, "", choices);
    Ok(())
}

fn open_choice_picker_from_command<H: TerminalEventHandler>(
    state: &mut TuiState,
    handler: &mut H,
    kind: ChoicePickerKind,
) -> Result<(), String> {
    match kind {
        ChoicePickerKind::Theme => {
            state.open_choice_picker(ChoicePickerKind::Theme, "", default_theme_names());
            Ok(())
        }
        ChoicePickerKind::Variant | ChoicePickerKind::Thinking => {
            open_choice_picker_from_handler(state, handler, kind)
        }
    }
}

fn select_choice_picker_from_handler<H: TerminalEventHandler>(
    state: &mut TuiState,
    handler: &mut H,
) -> Result<(), String> {
    let Some((kind, value)) = state.selected_choice_picker_value() else {
        state.status = "choice picker empty".to_string();
        return Ok(());
    };
    state.close_choice_picker();
    if kind == ChoicePickerKind::Theme {
        state.set_theme(&value);
        return Ok(());
    }
    let lines = handler.handle_command(&format!("/{} {value}", kind.command_name()))?;
    apply_handler_output(state, handler, lines);
    Ok(())
}

fn open_agent_picker_from_handler<H: TerminalEventHandler>(
    state: &mut TuiState,
    handler: &mut H,
    query: &str,
) -> Result<(), String> {
    let payload = handler.list_agents()?;
    let agents = payload
        .get("agents")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    state.open_agent_picker(query, agents);
    Ok(())
}

fn select_agent_picker_from_handler<H: TerminalEventHandler>(
    state: &mut TuiState,
    handler: &mut H,
) -> Result<(), String> {
    let Some(agent_id) = state.selected_agent_picker_id() else {
        state.status = "agent picker empty".to_string();
        return Ok(());
    };
    state.close_agent_picker();
    let lines = handler.handle_command(&format!("/agent {agent_id}"))?;
    apply_handler_output(state, handler, lines);
    Ok(())
}

fn open_model_picker_from_handler<H: TerminalEventHandler>(
    state: &mut TuiState,
    handler: &mut H,
    query: &str,
) -> Result<(), String> {
    let payload = handler.list_models()?;
    let models = payload
        .get("models")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    state.open_model_picker(query, models);
    Ok(())
}

fn select_model_picker_from_handler<H: TerminalEventHandler>(
    state: &mut TuiState,
    handler: &mut H,
) -> Result<(), String> {
    let Some(model_id) = state.selected_model_picker_id() else {
        state.status = "model picker empty".to_string();
        return Ok(());
    };
    state.close_model_picker();
    let lines = handler.handle_command(&format!("/models {model_id}"))?;
    apply_handler_output(state, handler, lines);
    Ok(())
}

fn open_session_picker_from_handler<H: TerminalEventHandler>(
    state: &mut TuiState,
    handler: &mut H,
    query: &str,
) -> Result<(), String> {
    let candidates = handler.search_sessions(query)?;
    state.open_session_picker(query, candidates);
    Ok(())
}

fn refresh_session_picker_from_handler<H: TerminalEventHandler>(
    state: &mut TuiState,
    handler: &mut H,
) -> Result<(), String> {
    let query = state
        .session_picker
        .as_ref()
        .map(|picker| picker.query.clone())
        .unwrap_or_default();
    let candidates = handler.search_sessions(&query)?;
    if let Some(picker) = state.session_picker.as_mut() {
        picker.candidates = candidates;
        picker.selected = picker
            .selected
            .min(picker.candidates.len().saturating_sub(1));
    }
    state.status = "session picker".to_string();
    Ok(())
}

fn select_session_picker_from_handler<H: TerminalEventHandler>(
    state: &mut TuiState,
    handler: &mut H,
) -> Result<(), String> {
    let Some(session_id) = state.selected_session_picker_id() else {
        state.status = "session picker empty".to_string();
        return Ok(());
    };
    state.close_session_picker();
    state.session_id = Some(session_id.clone());
    let lines = handler.handle_command(&format!("/resume {session_id}"))?;
    apply_handler_output(state, handler, lines);
    Ok(())
}

fn open_file_picker_from_handler<H: TerminalEventHandler>(
    state: &mut TuiState,
    handler: &mut H,
    query: &str,
) -> Result<(), String> {
    let candidates = handler.search_files(query)?;
    state.open_file_picker(query, candidates);
    Ok(())
}

fn refresh_file_picker_from_handler<H: TerminalEventHandler>(
    state: &mut TuiState,
    handler: &mut H,
) -> Result<(), String> {
    let query = state
        .file_picker
        .as_ref()
        .map(|picker| picker.query.clone())
        .unwrap_or_default();
    let candidates = handler.search_files(&query)?;
    if let Some(picker) = state.file_picker.as_mut() {
        picker.candidates = candidates;
        picker.selected = picker
            .selected
            .min(picker.candidates.len().saturating_sub(1));
    }
    state.status = "file picker".to_string();
    Ok(())
}

fn file_picker_command_query(command: &str) -> Option<&str> {
    if command == "/files" {
        return Some("");
    }
    command.strip_prefix("/files ").map(str::trim)
}

fn session_picker_command_query(command: &str) -> Option<&str> {
    if command == "/sessions" {
        return Some("");
    }
    command.strip_prefix("/sessions ").map(str::trim)
}

fn model_picker_command_query(command: &str) -> Option<&str> {
    (command == "/models").then_some("")
}

fn agent_picker_command_query(command: &str) -> Option<&str> {
    (command == "/agents").then_some("")
}

fn choice_picker_command_kind(command: &str) -> Option<ChoicePickerKind> {
    match command {
        "/theme" | "/themes" => Some(ChoicePickerKind::Theme),
        "/variant" => Some(ChoicePickerKind::Variant),
        "/thinking" => Some(ChoicePickerKind::Thinking),
        _ => None,
    }
}

fn handle_local_state_command<H: TerminalEventHandler>(
    submitted: &str,
    state: &mut TuiState,
    handler: &mut H,
) -> Result<bool, String> {
    if !is_local_state_command(submitted) {
        return Ok(false);
    }
    let approval_start = state.approval_responses.len();
    let question_start = state.question_responses.len();
    state.input_buffer = submitted.to_string();
    let remote_prompt = state.submit();
    if remote_prompt {
        return Ok(false);
    }
    let approval_responses: Vec<Value> = state.approval_responses[approval_start..].to_vec();
    let question_responses: Vec<Value> = state.question_responses[question_start..].to_vec();
    for response in approval_responses {
        let lines = handler.handle_approval_response(&response)?;
        apply_handler_output(state, handler, lines);
    }
    for response in question_responses {
        let lines = handler.handle_question_response(&response)?;
        apply_handler_output(state, handler, lines);
    }
    Ok(true)
}

fn dispatch_new_interaction_responses<H: TerminalEventHandler>(
    state: &mut TuiState,
    handler: &mut H,
    approval_start: usize,
    question_start: usize,
) -> Result<(), String> {
    let approval_responses: Vec<Value> = state.approval_responses[approval_start..].to_vec();
    let question_responses: Vec<Value> = state.question_responses[question_start..].to_vec();
    for response in approval_responses {
        let lines = handler.handle_approval_response(&response)?;
        apply_handler_output(state, handler, lines);
    }
    for response in question_responses {
        let lines = handler.handle_question_response(&response)?;
        apply_handler_output(state, handler, lines);
    }
    Ok(())
}

fn is_local_state_command(submitted: &str) -> bool {
    if matches!(submitted, "/help" | "/?" | "/") {
        return true;
    }
    let command = submitted
        .strip_prefix('/')
        .and_then(|value| value.split_whitespace().next())
        .unwrap_or_default();
    matches!(
        command,
        "allow"
            | "approve"
            | "deny"
            | "reject"
            | "answer"
            | "dismiss"
            | "stash"
            | "unstash"
            | "stashes"
            | "themes"
            | "theme"
            | "config"
            | "keybinds"
            | "usage"
            | "warnings"
            | "tool-details"
            | "editor"
            | "attach"
    )
}

fn apply_handler_output<H: TerminalEventHandler>(
    state: &mut TuiState,
    handler: &mut H,
    lines: Vec<TimelineLine>,
) {
    state.timeline.extend(lines);
    apply_app_event_values(state, handler.drain_app_events());
}

fn apply_app_event_values(state: &mut TuiState, events: Vec<Value>) {
    for event in events {
        let result = state.apply_app_event(&event);
        if !result
            .get("applied")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            let method = event
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or("event");
            state.timeline.push(TimelineLine::new(
                "status",
                format!(
                    "{method}: {}",
                    compact_json(&event.get("params").cloned().unwrap_or(Value::Null))
                ),
                false,
            ));
        }
    }
}

fn keybind_matches(config: &TuiConfig, key: &KeyEvent, action: &str, default: &str) -> bool {
    let binding = config
        .keybinds
        .get(action)
        .map(String::as_str)
        .unwrap_or(default);
    key_matches_binding(key, binding)
}

fn key_matches_binding(key: &KeyEvent, binding: &str) -> bool {
    let normalized = binding.trim().to_ascii_lowercase();
    if let Some(value) = normalized.strip_prefix("ctrl+") {
        return key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char(ch) if ch.to_ascii_lowercase().to_string() == value);
    }
    if let Some(value) = normalized.strip_prefix("leader+") {
        return matches!(key.code, KeyCode::Char(ch) if ch.to_ascii_lowercase().to_string() == value);
    }
    match normalized.as_str() {
        "esc" | "escape" => key.code == KeyCode::Esc,
        "enter" => key.code == KeyCode::Enter,
        "up" => key.code == KeyCode::Up,
        "down" => key.code == KeyCode::Down,
        value if value.len() == 1 => {
            matches!(key.code, KeyCode::Char(ch) if ch.to_ascii_lowercase().to_string() == value)
        }
        _ => false,
    }
}

fn handle_remote_control_request<H: TerminalEventHandler>(
    state: &mut TuiState,
    handler: &mut H,
    request: &Value,
) {
    let approval_start = state.approval_responses.len();
    let question_start = state.question_responses.len();
    let result = state.apply_control_request(request);
    let approval_responses: Vec<Value> = state.approval_responses[approval_start..].to_vec();
    let question_responses: Vec<Value> = state.question_responses[question_start..].to_vec();
    for response in approval_responses {
        match handler.handle_approval_response(&response) {
            Ok(lines) => apply_handler_output(state, handler, lines),
            Err(error) => state.timeline.push(TimelineLine::new("error", error, true)),
        }
    }
    for response in question_responses {
        match handler.handle_question_response(&response) {
            Ok(lines) => apply_handler_output(state, handler, lines),
            Err(error) => state.timeline.push(TimelineLine::new("error", error, true)),
        }
    }
    if let Some(command) = result
        .get("command")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
    {
        if let Some(kind) = choice_picker_command_kind(command) {
            if let Err(error) = open_choice_picker_from_command(state, handler, kind) {
                state.timeline.push(TimelineLine::new("error", error, true));
            }
        } else if agent_picker_command_query(command).is_some() {
            if let Err(error) = open_agent_picker_from_handler(state, handler, "") {
                state.timeline.push(TimelineLine::new("error", error, true));
            }
        } else if model_picker_command_query(command).is_some() {
            if let Err(error) = open_model_picker_from_handler(state, handler, "") {
                state.timeline.push(TimelineLine::new("error", error, true));
            }
        } else if let Some(query) = session_picker_command_query(command) {
            if let Err(error) = open_session_picker_from_handler(state, handler, query) {
                state.timeline.push(TimelineLine::new("error", error, true));
            }
        } else if let Some(query) = file_picker_command_query(command) {
            if let Err(error) = open_file_picker_from_handler(state, handler, query) {
                state.timeline.push(TimelineLine::new("error", error, true));
            }
        } else {
            match handler.handle_command(command) {
                Ok(lines) => apply_handler_output(state, handler, lines),
                Err(error) => state.timeline.push(TimelineLine::new("error", error, true)),
            }
        }
    }
    let payload = json!({
        "ok": result
            .get("applied")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        "request": request,
        "result": result,
    });
    if let Err(error) = handler.record_control_response(&payload) {
        state
            .timeline
            .push(TimelineLine::new("warning", error, true));
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TimelineLine {
    pub kind: String,
    pub text: String,
    pub important: bool,
}

impl TimelineLine {
    #[must_use]
    pub fn new(kind: impl Into<String>, text: impl Into<String>, important: bool) -> Self {
        Self {
            kind: kind.into(),
            text: text.into(),
            important,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InteractionFocus {
    Approval,
    Question,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InteractionDockState {
    pub focus: Option<InteractionFocus>,
    pub selected: usize,
    pub question_index: usize,
    pub question_answers: Vec<Vec<String>>,
    pub custom_answer: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TuiConfig {
    pub theme: String,
    pub keybinds: BTreeMap<String, String>,
    pub leader_key: String,
    pub mouse: bool,
    pub scroll: u16,
    pub diff_style: String,
    pub attention_notifications: bool,
    pub sounds: bool,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            theme: "default".to_string(),
            keybinds: BTreeMap::from([
                ("editor".to_string(), "ctrl+e".to_string()),
                ("stash".to_string(), "ctrl+s".to_string()),
                ("unstash".to_string(), "ctrl+y".to_string()),
            ]),
            leader_key: "\\".to_string(),
            mouse: true,
            scroll: 5,
            diff_style: "unified".to_string(),
            attention_notifications: true,
            sounds: false,
        }
    }
}

impl TuiConfig {
    #[must_use]
    pub fn load_from_workspace(workspace: &Path) -> Self {
        let mut config = Self::default();
        let Some(path) = find_tui_config_path(workspace) else {
            return config;
        };
        let Ok(raw) = fs::read_to_string(path) else {
            return config;
        };
        let parsed = serde_json::from_str::<Value>(&strip_jsonc_comments(&raw))
            .ok()
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({}));
        if let Some(theme) = parsed.get("theme").and_then(Value::as_str) {
            config.theme = theme.to_string();
        }
        if let Some(keybinds) = parsed.get("keybinds").and_then(Value::as_object) {
            for (key, value) in keybinds {
                if let Some(binding) = value.as_str().filter(|value| !value.trim().is_empty()) {
                    config
                        .keybinds
                        .insert(key.to_string(), binding.trim().to_ascii_lowercase());
                }
            }
        }
        if let Some(leader_key) = parsed.get("leader_key").and_then(Value::as_str) {
            config.leader_key = leader_key.to_string();
        }
        if let Some(mouse) = parsed.get("mouse").and_then(Value::as_bool) {
            config.mouse = mouse;
        }
        if let Some(scroll) = parsed.get("scroll").and_then(Value::as_u64) {
            config.scroll = scroll.min(100) as u16;
        }
        if let Some(diff_style) = parsed.get("diff_style").and_then(Value::as_str) {
            config.diff_style = diff_style.to_string();
        }
        if let Some(enabled) = parsed
            .get("attention_notifications")
            .or_else(|| parsed.get("notifications"))
            .and_then(Value::as_bool)
        {
            config.attention_notifications = enabled;
        }
        if let Some(sounds) = parsed.get("sounds").and_then(Value::as_bool) {
            config.sounds = sounds;
        }
        config
    }
}

fn find_tui_config_path(workspace: &Path) -> Option<PathBuf> {
    if let Ok(path) = std::env::var("OPENAGENT_TUI_CONFIG")
        && !path.trim().is_empty()
    {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    [
        workspace.join(".openagent/tui.jsonc"),
        workspace.join(".openagent/tui.json"),
        workspace.join("tui.jsonc"),
        workspace.join("tui.json"),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

fn strip_jsonc_comments(raw: &str) -> String {
    let mut output = String::new();
    let mut chars = raw.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    while let Some(ch) = chars.next() {
        if in_string {
            output.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        if ch == '"' {
            in_string = true;
            output.push(ch);
            continue;
        }
        if ch == '/' && chars.peek() == Some(&'/') {
            chars.next();
            for next in chars.by_ref() {
                if next == '\n' {
                    output.push('\n');
                    break;
                }
            }
            continue;
        }
        if ch == '/' && chars.peek() == Some(&'*') {
            chars.next();
            let mut previous = '\0';
            for next in chars.by_ref() {
                if previous == '*' && next == '/' {
                    break;
                }
                previous = next;
            }
            continue;
        }
        output.push(ch);
    }
    output
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TuiState {
    pub input_buffer: String,
    pub status: String,
    pub timeline: Vec<TimelineLine>,
    pub session_id: Option<String>,
    pub current_turn_id: Option<String>,
    pub active_approval: Option<Value>,
    pub active_question: Option<Value>,
    pub interaction: InteractionDockState,
    pub approval_responses: Vec<Value>,
    pub question_responses: Vec<Value>,
    pub history: Vec<String>,
    pub history_cursor: Option<usize>,
    pub stash: Vec<String>,
    pub config: TuiConfig,
    pub show_tool_details: bool,
    pub runtime_warnings: Vec<String>,
    pub usage_totals: Value,
    pub file_picker: Option<FilePickerState>,
    pub session_picker: Option<SessionPickerState>,
    pub model_picker: Option<ModelPickerState>,
    pub agent_picker: Option<AgentPickerState>,
    pub choice_picker: Option<ChoicePickerState>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChoicePickerKind {
    Theme,
    Variant,
    Thinking,
}

impl ChoicePickerKind {
    fn command_name(self) -> &'static str {
        match self {
            Self::Theme => "theme",
            Self::Variant => "variant",
            Self::Thinking => "thinking",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Theme => "Themes",
            Self::Variant => "Variants",
            Self::Thinking => "Thinking",
        }
    }

    fn item_label(self) -> &'static str {
        match self {
            Self::Theme => "themes",
            Self::Variant => "variants",
            Self::Thinking => "thinking levels",
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FilePickerState {
    pub query: String,
    pub selected: usize,
    pub candidates: Vec<ComposerFileCandidate>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SessionPickerState {
    pub query: String,
    pub selected: usize,
    pub candidates: Vec<Value>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelPickerState {
    pub query: String,
    pub selected: usize,
    pub models: Vec<Value>,
    pub candidates: Vec<Value>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgentPickerState {
    pub query: String,
    pub selected: usize,
    pub agents: Vec<Value>,
    pub candidates: Vec<Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChoicePickerState {
    pub kind: ChoicePickerKind,
    pub query: String,
    pub selected: usize,
    pub choices: Vec<String>,
    pub candidates: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComposerFileCandidate {
    pub reference: String,
    pub kind: String,
}

impl TuiState {
    #[must_use]
    pub fn new() -> Self {
        Self {
            input_buffer: String::new(),
            status: "idle".to_string(),
            timeline: Vec::new(),
            session_id: None,
            current_turn_id: None,
            active_approval: None,
            active_question: None,
            interaction: InteractionDockState::default(),
            approval_responses: Vec::new(),
            question_responses: Vec::new(),
            history: Vec::new(),
            history_cursor: None,
            stash: Vec::new(),
            config: TuiConfig::load_from_workspace(
                &std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            ),
            show_tool_details: false,
            runtime_warnings: Vec::new(),
            usage_totals: usage_totals_value(0, 0, 0, 0.0),
            file_picker: None,
            session_picker: None,
            model_picker: None,
            agent_picker: None,
            choice_picker: None,
        }
    }

    #[must_use]
    pub fn with_config(config: TuiConfig) -> Self {
        Self {
            config,
            ..Self::new()
        }
    }

    fn active_interaction_focus(&self) -> Option<InteractionFocus> {
        match self.interaction.focus {
            Some(InteractionFocus::Approval) if self.active_approval.is_some() => {
                Some(InteractionFocus::Approval)
            }
            Some(InteractionFocus::Question) if self.active_question.is_some() => {
                Some(InteractionFocus::Question)
            }
            _ if self.active_approval.is_some() => Some(InteractionFocus::Approval),
            _ if self.active_question.is_some() => Some(InteractionFocus::Question),
            _ => None,
        }
    }

    fn focus_approval_interaction(&mut self) {
        self.interaction = InteractionDockState {
            focus: Some(InteractionFocus::Approval),
            selected: 0,
            ..InteractionDockState::default()
        };
    }

    fn focus_question_interaction(&mut self, question: &Value) {
        let count = question_items(question).len().max(1);
        self.interaction = InteractionDockState {
            focus: Some(InteractionFocus::Question),
            selected: 0,
            question_index: 0,
            question_answers: vec![Vec::new(); count],
            custom_answer: String::new(),
        };
    }

    fn clear_interaction(&mut self, focus: InteractionFocus) {
        if self.interaction.focus == Some(focus) {
            self.interaction = InteractionDockState::default();
        }
    }

    fn handle_interaction_key(&mut self, key: &KeyEvent) -> bool {
        match self.active_interaction_focus() {
            Some(InteractionFocus::Approval) => self.handle_approval_key(key),
            Some(InteractionFocus::Question) => self.handle_question_key(key),
            None => false,
        }
    }

    fn handle_approval_key(&mut self, key: &KeyEvent) -> bool {
        let option_count = 3;
        match key.code {
            KeyCode::Up => {
                self.interaction.selected = self
                    .interaction
                    .selected
                    .saturating_sub(1)
                    .min(option_count - 1);
                true
            }
            KeyCode::Down => {
                self.interaction.selected = (self.interaction.selected + 1).min(option_count - 1);
                true
            }
            KeyCode::Char(ch) if ch.is_ascii_digit() => {
                let index = ch.to_digit(10).unwrap_or_default().saturating_sub(1) as usize;
                if index < option_count {
                    self.interaction.selected = index;
                    self.submit_selected_approval();
                }
                true
            }
            KeyCode::Enter => {
                self.submit_selected_approval();
                true
            }
            KeyCode::Esc => {
                self.respond_active_approval("deny", None, Some("dismissed from TUI"));
                true
            }
            _ => true,
        }
    }

    fn submit_selected_approval(&mut self) {
        match self.interaction.selected {
            0 => {
                self.respond_active_approval("allow", Some("once"), None);
            }
            1 => {
                self.respond_active_approval("allow", Some("always"), None);
            }
            _ => {
                self.respond_active_approval("deny", None, Some("denied from TUI"));
            }
        }
    }

    fn handle_question_key(&mut self, key: &KeyEvent) -> bool {
        let Some(question) = self.active_question.clone() else {
            return false;
        };
        let items = question_items(&question);
        if items.is_empty() {
            if key.code == KeyCode::Esc {
                self.dismiss_active_question(Some("dismissed from TUI"));
            }
            return true;
        }
        self.interaction.question_index = self.interaction.question_index.min(items.len() - 1);
        let current = &items[self.interaction.question_index];
        let options = question_option_labels(current);
        self.interaction.selected = self
            .interaction
            .selected
            .min(options.len().saturating_sub(1));
        match key.code {
            KeyCode::Esc => {
                self.dismiss_active_question(Some("dismissed from TUI"));
                true
            }
            KeyCode::Left => {
                self.interaction.question_index = self.interaction.question_index.saturating_sub(1);
                self.interaction.selected = 0;
                self.interaction.custom_answer.clear();
                true
            }
            KeyCode::Right | KeyCode::Tab => {
                self.advance_question_or_submit();
                true
            }
            KeyCode::Up => {
                self.interaction.selected = self.interaction.selected.saturating_sub(1);
                true
            }
            KeyCode::Down => {
                if !options.is_empty() {
                    self.interaction.selected =
                        (self.interaction.selected + 1).min(options.len() - 1);
                }
                true
            }
            KeyCode::Backspace => {
                self.interaction.custom_answer.pop();
                true
            }
            KeyCode::Char(ch) if ch.is_ascii_digit() && !options.is_empty() => {
                let index = ch.to_digit(10).unwrap_or_default().saturating_sub(1) as usize;
                if let Some(answer) = options.get(index) {
                    self.answer_current_question(answer.clone());
                }
                true
            }
            KeyCode::Char(ch) => {
                self.interaction.custom_answer.push(ch);
                self.status = "editing answer".to_string();
                true
            }
            KeyCode::Enter => {
                if !self.interaction.custom_answer.trim().is_empty() {
                    self.answer_current_question(self.interaction.custom_answer.trim().to_string());
                } else if let Some(answer) = options.get(self.interaction.selected) {
                    self.answer_current_question(answer.clone());
                } else {
                    self.status = "question needs answer".to_string();
                }
                true
            }
            _ => true,
        }
    }

    fn answer_current_question(&mut self, answer: String) {
        let Some(question) = self.active_question.as_ref() else {
            return;
        };
        let items = question_items(question);
        if items.is_empty() {
            return;
        }
        let index = self.interaction.question_index.min(items.len() - 1);
        if self.interaction.question_answers.len() < items.len() {
            self.interaction
                .question_answers
                .resize_with(items.len(), Vec::new);
        }
        self.interaction.question_answers[index] = vec![answer];
        self.interaction.custom_answer.clear();
        if index + 1 < items.len() {
            self.interaction.question_index = index + 1;
            self.interaction.selected = 0;
            self.status = "question next".to_string();
            return;
        }
        let answers = self.interaction.question_answers.clone();
        self.answer_active_question(answers, None);
    }

    fn advance_question_or_submit(&mut self) {
        let Some(question) = self.active_question.as_ref() else {
            return;
        };
        let items = question_items(question);
        if items.is_empty() {
            return;
        }
        if self.interaction.question_index + 1 < items.len() {
            self.interaction.question_index += 1;
            self.interaction.selected = 0;
            self.interaction.custom_answer.clear();
            return;
        }
        if self
            .interaction
            .question_answers
            .iter()
            .take(items.len())
            .all(|answers| !answers.is_empty())
        {
            self.answer_active_question(self.interaction.question_answers.clone(), None);
        } else {
            self.status = "question needs answer".to_string();
        }
    }

    pub fn remember_history(&mut self, value: &str) {
        let value = value.trim();
        if value.is_empty() {
            return;
        }
        if self.history.last().is_some_and(|last| last == value) {
            self.history_cursor = None;
            return;
        }
        self.history.push(value.to_string());
        if self.history.len() > 100 {
            self.history.remove(0);
        }
        self.history_cursor = None;
    }

    pub fn history_previous(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let index = self.history_cursor.map_or_else(
            || self.history.len().saturating_sub(1),
            |index| index.saturating_sub(1),
        );
        self.history_cursor = Some(index);
        self.input_buffer = self.history[index].clone();
        self.status = "history".to_string();
    }

    pub fn history_next(&mut self) {
        let Some(index) = self.history_cursor else {
            return;
        };
        if index + 1 >= self.history.len() {
            self.history_cursor = None;
            self.input_buffer.clear();
        } else {
            let next = index + 1;
            self.history_cursor = Some(next);
            self.input_buffer = self.history[next].clone();
        }
        self.status = "history".to_string();
    }

    pub fn stash_current_input(&mut self) {
        let value = self.input_buffer.trim().to_string();
        if value.is_empty() {
            self.timeline
                .push(TimelineLine::new("warning", "nothing to stash", true));
            self.status = "stash empty".to_string();
            return;
        }
        self.stash.push(value);
        if self.stash.len() > 20 {
            self.stash.remove(0);
        }
        self.input_buffer.clear();
        self.timeline.push(TimelineLine::new(
            "status",
            format!("stashed draft: {} item(s)", self.stash.len()),
            true,
        ));
        self.status = "stashed".to_string();
    }

    pub fn restore_latest_stash(&mut self) {
        let Some(value) = self.stash.pop() else {
            self.timeline
                .push(TimelineLine::new("warning", "stash is empty", true));
            self.status = "stash empty".to_string();
            return;
        };
        self.input_buffer = value;
        self.status = "stash restored".to_string();
    }

    pub fn insert_attachment_reference(&mut self, reference: &str) {
        let Some(token) = normalize_attachment_reference_token(reference) else {
            self.timeline.push(TimelineLine::new(
                "warning",
                "usage: /attach <path[:range]>",
                true,
            ));
            self.status = "attach invalid".to_string();
            return;
        };
        if !self.input_buffer.is_empty()
            && !self
                .input_buffer
                .chars()
                .last()
                .is_some_and(char::is_whitespace)
        {
            self.input_buffer.push(' ');
        }
        self.input_buffer.push_str(&token);
        self.input_buffer.push(' ');
        self.timeline.push(TimelineLine::new(
            "status",
            format!("attached reference inserted: {token}"),
            true,
        ));
        self.status = "attachment inserted".to_string();
    }

    pub fn set_theme(&mut self, theme: &str) {
        self.config.theme = theme.to_string();
        self.timeline.push(TimelineLine::new(
            "status",
            format!("theme set to {}", self.config.theme),
            true,
        ));
        self.status = "theme updated".to_string();
    }

    pub fn open_choice_picker(
        &mut self,
        kind: ChoicePickerKind,
        query: &str,
        choices: Vec<String>,
    ) {
        self.file_picker = None;
        self.session_picker = None;
        self.model_picker = None;
        self.agent_picker = None;
        let query = query.trim().to_string();
        let candidates = filter_choice_picker_values(&choices, &query);
        self.choice_picker = Some(ChoicePickerState {
            kind,
            query,
            selected: 0,
            choices,
            candidates,
        });
        self.status = format!("{} picker", kind.command_name());
    }

    pub fn close_choice_picker(&mut self) {
        self.choice_picker = None;
        self.status = "choice picker closed".to_string();
    }

    pub fn filter_choice_picker(&mut self) {
        let Some(picker) = self.choice_picker.as_mut() else {
            return;
        };
        picker.candidates = filter_choice_picker_values(&picker.choices, &picker.query);
        picker.selected = picker
            .selected
            .min(picker.candidates.len().saturating_sub(1));
        self.status = format!("{} picker", picker.kind.command_name());
    }

    pub fn choice_picker_previous(&mut self) {
        let Some(picker) = self.choice_picker.as_mut() else {
            return;
        };
        picker.selected = picker.selected.saturating_sub(1);
        self.status = format!("{} picker", picker.kind.command_name());
    }

    pub fn choice_picker_next(&mut self) {
        let Some(picker) = self.choice_picker.as_mut() else {
            return;
        };
        if !picker.candidates.is_empty() {
            picker.selected = (picker.selected + 1).min(picker.candidates.len() - 1);
        }
        self.status = format!("{} picker", picker.kind.command_name());
    }

    pub fn selected_choice_picker_value(&self) -> Option<(ChoicePickerKind, String)> {
        self.choice_picker.as_ref().and_then(|picker| {
            picker
                .candidates
                .get(picker.selected)
                .filter(|value| !value.is_empty())
                .map(|value| (picker.kind, value.clone()))
        })
    }

    pub fn open_agent_picker(&mut self, query: &str, agents: Vec<Value>) {
        self.file_picker = None;
        self.session_picker = None;
        self.model_picker = None;
        self.choice_picker = None;
        let query = query.trim().to_string();
        let candidates = filter_agents_for_picker(&agents, &query);
        self.agent_picker = Some(AgentPickerState {
            query,
            selected: 0,
            agents,
            candidates,
        });
        self.status = "agent picker".to_string();
    }

    pub fn close_agent_picker(&mut self) {
        self.agent_picker = None;
        self.status = "agent picker closed".to_string();
    }

    pub fn filter_agent_picker(&mut self) {
        let Some(picker) = self.agent_picker.as_mut() else {
            return;
        };
        picker.candidates = filter_agents_for_picker(&picker.agents, &picker.query);
        picker.selected = picker
            .selected
            .min(picker.candidates.len().saturating_sub(1));
        self.status = "agent picker".to_string();
    }

    pub fn agent_picker_previous(&mut self) {
        let Some(picker) = self.agent_picker.as_mut() else {
            return;
        };
        picker.selected = picker.selected.saturating_sub(1);
        self.status = "agent picker".to_string();
    }

    pub fn agent_picker_next(&mut self) {
        let Some(picker) = self.agent_picker.as_mut() else {
            return;
        };
        if !picker.candidates.is_empty() {
            picker.selected = (picker.selected + 1).min(picker.candidates.len() - 1);
        }
        self.status = "agent picker".to_string();
    }

    pub fn selected_agent_picker_id(&self) -> Option<String> {
        self.agent_picker
            .as_ref()
            .and_then(|picker| picker.candidates.get(picker.selected))
            .and_then(|agent| agent.get("id").and_then(Value::as_str))
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
    }

    pub fn open_model_picker(&mut self, query: &str, models: Vec<Value>) {
        self.file_picker = None;
        self.session_picker = None;
        self.agent_picker = None;
        self.choice_picker = None;
        let query = query.trim().to_string();
        let candidates = filter_models_for_picker(&models, &query);
        self.model_picker = Some(ModelPickerState {
            query,
            selected: 0,
            models,
            candidates,
        });
        self.status = "model picker".to_string();
    }

    pub fn close_model_picker(&mut self) {
        self.model_picker = None;
        self.status = "model picker closed".to_string();
    }

    pub fn filter_model_picker(&mut self) {
        let Some(picker) = self.model_picker.as_mut() else {
            return;
        };
        picker.candidates = filter_models_for_picker(&picker.models, &picker.query);
        picker.selected = picker
            .selected
            .min(picker.candidates.len().saturating_sub(1));
        self.status = "model picker".to_string();
    }

    pub fn model_picker_previous(&mut self) {
        let Some(picker) = self.model_picker.as_mut() else {
            return;
        };
        picker.selected = picker.selected.saturating_sub(1);
        self.status = "model picker".to_string();
    }

    pub fn model_picker_next(&mut self) {
        let Some(picker) = self.model_picker.as_mut() else {
            return;
        };
        if !picker.candidates.is_empty() {
            picker.selected = (picker.selected + 1).min(picker.candidates.len() - 1);
        }
        self.status = "model picker".to_string();
    }

    pub fn selected_model_picker_id(&self) -> Option<String> {
        self.model_picker
            .as_ref()
            .and_then(|picker| picker.candidates.get(picker.selected))
            .and_then(|model| model.get("id").and_then(Value::as_str))
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
    }

    pub fn open_session_picker(&mut self, query: &str, candidates: Vec<Value>) {
        self.model_picker = None;
        self.agent_picker = None;
        self.choice_picker = None;
        self.file_picker = None;
        self.session_picker = Some(SessionPickerState {
            query: query.trim().to_string(),
            selected: 0,
            candidates,
        });
        self.status = "session picker".to_string();
    }

    pub fn close_session_picker(&mut self) {
        self.session_picker = None;
        self.status = "session picker closed".to_string();
    }

    pub fn session_picker_previous(&mut self) {
        let Some(picker) = self.session_picker.as_mut() else {
            return;
        };
        picker.selected = picker.selected.saturating_sub(1);
        self.status = "session picker".to_string();
    }

    pub fn session_picker_next(&mut self) {
        let Some(picker) = self.session_picker.as_mut() else {
            return;
        };
        if !picker.candidates.is_empty() {
            picker.selected = (picker.selected + 1).min(picker.candidates.len() - 1);
        }
        self.status = "session picker".to_string();
    }

    pub fn selected_session_picker_id(&self) -> Option<String> {
        self.session_picker
            .as_ref()
            .and_then(|picker| picker.candidates.get(picker.selected))
            .and_then(session_id_from_payload)
    }

    pub fn open_file_picker(&mut self, query: &str, candidates: Vec<ComposerFileCandidate>) {
        self.model_picker = None;
        self.agent_picker = None;
        self.choice_picker = None;
        self.session_picker = None;
        self.file_picker = Some(FilePickerState {
            query: query.trim().to_string(),
            selected: 0,
            candidates,
        });
        self.status = "file picker".to_string();
    }

    pub fn close_file_picker(&mut self) {
        self.file_picker = None;
        self.status = "file picker closed".to_string();
    }

    pub fn file_picker_previous(&mut self) {
        let Some(picker) = self.file_picker.as_mut() else {
            return;
        };
        picker.selected = picker.selected.saturating_sub(1);
        self.status = "file picker".to_string();
    }

    pub fn file_picker_next(&mut self) {
        let Some(picker) = self.file_picker.as_mut() else {
            return;
        };
        if !picker.candidates.is_empty() {
            picker.selected = (picker.selected + 1).min(picker.candidates.len() - 1);
        }
        self.status = "file picker".to_string();
    }

    pub fn insert_selected_file_picker_reference(&mut self) {
        let Some(reference) = self.file_picker.as_ref().and_then(|picker| {
            picker
                .candidates
                .get(picker.selected)
                .map(|candidate| candidate.reference.clone())
        }) else {
            self.status = "file picker empty".to_string();
            return;
        };
        self.file_picker = None;
        self.insert_attachment_reference(&reference);
    }

    pub fn edit_input_with_external_editor(&mut self) -> Result<(), String> {
        let editor = std::env::var("VISUAL")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                std::env::var("EDITOR")
                    .ok()
                    .filter(|value| !value.trim().is_empty())
            })
            .unwrap_or_else(|| "vi".to_string());
        let path = std::env::temp_dir().join(format!(
            "openagent-tui-editor-{}-{}.md",
            std::process::id(),
            self.history.len()
        ));
        fs::write(&path, &self.input_buffer).map_err(|error| error.to_string())?;
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        let status = Command::new(&editor)
            .arg(&path)
            .status()
            .map_err(|error| format!("failed to launch editor `{editor}`: {error}"));
        let _ = execute!(io::stdout(), EnterAlternateScreen);
        let _ = enable_raw_mode();
        status.and_then(|status| {
            if status.success() {
                Ok(())
            } else {
                Err(format!("editor exited with status {status}"))
            }
        })?;
        self.input_buffer = fs::read_to_string(&path).map_err(|error| error.to_string())?;
        let _ = fs::remove_file(&path);
        self.status = "editor updated prompt".to_string();
        Ok(())
    }

    fn merge_usage(&mut self, usage: &Value) {
        let input = self.usage_totals["input_tokens"]
            .as_u64()
            .unwrap_or_default()
            + usage
                .get("input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or_default();
        let output = self.usage_totals["output_tokens"]
            .as_u64()
            .unwrap_or_default()
            + usage
                .get("output_tokens")
                .and_then(Value::as_u64)
                .unwrap_or_default();
        let total = self.usage_totals["total_tokens"]
            .as_u64()
            .unwrap_or_default()
            + usage
                .get("total_tokens")
                .and_then(Value::as_u64)
                .unwrap_or_default();
        let cost = self.usage_totals["cost"].as_f64().unwrap_or_default()
            + usage
                .get("cost")
                .and_then(Value::as_f64)
                .unwrap_or_default();
        self.usage_totals = usage_totals_value(input, output, total, cost);
    }

    pub fn apply_control_request(&mut self, request: &Value) -> Value {
        let path = request
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mut action;
        let mut params;
        if !path.is_empty() {
            action = normalize_control_action(path.trim_start_matches("/tui/").trim_matches('/'));
            params = object_value(request.get("body"));
            if action == "publish" {
                (action, params) = control_publish_to_action(&params);
            }
        } else {
            action = request
                .get("action")
                .or_else(|| request.get("type"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            params = object_value(request.get("params"));
        }
        action = normalize_control_action(&action);

        match action.as_str() {
            "prompt.append" => {
                let text = params
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                self.input_buffer.push_str(text);
                self.status = "prompt updated".to_string();
                json!({"applied": true, "action": action})
            }
            "prompt.submit" => {
                let submitted = self.submit();
                json!({"applied": submitted, "action": action})
            }
            "prompt.clear" => {
                self.input_buffer.clear();
                self.status = "prompt cleared".to_string();
                json!({"applied": true, "action": action})
            }
            "help.open" => {
                self.show_help();
                json!({"applied": true, "action": action})
            }
            "sessions.open" => {
                let query = control_string_field(&params, &["query", "text", "value"]);
                let command = if query.is_empty() {
                    "/sessions".to_string()
                } else {
                    format!("/sessions {query}")
                };
                self.timeline.push(TimelineLine::new(
                    "status",
                    format!("queued session picker: {command}"),
                    true,
                ));
                self.status = "session picker queued".to_string();
                json!({"applied": true, "action": action, "command": command})
            }
            "session.select" => {
                let session_id = params
                    .get("sessionID")
                    .or_else(|| params.get("session_id"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if session_id.is_empty() {
                    self.timeline.push(TimelineLine::new(
                        "error",
                        "control request missing sessionID",
                        true,
                    ));
                    self.status = "control invalid".to_string();
                    return json!({"applied": false, "action": action, "error": "sessionID is required"});
                }
                self.session_id = Some(session_id.to_string());
                self.input_buffer.clear();
                self.timeline.clear();
                self.timeline.push(TimelineLine::new(
                    "status",
                    format!("resumed session: {session_id}"),
                    true,
                ));
                self.status = "session resumed".to_string();
                json!({"applied": true, "action": action, "command": format!("/resume {session_id}")})
            }
            "session.rename" => self.session_command_control(&params, &action, "rename"),
            "session.archive" => {
                self.session_literal_command_control(&action, "/archive".to_string())
            }
            "session.unarchive" => {
                self.session_literal_command_control(&action, "/unarchive".to_string())
            }
            "session.delete" => {
                self.session_literal_command_control(&action, "/delete".to_string())
            }
            "session.fork" => self.session_literal_command_control(&action, "/fork".to_string()),
            "session.children" => {
                self.session_literal_command_control(&action, "/children".to_string())
            }
            "session.parent" => {
                self.session_literal_command_control(&action, "/parent".to_string())
            }
            "session.share" => self.session_literal_command_control(&action, "/share".to_string()),
            "session.unshare" => {
                self.session_literal_command_control(&action, "/unshare".to_string())
            }
            "session.compact" => {
                self.session_literal_command_control(&action, "/compact".to_string())
            }
            "session.details" => {
                self.session_literal_command_control(&action, "/details".to_string())
            }
            "session.undo" => self.session_literal_command_control(&action, "/undo".to_string()),
            "session.redo" => self.session_literal_command_control(&action, "/redo".to_string()),
            "toast.show" => {
                let message = params
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if message.is_empty() {
                    self.status = "control invalid".to_string();
                    return json!({"applied": false, "action": action, "error": "message is required"});
                }
                let title = params
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("toast");
                let variant = params
                    .get("variant")
                    .and_then(Value::as_str)
                    .unwrap_or("status")
                    .to_ascii_lowercase();
                let kind = if matches!(variant.as_str(), "error" | "danger") {
                    "error"
                } else if matches!(variant.as_str(), "warn" | "warning") {
                    "warning"
                } else {
                    "status"
                };
                self.timeline
                    .push(TimelineLine::new(kind, format!("{title}: {message}"), true));
                self.status = title.to_string();
                json!({"applied": true, "action": action})
            }
            "command.execute" => {
                let command = params
                    .get("command")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if command.is_empty() {
                    self.status = "control invalid".to_string();
                    return json!({"applied": false, "action": action, "error": "command is required"});
                }
                self.input_buffer = if command.starts_with('/') {
                    command.to_string()
                } else {
                    format!("/{command}")
                };
                if is_local_state_command(&self.input_buffer)
                    || matches!(self.input_buffer.as_str(), "/help" | "/?" | "/")
                {
                    let submitted = self.submit();
                    json!({"applied": submitted, "action": action})
                } else {
                    let command = self.input_buffer.clone();
                    self.input_buffer.clear();
                    self.timeline.push(TimelineLine::new(
                        "status",
                        format!("queued command: {command}"),
                        true,
                    ));
                    self.status = "command queued".to_string();
                    json!({"applied": true, "action": action, "command": command})
                }
            }
            "approval.respond" => self.respond_active_approval_from_params(&params, &action),
            "question.reply" => self.answer_active_question_from_params(&params, &action),
            "question.dismiss" => self.dismiss_active_question_from_params(&params, &action),
            "model.open" => self.open_model_control(&params, &action),
            "model.select" | "model.set" => self.select_model_control(&params, &action),
            "agent.open" => self.open_agent_control(&params, &action),
            "agent.select" | "agent.set" => self.select_agent_control(&params, &action),
            "variant.open" => self.open_variant_control(&params, &action),
            "variant.select" | "variant.set" => {
                self.select_named_session_setting_control(&params, &action, "variant", "variant")
            }
            "thinking.open" => self.open_thinking_control(&params, &action),
            "thinking.select" | "thinking.set" => {
                self.select_named_session_setting_control(&params, &action, "thinking", "level")
            }
            "theme.open" => self.open_theme_control(&params, &action),
            "theme.select" | "theme.set" => self.select_theme_control(&params, &action),
            "palette.open" => self.open_palette_control(&params, &action),
            "palette.execute" => self.execute_palette_control(&params, &action),
            "file.open" => self.open_file_control(&params, &action),
            "file.select" | "file.attach" => self.select_file_control(&params, &action),
            _ => {
                self.timeline.push(TimelineLine::new(
                    "warning",
                    format!(
                        "unknown TUI control: {}",
                        if action.is_empty() { "-" } else { &action }
                    ),
                    true,
                ));
                self.status = "control unknown".to_string();
                json!({"applied": false, "action": action, "unsupported": true})
            }
        }
    }

    pub fn submit(&mut self) -> bool {
        let raw_text = self.input_buffer.trim().to_string();
        if raw_text == "/help" || raw_text == "/?" || raw_text == "/" {
            self.show_help();
            self.input_buffer.clear();
            return false;
        }
        if self.handle_interaction_command(&raw_text) {
            if !raw_text.strip_prefix('/').is_some_and(|command| {
                command
                    .split_whitespace()
                    .next()
                    .is_some_and(|name| matches!(name, "unstash" | "editor" | "attach"))
            }) {
                self.input_buffer.clear();
            }
            return false;
        }
        if raw_text.is_empty() {
            return false;
        }
        self.input_buffer.clear();
        self.status = "running".to_string();
        self.timeline
            .push(TimelineLine::new("user", format!("> {raw_text}"), true));
        true
    }

    pub fn apply_app_event(&mut self, event: &Value) -> Value {
        let method = event
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match method {
            "turn/started" => self.apply_turn_started(event),
            "turn/approval_requested" => self.apply_approval_requested(event),
            "turn/approval_resolved" => self.apply_approval_resolved(event),
            "item/toolCall/started" => self.apply_tool_started(event),
            "item/toolCall/completed" => self.apply_tool_finished(event, false),
            "item/toolCall/failed" => self.apply_tool_finished(event, true),
            "item/agentMessage/started" => self.apply_agent_message_started(event),
            "item/agentMessage/delta" => self.apply_agent_message_delta(event),
            "item/agentMessage/completed" => self.apply_agent_message_completed(event),
            "item/question/requested" => self.apply_question_requested(event),
            "item/question/resolved" => self.apply_question_resolved(event),
            "item/reasoning/started" | "item/reasoning/delta" | "item/reasoning/completed" => {
                self.apply_reasoning_event(event)
            }
            "patch/detected" => self.apply_patch_event(event, "patch detected"),
            "patch/undone" => self.apply_patch_event(event, "patch undone"),
            "patch/redone" => self.apply_patch_event(event, "patch redone"),
            "turn/completed" => self.apply_turn_completed(event),
            "turn/failed" => self.apply_turn_failed(event, false),
            "turn/interrupted" => self.apply_turn_failed(event, true),
            "runtime/warning" | "warning" => self.apply_runtime_warning(event),
            _ => json!({"applied": false, "method": method, "unsupported": true}),
        }
    }

    pub fn respond_active_approval(
        &mut self,
        action: &str,
        scope: Option<&str>,
        note: Option<&str>,
    ) -> Value {
        let Some(approval) = self.active_approval.clone() else {
            self.status = "approval missing".to_string();
            return json!({"applied": false, "error": "no active approval"});
        };
        let mut payload = Map::new();
        payload.insert("action".to_string(), Value::String(action.to_string()));
        if let Some(scope) = scope.filter(|value| !value.trim().is_empty()) {
            payload.insert("scope".to_string(), Value::String(scope.trim().to_string()));
        }
        if let Some(note) = note.filter(|value| !value.trim().is_empty()) {
            payload.insert("note".to_string(), Value::String(note.trim().to_string()));
        }
        let Ok(mut response) = approval_response_payload(&Value::Object(payload)) else {
            self.status = "approval invalid".to_string();
            return json!({"applied": false, "error": "invalid approval response"});
        };
        merge_identity_fields(
            &mut response,
            &approval,
            &["request_id", "turn_id", "session_id", "tool_name"],
        );
        self.active_approval = None;
        self.clear_interaction(InteractionFocus::Approval);
        self.status = "approval resolved".to_string();
        self.timeline.push(TimelineLine::new(
            "status",
            approval_response_summary(&response),
            true,
        ));
        self.approval_responses.push(response.clone());
        json!({"applied": true, "action": "approval.respond", "payload": response})
    }

    pub fn answer_active_question(
        &mut self,
        answers: Vec<Vec<String>>,
        note: Option<&str>,
    ) -> Value {
        let Some(question) = self.active_question.clone() else {
            self.status = "question missing".to_string();
            return json!({"applied": false, "error": "no active question"});
        };
        let mut payload = Map::new();
        payload.insert("answers".to_string(), json!(answers));
        if let Some(note) = note.filter(|value| !value.trim().is_empty()) {
            payload.insert("note".to_string(), Value::String(note.trim().to_string()));
        }
        let Ok(mut response) = question_reply_payload(&Value::Object(payload)) else {
            self.status = "question invalid".to_string();
            return json!({"applied": false, "error": "invalid question response"});
        };
        merge_identity_fields(
            &mut response,
            &question,
            &["request_id", "turn_id", "session_id", "tool_call_id"],
        );
        self.active_question = None;
        self.clear_interaction(InteractionFocus::Question);
        self.status = "question answered".to_string();
        self.timeline.push(TimelineLine::new(
            "status",
            question_response_summary(&response),
            true,
        ));
        self.question_responses.push(response.clone());
        json!({"applied": true, "action": "question.reply", "payload": response})
    }

    pub fn dismiss_active_question(&mut self, note: Option<&str>) -> Value {
        let Some(question) = self.active_question.clone() else {
            self.status = "question missing".to_string();
            return json!({"applied": false, "error": "no active question"});
        };
        let mut payload = Map::new();
        if let Some(note) = note.filter(|value| !value.trim().is_empty()) {
            payload.insert("note".to_string(), Value::String(note.trim().to_string()));
        }
        let mut response = question_dismiss_payload(&Value::Object(payload));
        merge_identity_fields(
            &mut response,
            &question,
            &["request_id", "turn_id", "session_id", "tool_call_id"],
        );
        self.active_question = None;
        self.clear_interaction(InteractionFocus::Question);
        self.status = "question dismissed".to_string();
        self.timeline.push(TimelineLine::new(
            "warning",
            question_response_summary(&response),
            true,
        ));
        self.question_responses.push(response.clone());
        json!({"applied": true, "action": "question.dismiss", "payload": response})
    }

    fn show_help(&mut self) {
        let lines = BUILTIN_COMMANDS
            .iter()
            .map(|(name, description)| format!("{name} - {description}"))
            .collect::<Vec<_>>()
            .join("\n");
        self.timeline.push(TimelineLine::new(
            "status",
            format!("built-in commands:\n{lines}"),
            true,
        ));
        self.status = "help listed".to_string();
    }

    fn apply_turn_started(&mut self, event: &Value) -> Value {
        let params = object_value(event.get("params"));
        self.update_session_and_turn(&params);
        let turn_id = params
            .get("turn_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        self.status = "running".to_string();
        self.timeline.push(TimelineLine::new(
            "status",
            if turn_id.is_empty() {
                "turn started".to_string()
            } else {
                format!("turn started: {turn_id}")
            },
            true,
        ));
        json!({"applied": true, "method": "turn/started"})
    }

    fn apply_agent_message_started(&mut self, event: &Value) -> Value {
        let params = object_value(event.get("params"));
        self.update_session_and_turn(&params);
        self.status = "assistant streaming".to_string();
        json!({"applied": true, "method": "item/agentMessage/started"})
    }

    fn apply_agent_message_delta(&mut self, event: &Value) -> Value {
        let params = object_value(event.get("params"));
        self.update_session_and_turn(&params);
        let text = params
            .get("delta")
            .and_then(Value::as_str)
            .or_else(|| {
                params
                    .get("event")
                    .and_then(|event| event.get("text"))
                    .and_then(Value::as_str)
            })
            .unwrap_or_default();
        if !text.trim().is_empty() {
            self.timeline
                .push(TimelineLine::new("assistant", text.to_string(), false));
        }
        self.status = "assistant streaming".to_string();
        json!({"applied": true, "method": "item/agentMessage/delta"})
    }

    fn apply_agent_message_completed(&mut self, event: &Value) -> Value {
        let params = object_value(event.get("params"));
        self.update_session_and_turn(&params);
        self.status = "assistant completed".to_string();
        json!({"applied": true, "method": "item/agentMessage/completed"})
    }

    fn apply_turn_completed(&mut self, event: &Value) -> Value {
        let params = object_value(event.get("params"));
        self.update_session_and_turn(&params);
        if let Some(answer) = params
            .get("final_answer")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
        {
            self.timeline
                .push(TimelineLine::new("assistant", answer.to_string(), true));
        }
        if let Some(trace) = params.get("trace").filter(|value| value.is_object()) {
            self.timeline.push(TimelineLine::new(
                "status",
                format!("trace: {}", compact_json(trace)),
                false,
            ));
        }
        if let Some(usage) = params.get("usage").filter(|value| value.is_object()) {
            self.merge_usage(usage);
            self.timeline.push(TimelineLine::new(
                "status",
                format!(
                    "usage: {} totals={}",
                    compact_json(usage),
                    compact_json(&self.usage_totals)
                ),
                false,
            ));
        }
        self.status = params
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("completed")
            .to_string();
        json!({"applied": true, "method": "turn/completed"})
    }

    fn apply_turn_failed(&mut self, event: &Value, interrupted: bool) -> Value {
        let params = object_value(event.get("params"));
        self.update_session_and_turn(&params);
        let error = params
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or(if interrupted { "interrupted" } else { "failed" });
        self.status = if interrupted {
            "interrupted".to_string()
        } else {
            "failed".to_string()
        };
        self.timeline
            .push(TimelineLine::new("warning", error.to_string(), true));
        json!({"applied": true, "method": if interrupted { "turn/interrupted" } else { "turn/failed" }})
    }

    fn apply_tool_started(&mut self, event: &Value) -> Value {
        let params = object_value(event.get("params"));
        self.update_session_and_turn(&params);
        let name = params
            .get("name")
            .or_else(|| params.get("tool_name"))
            .and_then(Value::as_str)
            .unwrap_or("tool");
        let input = params.get("input").cloned().unwrap_or_else(|| json!({}));
        self.status = format!("tool running: {name}");
        self.timeline.push(TimelineLine::new(
            "status",
            format!("tool started: {name} {}", compact_json(&input)),
            false,
        ));
        json!({"applied": true, "method": "item/toolCall/started"})
    }

    fn apply_tool_finished(&mut self, event: &Value, failed: bool) -> Value {
        let params = object_value(event.get("params"));
        self.update_session_and_turn(&params);
        let name = params
            .get("name")
            .or_else(|| params.get("tool_name"))
            .and_then(Value::as_str)
            .unwrap_or("tool");
        let output = params
            .get("error")
            .or_else(|| params.get("output"))
            .cloned()
            .unwrap_or(Value::Null);
        self.status = if failed {
            format!("tool failed: {name}")
        } else {
            format!("tool completed: {name}")
        };
        self.timeline.push(TimelineLine::new(
            if failed { "warning" } else { "status" },
            format!("{}: {name} {}", self.status, compact_json(&output)),
            failed,
        ));
        if self.show_tool_details {
            let metadata = params.get("metadata").cloned().unwrap_or_else(|| json!({}));
            self.timeline.push(TimelineLine::new(
                "status",
                format!("tool details: {}", compact_json(&metadata)),
                false,
            ));
        }
        json!({"applied": true, "method": if failed { "item/toolCall/failed" } else { "item/toolCall/completed" }})
    }

    fn apply_question_resolved(&mut self, event: &Value) -> Value {
        let params = object_value(event.get("params"));
        let request_id = params
            .get("request_id")
            .or_else(|| {
                params
                    .get("question")
                    .and_then(|value| value.get("request_id"))
            })
            .and_then(Value::as_str)
            .unwrap_or_default();
        if let Some(active) = self.active_question.as_ref()
            && !request_id.is_empty()
            && string_field(active, "request_id") == request_id
        {
            self.active_question = None;
            self.clear_interaction(InteractionFocus::Question);
        }
        self.status = "question resolved".to_string();
        self.timeline
            .push(TimelineLine::new("status", "question resolved", true));
        json!({"applied": true, "method": "item/question/resolved"})
    }

    fn apply_reasoning_event(&mut self, event: &Value) -> Value {
        let params = object_value(event.get("params"));
        self.update_session_and_turn(&params);
        let text = params
            .get("delta")
            .or_else(|| params.get("text"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !text.trim().is_empty() {
            self.timeline.push(TimelineLine::new(
                "status",
                format!("reasoning: {text}"),
                false,
            ));
        }
        self.status = "reasoning".to_string();
        json!({"applied": true, "method": event.get("method").and_then(Value::as_str).unwrap_or("item/reasoning")})
    }

    fn apply_runtime_warning(&mut self, event: &Value) -> Value {
        let params = object_value(event.get("params"));
        let message = params
            .get("message")
            .or_else(|| params.get("warning"))
            .and_then(Value::as_str)
            .unwrap_or("runtime warning");
        self.status = "runtime warning".to_string();
        self.runtime_warnings.push(message.to_string());
        if self.runtime_warnings.len() > 50 {
            self.runtime_warnings.remove(0);
        }
        self.timeline.push(TimelineLine::new(
            "warning",
            format!("warning: {message}"),
            true,
        ));
        json!({"applied": true, "method": event.get("method").and_then(Value::as_str).unwrap_or("warning")})
    }

    fn apply_patch_event(&mut self, event: &Value, label: &str) -> Value {
        let params = object_value(event.get("params"));
        self.update_session_and_turn(&params);
        let patch = params.get("patch").cloned().unwrap_or_else(|| json!({}));
        self.status = label.to_string();
        self.timeline.extend(patch_lines(label, &patch, true));
        json!({"applied": true, "method": event.get("method").and_then(Value::as_str).unwrap_or("patch")})
    }

    fn apply_approval_requested(&mut self, event: &Value) -> Value {
        let params = object_value(event.get("params"));
        self.update_session_and_turn(&params);
        let approval = params
            .get("approval")
            .filter(|value| value.is_object())
            .cloned()
            .unwrap_or_else(|| json!({}));
        if string_field(&approval, "request_id").is_empty() {
            self.status = "approval invalid".to_string();
            return json!({"applied": false, "method": "turn/approval_requested", "error": "approval.request_id is required"});
        }
        self.active_approval = Some(approval.clone());
        self.focus_approval_interaction();
        self.status = "approval pending".to_string();
        self.timeline.push(TimelineLine::new(
            "warning",
            approval_request_summary(&approval),
            true,
        ));
        json!({"applied": true, "method": "turn/approval_requested"})
    }

    fn apply_approval_resolved(&mut self, event: &Value) -> Value {
        let params = object_value(event.get("params"));
        self.update_session_and_turn(&params);
        let approval = params
            .get("approval")
            .filter(|value| value.is_object())
            .cloned()
            .unwrap_or_else(|| json!({}));
        if approval_matches_active(&self.active_approval, &approval) {
            self.active_approval = None;
            self.clear_interaction(InteractionFocus::Approval);
        }
        self.status = "approval resolved".to_string();
        self.timeline.push(TimelineLine::new(
            "status",
            approval_response_summary(&approval),
            true,
        ));
        json!({"applied": true, "method": "turn/approval_resolved"})
    }

    fn apply_question_requested(&mut self, event: &Value) -> Value {
        let params = object_value(event.get("params"));
        self.update_session_and_turn(&params);
        let question = params
            .get("event")
            .filter(|value| value.is_object())
            .cloned()
            .unwrap_or(Value::Object(
                params.as_object().cloned().unwrap_or_default(),
            ));
        if string_field(&question, "request_id").is_empty() {
            self.status = "question invalid".to_string();
            return json!({"applied": false, "method": "item/question/requested", "error": "question.request_id is required"});
        }
        self.active_question = Some(question.clone());
        self.focus_question_interaction(&question);
        self.status = "question pending".to_string();
        self.timeline.push(TimelineLine::new(
            "warning",
            question_request_summary(&question),
            true,
        ));
        json!({"applied": true, "method": "item/question/requested"})
    }

    fn update_session_and_turn(&mut self, params: &Value) {
        if let Some(session_id) = params
            .get("session_id")
            .or_else(|| params.get("thread_id"))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            self.session_id = Some(session_id.to_string());
        }
        if let Some(turn_id) = params
            .get("turn_id")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        {
            self.current_turn_id = Some(turn_id.to_string());
        }
    }

    fn handle_interaction_command(&mut self, raw_text: &str) -> bool {
        let Some(command_line) = raw_text.strip_prefix('/') else {
            return false;
        };
        let mut parts = command_line.split_whitespace();
        let Some(name) = parts.next() else {
            return false;
        };
        match name {
            "allow" | "approve" => {
                let scope = parts.next();
                self.respond_active_approval("allow", scope, None);
                true
            }
            "deny" | "reject" => {
                let note = command_line
                    .strip_prefix(name)
                    .map(str::trim)
                    .filter(|value| !value.is_empty());
                self.respond_active_approval("deny", None, note);
                true
            }
            "answer" => {
                let answer = command_line
                    .strip_prefix(name)
                    .map(str::trim)
                    .unwrap_or_default();
                if answer.is_empty() {
                    self.status = "question invalid".to_string();
                    self.timeline
                        .push(TimelineLine::new("warning", "usage: /answer <text>", true));
                    return true;
                }
                self.answer_active_question(vec![vec![answer.to_string()]], None);
                true
            }
            "dismiss" => {
                let note = command_line
                    .strip_prefix(name)
                    .map(str::trim)
                    .filter(|value| !value.is_empty());
                self.dismiss_active_question(note);
                true
            }
            "stash" => {
                let draft = command_line
                    .strip_prefix(name)
                    .map(str::trim)
                    .unwrap_or_default();
                if draft.is_empty() {
                    self.timeline
                        .push(TimelineLine::new("warning", "usage: /stash <draft>", true));
                    self.status = "stash empty".to_string();
                } else {
                    self.input_buffer = draft.to_string();
                    self.stash_current_input();
                }
                true
            }
            "unstash" => {
                self.restore_latest_stash();
                true
            }
            "stashes" => {
                if self.stash.is_empty() {
                    self.timeline
                        .push(TimelineLine::new("status", "stash: empty", false));
                } else {
                    self.timeline.push(TimelineLine::new(
                        "status",
                        format!("stash: {} item(s)", self.stash.len()),
                        false,
                    ));
                    for (index, item) in self.stash.iter().rev().take(10).enumerate() {
                        self.timeline.push(TimelineLine::new(
                            "status",
                            format!("{}: {}", index + 1, clip_chars(item, 120)),
                            false,
                        ));
                    }
                }
                self.status = "stash listed".to_string();
                true
            }
            "attach" => {
                let reference = command_line
                    .strip_prefix(name)
                    .map(str::trim)
                    .unwrap_or_default();
                if reference.is_empty() {
                    self.timeline.push(TimelineLine::new(
                        "warning",
                        "usage: /attach <path[:range]>",
                        true,
                    ));
                    self.status = "attach invalid".to_string();
                } else {
                    self.input_buffer.clear();
                    self.insert_attachment_reference(reference);
                }
                true
            }
            "themes" | "theme" => {
                let requested = command_line
                    .strip_prefix(name)
                    .map(str::trim)
                    .unwrap_or_default();
                if requested.is_empty() {
                    self.timeline.push(TimelineLine::new(
                        "status",
                        format!(
                            "themes: default, light, high-contrast, midnight (current: {})",
                            self.config.theme
                        ),
                        false,
                    ));
                    self.status = "theme updated".to_string();
                } else {
                    self.set_theme(requested);
                }
                true
            }
            "config" => {
                self.timeline.push(TimelineLine::new(
                    "status",
                    format!("tui config: {}", compact_json(&json!(self.config))),
                    false,
                ));
                self.status = "config listed".to_string();
                true
            }
            "keybinds" => {
                self.timeline.push(TimelineLine::new(
                    "status",
                    format!("keybinds: {}", compact_json(&json!(self.config.keybinds))),
                    false,
                ));
                self.status = "keybinds listed".to_string();
                true
            }
            "usage" => {
                self.timeline.push(TimelineLine::new(
                    "status",
                    format!("usage totals: {}", compact_json(&self.usage_totals)),
                    false,
                ));
                self.status = "usage listed".to_string();
                true
            }
            "warnings" => {
                if self.runtime_warnings.is_empty() {
                    self.timeline
                        .push(TimelineLine::new("status", "warnings: none", false));
                } else {
                    self.timeline.push(TimelineLine::new(
                        "warning",
                        format!("warnings: {} item(s)", self.runtime_warnings.len()),
                        true,
                    ));
                    for warning in self.runtime_warnings.iter().rev().take(10) {
                        self.timeline
                            .push(TimelineLine::new("warning", warning.clone(), false));
                    }
                }
                self.status = "warnings listed".to_string();
                true
            }
            "tool-details" => {
                let requested = command_line
                    .strip_prefix(name)
                    .map(str::trim)
                    .unwrap_or_default();
                self.show_tool_details = match requested {
                    "on" | "true" | "1" => true,
                    "off" | "false" | "0" => false,
                    _ => !self.show_tool_details,
                };
                self.timeline.push(TimelineLine::new(
                    "status",
                    format!(
                        "tool details {}",
                        if self.show_tool_details { "on" } else { "off" }
                    ),
                    true,
                ));
                self.status = "tool details toggled".to_string();
                true
            }
            "editor" => {
                self.input_buffer.clear();
                if let Err(error) = self.edit_input_with_external_editor() {
                    self.timeline.push(TimelineLine::new("error", error, true));
                    self.status = "editor failed".to_string();
                }
                true
            }
            _ => false,
        }
    }

    fn respond_active_approval_from_params(&mut self, params: &Value, action_name: &str) -> Value {
        let action = params
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let scope = params.get("scope").and_then(Value::as_str);
        let note = params.get("note").and_then(Value::as_str);
        let mut result = self.respond_active_approval(action, scope, note);
        set_action_name(&mut result, action_name);
        result
    }

    fn answer_active_question_from_params(&mut self, params: &Value, action_name: &str) -> Value {
        let answers = params
            .get("answers")
            .cloned()
            .or_else(|| {
                params
                    .get("answer")
                    .and_then(Value::as_str)
                    .map(|answer| json!([[answer]]))
            })
            .and_then(|value| answer_vecs(&value))
            .unwrap_or_default();
        let note = params.get("note").and_then(Value::as_str);
        let mut result = self.answer_active_question(answers, note);
        set_action_name(&mut result, action_name);
        result
    }

    fn dismiss_active_question_from_params(&mut self, params: &Value, action_name: &str) -> Value {
        let note = params.get("note").and_then(Value::as_str);
        let mut result = self.dismiss_active_question(note);
        set_action_name(&mut result, action_name);
        result
    }

    fn open_model_control(&mut self, params: &Value, action_name: &str) -> Value {
        if let Some(models) = params.get("models").and_then(Value::as_array).cloned() {
            self.open_model_picker("", models);
            self.timeline.extend(model_list_lines(params));
            json!({"applied": true, "action": action_name})
        } else {
            self.timeline.push(TimelineLine::new(
                "status",
                "queued model picker: /models",
                true,
            ));
            self.status = "model picker queued".to_string();
            json!({"applied": true, "action": action_name, "command": "/models"})
        }
    }

    fn select_model_control(&mut self, params: &Value, action_name: &str) -> Value {
        let model = control_string_field(params, &["model", "model_id", "modelID", "id"]);
        if model.is_empty() {
            self.status = "control invalid".to_string();
            return json!({"applied": false, "action": action_name, "error": "model id is required"});
        }
        let command = format!("/models {model}");
        self.timeline.push(TimelineLine::new(
            "status",
            format!("queued model selection: {model}"),
            true,
        ));
        self.status = "model queued".to_string();
        json!({"applied": true, "action": action_name, "command": command})
    }

    fn open_agent_control(&mut self, params: &Value, action_name: &str) -> Value {
        if let Some(agents) = params.get("agents").and_then(Value::as_array).cloned() {
            self.open_agent_picker("", agents);
            self.timeline.extend(agent_list_lines(params));
            json!({"applied": true, "action": action_name})
        } else {
            self.timeline.push(TimelineLine::new(
                "status",
                "queued agent picker: /agents",
                true,
            ));
            self.status = "agent picker queued".to_string();
            json!({"applied": true, "action": action_name, "command": "/agents"})
        }
    }

    fn select_agent_control(&mut self, params: &Value, action_name: &str) -> Value {
        let agent = control_string_field(params, &["agent", "agent_id", "agentID", "id"]);
        if agent.is_empty() {
            self.status = "control invalid".to_string();
            return json!({"applied": false, "action": action_name, "error": "agent id is required"});
        }
        self.queue_session_setting_command(action_name, "agent", &agent)
    }

    fn open_variant_control(&mut self, params: &Value, action_name: &str) -> Value {
        if let Some(variants) = params
            .get("variants")
            .and_then(Value::as_array)
            .map(|items| string_array(items))
            .filter(|items| !items.is_empty())
        {
            self.open_choice_picker(ChoicePickerKind::Variant, "", variants.clone());
            self.timeline.push(TimelineLine::new(
                "status",
                format!("variants: {}", variants.join(", ")),
                true,
            ));
            return json!({"applied": true, "action": action_name, "variants": variants});
        }
        self.timeline.push(TimelineLine::new(
            "status",
            "queued variant picker: /variant",
            true,
        ));
        self.status = "variant picker queued".to_string();
        json!({"applied": true, "action": action_name, "command": "/variant"})
    }

    fn open_thinking_control(&mut self, params: &Value, action_name: &str) -> Value {
        if let Some(levels) = params
            .get("levels")
            .or_else(|| params.get("thinking"))
            .and_then(Value::as_array)
            .map(|items| string_array(items))
            .filter(|items| !items.is_empty())
        {
            self.open_choice_picker(ChoicePickerKind::Thinking, "", levels.clone());
            self.timeline.push(TimelineLine::new(
                "status",
                format!("thinking levels: {}", levels.join(", ")),
                true,
            ));
            return json!({"applied": true, "action": action_name, "levels": levels});
        }
        self.timeline.push(TimelineLine::new(
            "status",
            "queued thinking picker: /thinking",
            true,
        ));
        self.status = "thinking picker queued".to_string();
        json!({"applied": true, "action": action_name, "command": "/thinking"})
    }

    fn select_named_session_setting_control(
        &mut self,
        params: &Value,
        action_name: &str,
        command: &str,
        field: &str,
    ) -> Value {
        let value = control_string_field(params, &[field, "value", "id", "name"]);
        if value.is_empty() {
            self.status = "control invalid".to_string();
            return json!({"applied": false, "action": action_name, "error": format!("{field} is required")});
        }
        self.queue_session_setting_command(action_name, command, &value)
    }

    fn queue_session_setting_command(
        &mut self,
        action_name: &str,
        command: &str,
        value: &str,
    ) -> Value {
        let slash = format!("/{command} {value}");
        self.timeline.push(TimelineLine::new(
            "status",
            format!("queued {command} selection: {value}"),
            true,
        ));
        self.status = format!("{command} queued");
        json!({"applied": true, "action": action_name, "command": slash})
    }

    fn open_theme_control(&mut self, params: &Value, action_name: &str) -> Value {
        let themes = params
            .get("themes")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
            })
            .filter(|items| !items.is_empty())
            .unwrap_or_else(default_theme_names);
        self.open_choice_picker(ChoicePickerKind::Theme, "", themes.clone());
        self.timeline.push(TimelineLine::new(
            "status",
            format!(
                "themes: {} (current: {})",
                themes.join(", "),
                self.config.theme
            ),
            true,
        ));
        json!({"applied": true, "action": action_name, "themes": themes})
    }

    fn select_theme_control(&mut self, params: &Value, action_name: &str) -> Value {
        let theme = control_string_field(params, &["theme", "id", "name"]);
        if theme.is_empty() {
            self.status = "control invalid".to_string();
            return json!({"applied": false, "action": action_name, "error": "theme is required"});
        }
        self.set_theme(&theme);
        json!({"applied": true, "action": action_name, "theme": theme})
    }

    fn open_palette_control(&mut self, params: &Value, action_name: &str) -> Value {
        let query = params
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase();
        let mut commands = BUILTIN_COMMANDS
            .iter()
            .filter(|(name, description)| {
                query.is_empty()
                    || name.to_ascii_lowercase().contains(&query)
                    || description.to_ascii_lowercase().contains(&query)
            })
            .take(12)
            .map(|(name, description)| format!("{name} - {description}"))
            .collect::<Vec<_>>();
        if commands.is_empty() {
            commands.push("no commands matched".to_string());
        }
        self.timeline.push(TimelineLine::new(
            "status",
            format!("command palette:\n{}", commands.join("\n")),
            true,
        ));
        self.status = "palette open".to_string();
        json!({"applied": true, "action": action_name, "commands": commands})
    }

    fn execute_palette_control(&mut self, params: &Value, action_name: &str) -> Value {
        let command = control_string_field(params, &["command", "id", "name"]);
        if command.is_empty() {
            self.status = "control invalid".to_string();
            return json!({"applied": false, "action": action_name, "error": "command is required"});
        }
        let command = if command.starts_with('/') {
            command
        } else {
            format!("/{command}")
        };
        if is_local_state_command(&command) || matches!(command.as_str(), "/help" | "/?" | "/") {
            self.input_buffer = command;
            let submitted = self.submit();
            json!({"applied": submitted, "action": action_name})
        } else {
            self.timeline.push(TimelineLine::new(
                "status",
                format!("queued palette command: {command}"),
                true,
            ));
            self.status = "palette command queued".to_string();
            json!({"applied": true, "action": action_name, "command": command})
        }
    }

    fn open_file_control(&mut self, params: &Value, action_name: &str) -> Value {
        let query = control_string_field(params, &["query", "text", "value"]);
        let command = if query.is_empty() {
            "/files".to_string()
        } else {
            format!("/files {query}")
        };
        self.timeline.push(TimelineLine::new(
            "status",
            format!("queued file picker: {command}"),
            true,
        ));
        self.status = "file picker queued".to_string();
        json!({"applied": true, "action": action_name, "command": command})
    }

    fn select_file_control(&mut self, params: &Value, action_name: &str) -> Value {
        let path = control_string_field(params, &["path", "file", "id", "value", "name"]);
        if path.is_empty() {
            self.status = "control invalid".to_string();
            return json!({"applied": false, "action": action_name, "error": "path is required"});
        }
        let reference = attachment_reference_from_parts(
            &path,
            params
                .get("line")
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok()),
            params
                .get("start")
                .or_else(|| params.get("line_start"))
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok()),
            params
                .get("end")
                .or_else(|| params.get("line_end"))
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok()),
        );
        let Some(token) = normalize_attachment_reference_token(&reference) else {
            self.status = "control invalid".to_string();
            return json!({"applied": false, "action": action_name, "error": "path cannot be represented as an @ attachment"});
        };
        self.file_picker = None;
        self.insert_attachment_reference(&reference);
        json!({"applied": true, "action": action_name, "reference": token})
    }

    fn session_command_control(&mut self, params: &Value, action_name: &str, verb: &str) -> Value {
        let value = control_string_field(params, &["title", "name", "value", "label"]);
        if value.is_empty() {
            self.status = "control invalid".to_string();
            return json!({"applied": false, "action": action_name, "error": "title is required"});
        }
        self.session_literal_command_control(action_name, format!("/{verb} {value}"))
    }

    fn session_literal_command_control(&mut self, action_name: &str, command: String) -> Value {
        self.timeline.push(TimelineLine::new(
            "status",
            format!("queued session command: {command}"),
            true,
        ));
        self.status = "session command queued".to_string();
        json!({"applied": true, "action": action_name, "command": command})
    }
}

#[must_use]
pub fn normalize_control_action(action: &str) -> String {
    match action {
        "append-prompt" => "prompt.append",
        "submit-prompt" => "prompt.submit",
        "clear-prompt" => "prompt.clear",
        "open-help" => "help.open",
        "open-sessions" => "sessions.open",
        "open-themes" => "theme.open",
        "open-models" => "model.open",
        "open-agents" => "agent.open",
        "open-variants" => "variant.open",
        "open-thinking" => "thinking.open",
        "open-palette" => "palette.open",
        "open-files" => "file.open",
        "select-session" => "session.select",
        "rename-session" => "session.rename",
        "archive-session" => "session.archive",
        "unarchive-session" => "session.unarchive",
        "delete-session" => "session.delete",
        "fork-session" => "session.fork",
        "session-children" | "open-children" => "session.children",
        "parent-session" => "session.parent",
        "share-session" => "session.share",
        "unshare-session" => "session.unshare",
        "compact-session" => "session.compact",
        "session-details" => "session.details",
        "undo-session" => "session.undo",
        "redo-session" => "session.redo",
        "select-model" => "model.select",
        "select-agent" => "agent.select",
        "select-variant" => "variant.select",
        "select-thinking" => "thinking.select",
        "select-theme" => "theme.select",
        "select-file" | "attach-file" => "file.select",
        "show-toast" => "toast.show",
        "execute-command" => "command.execute",
        "execute-palette" => "palette.execute",
        "respond-approval" => "approval.respond",
        "reply-question" => "question.reply",
        "dismiss-question" => "question.dismiss",
        other => other,
    }
    .to_string()
}

fn default_theme_names() -> Vec<String> {
    ["default", "light", "high-contrast", "midnight"]
        .into_iter()
        .map(ToString::to_string)
        .collect()
}

fn string_array(items: &[Value]) -> Vec<String> {
    items
        .iter()
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect()
}

fn control_string_field(params: &Value, keys: &[&str]) -> String {
    keys.iter()
        .find_map(|key| params.get(*key).and_then(Value::as_str))
        .unwrap_or_default()
        .trim()
        .to_string()
}

#[must_use]
pub fn control_publish_to_action(params: &Value) -> (String, Value) {
    let topic = params
        .get("type")
        .or_else(|| params.get("topic"))
        .or_else(|| params.get("event"))
        .or_else(|| params.get("method"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let payload = params.get("properties").or_else(|| params.get("payload"));
    let body = if let Some(Value::Object(object)) = payload {
        Value::Object(object.clone())
    } else {
        let mut object = Map::new();
        if let Some(source) = params.as_object() {
            for (key, value) in source {
                if !matches!(
                    key.as_str(),
                    "type" | "topic" | "event" | "method" | "properties" | "payload"
                ) {
                    object.insert(key.clone(), value.clone());
                }
            }
        }
        Value::Object(object)
    };
    let action = match topic {
        "tui.prompt.append" => "prompt.append",
        "tui.command.execute" => "command.execute",
        "tui.toast.show" => "toast.show",
        "tui.session.select" => "session.select",
        "tui.session.rename" => "session.rename",
        "tui.session.archive" => "session.archive",
        "tui.session.unarchive" => "session.unarchive",
        "tui.session.delete" => "session.delete",
        "tui.session.fork" => "session.fork",
        "tui.session.children" => "session.children",
        "tui.session.parent" => "session.parent",
        "tui.session.share" => "session.share",
        "tui.session.unshare" => "session.unshare",
        "tui.session.compact" => "session.compact",
        "tui.session.details" => "session.details",
        "tui.session.undo" => "session.undo",
        "tui.session.redo" => "session.redo",
        "tui.model.open" => "model.open",
        "tui.model.select" => "model.select",
        "tui.agent.open" => "agent.open",
        "tui.agent.select" => "agent.select",
        "tui.variant.open" => "variant.open",
        "tui.variant.select" => "variant.select",
        "tui.thinking.open" => "thinking.open",
        "tui.thinking.select" => "thinking.select",
        "tui.file.open" => "file.open",
        "tui.file.select" | "tui.file.attach" => "file.select",
        other => other,
    };
    (action.to_string(), body)
}

fn approval_request_summary(approval: &Value) -> String {
    let tool_name = string_field(approval, "tool_name").if_empty_then(|| "tool".to_string());
    let tool_input = approval
        .get("tool_input")
        .map(compact_json)
        .unwrap_or_else(|| "{}".to_string());
    let mut lines = vec![format!("approval required: {tool_name} {tool_input}")];
    if let Some(call_id) = approval.get("call_id").and_then(Value::as_str) {
        if !call_id.is_empty() {
            lines.push(format!("call: {call_id}"));
        }
    }
    if let Some(preview) = approval.get("preview").filter(|value| value.is_object()) {
        lines.extend(preview_lines(preview));
    }
    lines.join("\n")
}

fn approval_response_summary(approval: &Value) -> String {
    let action = string_field(approval, "action").if_empty_then(|| "resolved".to_string());
    let tool = string_field(approval, "tool_name").if_empty_then(|| "tool".to_string());
    let mut suffix = Vec::new();
    if let Some(scope) = approval.get("scope").and_then(Value::as_str) {
        if !scope.is_empty() {
            suffix.push(scope.to_string());
        }
    }
    if let Some(note) = approval.get("note").and_then(Value::as_str) {
        if !note.is_empty() {
            suffix.push(note.to_string());
        }
    }
    if suffix.is_empty() {
        format!("approval {action}: {tool}")
    } else {
        format!("approval {action}: {tool} ({})", suffix.join("; "))
    }
}

fn preview_lines(preview: &Value) -> Vec<String> {
    let mut lines = Vec::new();
    let kind = string_field(preview, "kind").if_empty_then(|| "tool".to_string());
    lines.push(format!("preview: {kind}"));
    if let Some(path) = preview.get("path").and_then(Value::as_str) {
        if !path.is_empty() {
            let status = preview
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let suffix = if status.is_empty() {
                String::new()
            } else {
                format!(" ({status})")
            };
            lines.push(format!("path: {path}{suffix}"));
        }
    }
    if let Some(command) = preview.get("command").and_then(Value::as_str) {
        if !command.is_empty() {
            lines.push(format!("command: {command}"));
        }
    }
    if let Some(warnings) = preview.get("warnings").and_then(Value::as_array) {
        lines.extend(
            warnings
                .iter()
                .filter_map(Value::as_str)
                .take(3)
                .map(|warning| format!("warning: {warning}")),
        );
    }
    if let Some(diff) = preview.get("diff").and_then(Value::as_str) {
        if !diff.trim().is_empty() {
            lines.push("diff:".to_string());
            lines.extend(trim_lines(diff, 40));
        }
    }
    if let Some(summary) = preview.get("summary").and_then(Value::as_str) {
        if !summary.is_empty() {
            lines.push(format!("summary: {summary}"));
        }
    }
    lines
}

fn question_request_summary(question: &Value) -> String {
    let questions = question
        .get("questions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut lines = vec![format!("question requested: {} item(s)", questions.len())];
    for (index, item) in questions.iter().enumerate().take(5) {
        let label = item
            .get("header")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or("Question");
        let text = item
            .get("question")
            .and_then(Value::as_str)
            .unwrap_or_default();
        lines.push(format!("{}. {label}: {text}", index + 1));
        if let Some(options) = item.get("options").and_then(Value::as_array) {
            for option in options.iter().take(3) {
                if let Some(option_label) = option.get("label").and_then(Value::as_str) {
                    lines.push(format!("   - {option_label}"));
                }
            }
        }
    }
    lines.join("\n")
}

fn question_items(question: &Value) -> Vec<Value> {
    question
        .get("questions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn question_option_values(question: &Value) -> Vec<Value> {
    question
        .get("options")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn question_option_labels(question: &Value) -> Vec<String> {
    question_option_values(question)
        .into_iter()
        .filter_map(|option| {
            option
                .get("label")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| option.as_str().map(str::to_string))
        })
        .filter(|label| !label.is_empty())
        .collect()
}

fn question_response_summary(response: &Value) -> String {
    let dismissed = response
        .get("dismissed")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if dismissed {
        let note = response
            .get("note")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if note.is_empty() {
            "question dismissed".to_string()
        } else {
            format!("question dismissed: {note}")
        }
    } else {
        let count = response
            .get("answers")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        format!("question answered: {count} item(s)")
    }
}

fn merge_identity_fields(target: &mut Value, source: &Value, keys: &[&str]) {
    let Some(target_object) = target.as_object_mut() else {
        return;
    };
    for key in keys {
        if let Some(value) = source.get(*key) {
            if !value.is_null() {
                target_object.insert((*key).to_string(), value.clone());
            }
        }
    }
}

fn approval_matches_active(active: &Option<Value>, approval: &Value) -> bool {
    let Some(active) = active else {
        return false;
    };
    let active_id = string_field(active, "request_id");
    !active_id.is_empty() && active_id == string_field(approval, "request_id")
}

fn answer_vecs(value: &Value) -> Option<Vec<Vec<String>>> {
    value.as_array().map(|items| {
        items
            .iter()
            .filter_map(|item| {
                if let Some(text) = item.as_str() {
                    Some(vec![text.to_string()])
                } else {
                    item.as_array().map(|values| {
                        values
                            .iter()
                            .filter_map(Value::as_str)
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                    })
                }
            })
            .collect()
    })
}

fn set_action_name(value: &mut Value, action: &str) {
    if let Some(object) = value.as_object_mut() {
        object.insert("action".to_string(), Value::String(action.to_string()));
    }
}

fn compact_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
}

fn usage_totals_value(
    input_tokens: u64,
    output_tokens: u64,
    total_tokens: u64,
    cost: f64,
) -> Value {
    json!({
        "input_tokens": input_tokens,
        "output_tokens": output_tokens,
        "total_tokens": total_tokens,
        "cost": cost,
    })
}

fn timeline_style(config: &TuiConfig, kind: &str) -> Style {
    let color = match config.theme.as_str() {
        "light" => match kind {
            "error" => Color::Red,
            "warning" => Color::Yellow,
            "user" => Color::Blue,
            "assistant" => Color::Black,
            "patch" | "diff-hunk" => Color::Blue,
            "diff-add" => Color::Green,
            "diff-del" => Color::Red,
            "diff-meta" => Color::DarkGray,
            _ => Color::DarkGray,
        },
        "high-contrast" => match kind {
            "error" => Color::LightRed,
            "warning" => Color::LightYellow,
            "user" => Color::LightGreen,
            "assistant" => Color::White,
            "patch" | "diff-hunk" => Color::LightCyan,
            "diff-add" => Color::LightGreen,
            "diff-del" => Color::LightRed,
            "diff-meta" => Color::LightYellow,
            _ => Color::LightCyan,
        },
        "midnight" => match kind {
            "error" => Color::LightRed,
            "warning" => Color::LightMagenta,
            "user" => Color::LightBlue,
            "assistant" => Color::White,
            "patch" | "diff-hunk" => Color::LightBlue,
            "diff-add" => Color::LightGreen,
            "diff-del" => Color::LightRed,
            "diff-meta" => Color::DarkGray,
            _ => Color::Cyan,
        },
        _ => match kind {
            "error" => Color::Red,
            "warning" => Color::Yellow,
            "user" => Color::Green,
            "assistant" => Color::White,
            "patch" | "diff-hunk" => Color::Cyan,
            "diff-add" => Color::Green,
            "diff-del" => Color::Red,
            "diff-meta" => Color::DarkGray,
            _ => Color::Gray,
        },
    };
    Style::default().fg(color)
}

fn trim_lines(value: &str, max_lines: usize) -> Vec<String> {
    let lines = value.lines().map(ToString::to_string).collect::<Vec<_>>();
    if lines.len() <= max_lines {
        return lines;
    }
    let omitted = lines.len() - max_lines;
    let mut output = lines.into_iter().take(max_lines).collect::<Vec<_>>();
    output.push(format!("... diff truncated ({omitted} more lines) ..."));
    output
}

fn clip_chars(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_string();
    }
    format!("{}...", value.chars().take(limit).collect::<String>())
}

fn string_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppBridgeTerminalOptions {
    pub server_url: String,
    pub auth: RemoteAuth,
    pub workspace: PathBuf,
    pub session_id: Option<String>,
    pub continue_last: bool,
    pub fork: bool,
    pub permission: Option<String>,
    pub dangerously_skip_permissions: bool,
}

impl Default for AppBridgeTerminalOptions {
    fn default() -> Self {
        Self {
            server_url: "http://127.0.0.1:8787".to_string(),
            auth: RemoteAuth::default(),
            workspace: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            session_id: None,
            continue_last: false,
            fork: false,
            permission: None,
            dangerously_skip_permissions: false,
        }
    }
}

pub struct AppBridgeTerminalHandler {
    client: RemoteRuntimeClient,
    workspace: PathBuf,
    current_session: Option<String>,
    continue_last: bool,
    fork_next: bool,
    permission: Option<String>,
    dangerously_skip_permissions: bool,
    last_turn_id: Option<String>,
    last_global_event_id: u64,
    pending_events: Vec<Value>,
    seen_events: BTreeSet<String>,
}

impl AppBridgeTerminalHandler {
    pub fn connect(options: AppBridgeTerminalOptions) -> Result<Self, String> {
        let client = RemoteRuntimeClient::new(options.server_url.clone())
            .with_auth(options.auth)
            .with_timeout(Duration::from_secs(3));
        client.health()?;
        let mut handler = Self {
            client,
            workspace: options.workspace,
            current_session: options.session_id,
            continue_last: options.continue_last,
            fork_next: options.fork,
            permission: options.permission,
            dangerously_skip_permissions: options.dangerously_skip_permissions,
            last_turn_id: None,
            last_global_event_id: 0,
            pending_events: Vec::new(),
            seen_events: BTreeSet::new(),
        };
        if handler.current_session.is_none() && (handler.continue_last || handler.fork_next) {
            let session_id = handler.client.select_session(
                None,
                handler.continue_last,
                handler.fork_next,
                &handler.workspace,
            )?;
            handler.current_session = Some(session_id);
            handler.fork_next = false;
        }
        Ok(handler)
    }

    #[must_use]
    pub fn server_url(&self) -> &str {
        self.client.server_url()
    }

    #[must_use]
    pub fn current_session(&self) -> Option<&str> {
        self.current_session.as_deref()
    }

    fn ensure_session(&mut self) -> Result<String, String> {
        if let Some(session_id) = self.current_session.clone() {
            return Ok(session_id);
        }
        let session_id = self
            .client
            .select_session(None, false, false, &self.workspace)?;
        self.current_session = Some(session_id.clone());
        Ok(session_id)
    }

    fn start_new_session(&mut self, fork_from: Option<String>) -> Result<String, String> {
        let session_id = self
            .client
            .create_session(&self.workspace, fork_from.as_deref())?;
        self.current_session = Some(session_id.clone());
        Ok(session_id)
    }

    fn require_current_session(&self) -> Result<String, String> {
        self.current_session
            .clone()
            .ok_or_else(|| "no current session; use /new or /resume <session_id>".to_string())
    }

    fn remember_payload_events(&mut self, payload: &Value) {
        let events = self.filter_new_events(events_from_payload(payload));
        self.pending_events.extend(events);
    }

    fn filter_new_events(&mut self, events: Vec<Value>) -> Vec<Value> {
        let mut output = Vec::new();
        for event in events {
            let sequence = event_sequence(&event);
            if sequence > self.last_global_event_id {
                self.last_global_event_id = sequence;
            }
            let key = event_identity_key(&event);
            if self.seen_events.insert(key) {
                output.push(event);
            }
        }
        output
    }

    fn turn_options(&self) -> Value {
        let mut value = json!({});
        if let Some(permission) = self
            .permission
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            value["permission"] = json!(permission);
        }
        if self.dangerously_skip_permissions {
            value["dangerously_skip_permissions"] = json!(true);
        }
        value
    }

    fn update_session_setting(
        &mut self,
        key: &str,
        value: &str,
    ) -> Result<Vec<TimelineLine>, String> {
        let session_id = self.require_current_session()?;
        let mut body = Map::new();
        body.insert(key.to_string(), json!(value));
        let payload = self
            .client
            .update_session(&session_id, Value::Object(body))?;
        Ok(vec![TimelineLine::new(
            "status",
            format!(
                "{key} set to {value}: {}",
                compact_json(&payload["session"])
            ),
            true,
        )])
    }
}

impl TerminalEventHandler for AppBridgeTerminalHandler {
    fn initial_lines(&mut self) -> Vec<TimelineLine> {
        let mut lines = vec![TimelineLine::new(
            "status",
            format!("connected to {}", self.client.server_url()),
            true,
        )];
        if let Some(session_id) = self.current_session.as_deref() {
            lines.push(TimelineLine::new(
                "status",
                format!("current session: {session_id}"),
                true,
            ));
        }
        match self.client.list_sessions() {
            Ok(sessions) if sessions.is_empty() => {
                lines.push(TimelineLine::new("status", "remote sessions: none", false));
            }
            Ok(sessions) => lines.extend(session_list_lines(&sessions)),
            Err(error) => lines.push(TimelineLine::new("warning", error, true)),
        }
        lines
    }

    fn poll_app_events(&mut self) -> Result<Vec<Value>, String> {
        let events = self.client.global_events(self.last_global_event_id)?;
        Ok(self.filter_new_events(events))
    }

    fn poll_control_request(&mut self) -> Result<Option<Value>, String> {
        let request = self.client.next_tui_control()?;
        let path = request
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if path.is_empty() {
            return Ok(None);
        }
        Ok(Some(request))
    }

    fn record_control_response(&mut self, payload: &Value) -> Result<(), String> {
        self.client.record_tui_control_response(payload).map(|_| ())
    }

    fn drain_app_events(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.pending_events)
    }

    fn search_files(&mut self, query: &str) -> Result<Vec<ComposerFileCandidate>, String> {
        Ok(fuzzy_find_files(&self.workspace, query, 20)
            .into_iter()
            .map(composer_candidate_from_match)
            .collect())
    }

    fn search_sessions(&mut self, query: &str) -> Result<Vec<Value>, String> {
        self.client.search_sessions(query)
    }

    fn list_models(&mut self) -> Result<Value, String> {
        self.client.models()
    }

    fn list_agents(&mut self) -> Result<Value, String> {
        self.client.agents()
    }

    fn handle_submit(&mut self, prompt: &str) -> Result<Vec<TimelineLine>, String> {
        let session_id = self.ensure_session()?;
        let mut lines = Vec::new();
        let mut options = self.turn_options();
        let outbound_prompt = if let Some(command) = prompt
            .trim()
            .strip_prefix('!')
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            options["tool_call"] = json!({
                "call_id": format!("call_bash_{}", std::process::id()),
                "name": "bash",
                "input": {"command": command},
            });
            lines.push(TimelineLine::new(
                "status",
                format!("bash tool queued: {command}"),
                true,
            ));
            format!("Run shell command:\n{command}")
        } else {
            let expanded = expand_file_attachments(&self.workspace, prompt);
            lines.extend(expanded.lines);
            expanded.prompt
        };
        let payload = self
            .client
            .start_turn(&session_id, &outbound_prompt, options)?;
        self.last_turn_id = turn_id_from_payload(&payload).or_else(|| self.last_turn_id.clone());
        self.remember_payload_events(&payload);
        Ok(lines)
    }

    fn handle_command(&mut self, command: &str) -> Result<Vec<TimelineLine>, String> {
        if command == "/connect" {
            return Ok(vec![TimelineLine::new(
                "warning",
                "usage: /connect <server_url>",
                true,
            )]);
        }
        if let Some(url) = command.strip_prefix("/connect ").map(str::trim) {
            if url.is_empty() {
                return Ok(vec![TimelineLine::new(
                    "warning",
                    "usage: /connect <server_url>",
                    true,
                )]);
            }
            let client = RemoteRuntimeClient::new(url)
                .with_auth(self.client.auth().clone())
                .with_timeout(Duration::from_secs(3));
            client.health()?;
            self.client = client;
            self.current_session = None;
            self.last_turn_id = None;
            self.last_global_event_id = 0;
            self.seen_events.clear();
            return Ok(vec![TimelineLine::new(
                "status",
                format!("connected to {}", self.client.server_url()),
                true,
            )]);
        }
        if command == "/sessions" || command.starts_with("/sessions ") {
            let query = command
                .strip_prefix("/sessions")
                .map(str::trim)
                .unwrap_or_default();
            return self.client.search_sessions(query).map(|sessions| {
                if sessions.is_empty() {
                    vec![TimelineLine::new("status", "remote sessions: none", false)]
                } else {
                    session_list_lines(&sessions)
                }
            });
        }
        if let Some(session_id) = command.strip_prefix("/resume ").map(str::trim) {
            if session_id.is_empty() {
                return Ok(vec![TimelineLine::new(
                    "warning",
                    "usage: /resume <session_id>",
                    true,
                )]);
            }
            self.current_session = Some(session_id.to_string());
            return Ok(vec![TimelineLine::new(
                "status",
                format!("current session: {session_id}"),
                true,
            )]);
        }
        if command == "/transcript" || command.starts_with("/transcript ") {
            let raw_limit = command.strip_prefix("/transcript ").map(str::trim);
            let limit = match raw_limit {
                Some("") | None => None,
                Some(value) => match value.parse::<usize>() {
                    Ok(limit) => Some(limit),
                    Err(_) => {
                        return Ok(vec![TimelineLine::new(
                            "warning",
                            "usage: /transcript [limit]",
                            true,
                        )]);
                    }
                },
            };
            let session_id = self.require_current_session()?;
            let payload = self.client.session_messages(&session_id, limit)?;
            return Ok(transcript_lines(&payload));
        }
        if let Some(title) = command.strip_prefix("/rename ").map(str::trim) {
            if title.is_empty() {
                return Ok(vec![TimelineLine::new(
                    "warning",
                    "usage: /rename <title>",
                    true,
                )]);
            }
            let session_id = self.require_current_session()?;
            let payload = self
                .client
                .update_session(&session_id, json!({"title": title}))?;
            return Ok(vec![TimelineLine::new(
                "status",
                format!("renamed session: {}", compact_json(&payload["session"])),
                true,
            )]);
        }
        if command == "/archive" || command == "/unarchive" {
            let session_id = self.require_current_session()?;
            let archived = command == "/archive";
            let payload = self
                .client
                .update_session(&session_id, json!({"archived": archived}))?;
            return Ok(vec![TimelineLine::new(
                "status",
                format!(
                    "{} session: {}",
                    if archived { "archived" } else { "unarchived" },
                    compact_json(&payload["session"])
                ),
                true,
            )]);
        }
        if command == "/delete" {
            let session_id = self.require_current_session()?;
            let payload = self.client.delete_session(&session_id)?;
            self.current_session = None;
            return Ok(vec![TimelineLine::new(
                "warning",
                format!("deleted session: {}", compact_json(&payload)),
                true,
            )]);
        }
        if command == "/new" {
            let session_id = self.start_new_session(None)?;
            return Ok(vec![TimelineLine::new(
                "status",
                format!("created session: {session_id}"),
                true,
            )]);
        }
        if command == "/fork" {
            let Some(base) = self.current_session.clone() else {
                return Ok(vec![TimelineLine::new(
                    "warning",
                    "no current session to fork; use /new or /resume <session_id>",
                    true,
                )]);
            };
            let session_id = self.start_new_session(Some(base))?;
            return Ok(vec![TimelineLine::new(
                "status",
                format!("forked session: {session_id}"),
                true,
            )]);
        }
        if command == "/children" {
            let session_id = self.require_current_session()?;
            let children = self.client.children(&session_id)?;
            if children.is_empty() {
                return Ok(vec![TimelineLine::new(
                    "status",
                    "child sessions: none",
                    false,
                )]);
            }
            return Ok(session_list_lines(&children));
        }
        if command == "/parent" {
            let session_id = self.require_current_session()?;
            let payload = self.client.get_session(&session_id)?;
            let parent = payload
                .get("metadata")
                .and_then(|metadata| {
                    metadata
                        .get("parent_session_id")
                        .or_else(|| metadata.get("forked_from"))
                })
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty());
            let Some(parent) = parent else {
                return Ok(vec![TimelineLine::new(
                    "status",
                    "current session has no parent",
                    false,
                )]);
            };
            self.current_session = Some(parent.to_string());
            return Ok(vec![TimelineLine::new(
                "status",
                format!("current session: {parent}"),
                true,
            )]);
        }
        if command == "/share" {
            let session_id = self.require_current_session()?;
            let payload = self.client.share_session(&session_id)?;
            return Ok(vec![TimelineLine::new(
                "status",
                format!("shared session: {}", compact_json(&payload)),
                true,
            )]);
        }
        if command == "/unshare" {
            let session_id = self.require_current_session()?;
            let payload = self.client.unshare_session(&session_id)?;
            return Ok(vec![TimelineLine::new(
                "status",
                format!("unshared session: {}", compact_json(&payload)),
                true,
            )]);
        }
        if command == "/compact" {
            let session_id = self.require_current_session()?;
            let payload = self.client.compact_session(&session_id)?;
            return Ok(vec![TimelineLine::new(
                "status",
                format!("compacted session: {}", compact_json(&payload["summary"])),
                true,
            )]);
        }
        if command == "/details" {
            let session_id = self.require_current_session()?;
            let payload = self.client.session_diff(&session_id)?;
            return Ok(diff_detail_lines(&payload));
        }
        if command == "/undo" {
            let session_id = self.require_current_session()?;
            let payload = self.client.undo_session(&session_id)?;
            self.remember_payload_events(&payload);
            return Ok(patch_result_lines("undo", &payload));
        }
        if command == "/redo" {
            let session_id = self.require_current_session()?;
            let payload = self.client.redo_session(&session_id)?;
            self.remember_payload_events(&payload);
            return Ok(patch_result_lines("redo", &payload));
        }
        if command == "/export" {
            let session_id = self.require_current_session()?;
            let payload = self.client.get_session(&session_id)?;
            return Ok(vec![TimelineLine::new(
                "status",
                format!("session export: {}", compact_json(&payload)),
                true,
            )]);
        }
        if command == "/init" {
            let created = initialize_openagent_project_files(&self.workspace)?;
            return Ok(vec![TimelineLine::new(
                "status",
                format!("initialized project files: {}", created.join(", ")),
                true,
            )]);
        }
        if command == "/status" {
            let session = self
                .current_session
                .as_deref()
                .map(|session_id| self.client.get_session(session_id))
                .transpose()?;
            return Ok(vec![TimelineLine::new(
                "status",
                format!(
                    "server={} session={} turn={} {}",
                    self.client.server_url(),
                    self.current_session.as_deref().unwrap_or("-"),
                    self.last_turn_id.as_deref().unwrap_or("-"),
                    session
                        .as_ref()
                        .map(compact_json)
                        .unwrap_or_else(|| "{}".to_string())
                ),
                true,
            )]);
        }
        if command == "/files" || command.starts_with("/files ") {
            let query = command
                .strip_prefix("/files")
                .map(str::trim)
                .unwrap_or_default();
            let matches = fuzzy_find_files(&self.workspace, query, 20);
            return Ok(file_picker_lines(query, &matches));
        }
        if command == "/models" || command.starts_with("/models ") {
            let model_id = command.strip_prefix("/models ").map(str::trim);
            if let Some(model_id) = model_id.filter(|value| !value.is_empty()) {
                return self.update_session_setting("model", model_id);
            }
            let payload = self.client.models()?;
            return Ok(model_list_lines(&payload));
        }
        if command == "/agents" {
            let payload = self.client.agents()?;
            return Ok(agent_list_lines(&payload));
        }
        if let Some(agent) = command.strip_prefix("/agent ").map(str::trim) {
            if agent.is_empty() {
                return Ok(vec![TimelineLine::new(
                    "warning",
                    "usage: /agent <id>",
                    true,
                )]);
            }
            return self.update_session_setting("agent", agent);
        }
        if let Some(variant) = command.strip_prefix("/variant ").map(str::trim) {
            if variant.is_empty() {
                return Ok(vec![TimelineLine::new(
                    "warning",
                    "usage: /variant <default|fast|balanced|deep>",
                    true,
                )]);
            }
            return self.update_session_setting("variant", variant);
        }
        if command == "/thinking" || command.starts_with("/thinking ") {
            let Some(thinking) = command.strip_prefix("/thinking ").map(str::trim) else {
                return Ok(vec![TimelineLine::new(
                    "status",
                    "thinking levels: off, low, medium, high",
                    false,
                )]);
            };
            if thinking.is_empty() {
                return Ok(vec![TimelineLine::new(
                    "warning",
                    "usage: /thinking <off|low|medium|high>",
                    true,
                )]);
            }
            return self.update_session_setting("thinking", thinking);
        }
        if command == "/interrupt" || command.starts_with("/interrupt ") {
            let turn_id = command
                .strip_prefix("/interrupt ")
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .or_else(|| self.last_turn_id.clone());
            let Some(turn_id) = turn_id else {
                return Ok(vec![TimelineLine::new(
                    "warning",
                    "no turn to interrupt",
                    true,
                )]);
            };
            let payload = self.client.interrupt_turn(&turn_id)?;
            self.remember_payload_events(&payload);
            return Ok(Vec::new());
        }
        Ok(vec![TimelineLine::new(
            "status",
            "commands: /sessions [query], /resume <id>, /transcript [limit], /rename <title>, /new, /fork, /children, /parent, /archive, /delete, /share, /unshare, /compact, /status, /files [query], /attach <path[:range]>, /models [id], /agents, /agent <id>, /variant <name>, /thinking <level>, /themes [name], /config, /keybinds, /interrupt [turn_id], /allow, /deny, /answer, /dismiss, /exit",
            false,
        )])
    }

    fn handle_approval_response(&mut self, payload: &Value) -> Result<Vec<TimelineLine>, String> {
        let response = self.client.respond_approval(payload)?;
        self.remember_payload_events(&response);
        Ok(Vec::new())
    }

    fn handle_question_response(&mut self, payload: &Value) -> Result<Vec<TimelineLine>, String> {
        let response = self.client.respond_question(payload)?;
        self.remember_payload_events(&response);
        Ok(Vec::new())
    }
}

fn session_list_lines(sessions: &[Value]) -> Vec<TimelineLine> {
    let mut lines = vec![TimelineLine::new(
        "status",
        format!("remote sessions: {}", sessions.len()),
        false,
    )];
    lines.extend(sessions.iter().take(20).map(|session| {
        let id = session_id_from_payload(session).unwrap_or_else(|| "<unknown>".to_string());
        let status = session
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let messages = session
            .get("message_count")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        let workspace = session
            .get("workspace")
            .and_then(Value::as_str)
            .unwrap_or(".");
        TimelineLine::new(
            "status",
            format!("{id}  status={status}  messages={messages}  workspace={workspace}"),
            false,
        )
    }));
    lines
}

fn filter_agents_for_picker(agents: &[Value], query: &str) -> Vec<Value> {
    let query = query.trim().to_ascii_lowercase();
    agents
        .iter()
        .filter(|agent| query.is_empty() || agent_matches_query(agent, &query))
        .cloned()
        .collect()
}

fn agent_matches_query(agent: &Value, query: &str) -> bool {
    ["id", "name", "description"].iter().any(|key| {
        agent
            .get(*key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase()
            .contains(query)
    })
}

fn agent_picker_label(agent: &Value) -> String {
    let id = agent.get("id").and_then(Value::as_str).unwrap_or("agent");
    let name = agent.get("name").and_then(Value::as_str).unwrap_or(id);
    let description = agent
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let default = if agent
        .get("default")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        "  default"
    } else {
        ""
    };
    if description.is_empty() {
        format!("{id} - {name}{default}")
    } else {
        format!("{id} - {name}: {description}{default}")
    }
}

fn filter_models_for_picker(models: &[Value], query: &str) -> Vec<Value> {
    let query = query.trim().to_ascii_lowercase();
    models
        .iter()
        .filter(|model| query.is_empty() || model_matches_query(model, &query))
        .cloned()
        .collect()
}

fn model_matches_query(model: &Value, query: &str) -> bool {
    ["id", "name", "provider_id"].iter().any(|key| {
        model
            .get(*key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase()
            .contains(query)
    })
}

fn model_picker_label(model: &Value) -> String {
    let provider = model
        .get("provider_id")
        .and_then(Value::as_str)
        .unwrap_or("provider");
    let id = model.get("id").and_then(Value::as_str).unwrap_or("model");
    let name = model.get("name").and_then(Value::as_str).unwrap_or(id);
    let default = if model
        .get("default")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        "  default"
    } else {
        ""
    };
    format!("{provider}/{id} - {name}{default}")
}

fn choice_picker_values_from_models(payload: &Value, kind: ChoicePickerKind) -> Vec<String> {
    let key = match kind {
        ChoicePickerKind::Theme => return default_theme_names(),
        ChoicePickerKind::Variant => "variants",
        ChoicePickerKind::Thinking => "thinking",
    };
    payload
        .get(key)
        .and_then(Value::as_array)
        .map(|items| string_array(items))
        .filter(|items| !items.is_empty())
        .unwrap_or_else(|| default_choice_picker_values(kind))
}

fn default_choice_picker_values(kind: ChoicePickerKind) -> Vec<String> {
    match kind {
        ChoicePickerKind::Theme => default_theme_names(),
        ChoicePickerKind::Variant => ["default", "fast", "balanced", "deep"]
            .into_iter()
            .map(ToString::to_string)
            .collect(),
        ChoicePickerKind::Thinking => ["off", "low", "medium", "high"]
            .into_iter()
            .map(ToString::to_string)
            .collect(),
    }
}

fn filter_choice_picker_values(choices: &[String], query: &str) -> Vec<String> {
    let query = query.trim().to_ascii_lowercase();
    choices
        .iter()
        .filter(|choice| query.is_empty() || choice.to_ascii_lowercase().contains(&query))
        .cloned()
        .collect()
}

fn session_picker_label(session: &Value) -> String {
    let id = session_id_from_payload(session).unwrap_or_else(|| "<unknown>".to_string());
    let title = session
        .get("title")
        .or_else(|| {
            session
                .get("metadata")
                .and_then(|metadata| metadata.get("title"))
        })
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or("untitled");
    let status = session
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let messages = session
        .get("message_count")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let workspace = session
        .get("workspace")
        .and_then(Value::as_str)
        .unwrap_or(".");
    format!("{id}  {title}  status={status}  messages={messages}  workspace={workspace}")
}

fn transcript_lines(payload: &Value) -> Vec<TimelineLine> {
    let messages = payload
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let total = payload
        .get("message_count")
        .and_then(Value::as_u64)
        .unwrap_or(messages.len() as u64);
    let limit = payload
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(messages.len() as u64);
    let mut lines = vec![TimelineLine::new(
        "status",
        format!(
            "transcript: {} of {total} message(s), limit={limit}",
            messages.len()
        ),
        false,
    )];
    if messages.is_empty() {
        lines.push(TimelineLine::new("status", "transcript: empty", false));
        return lines;
    }
    lines.extend(messages.iter().map(|message| {
        let index = message
            .get("index")
            .and_then(Value::as_u64)
            .map(|value| format!("#{value} "))
            .unwrap_or_default();
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("message");
        let content = message
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        TimelineLine::new(
            "message",
            format!("{index}{role}: {}", clip_chars(&content, 220)),
            false,
        )
    }));
    lines
}

fn model_list_lines(payload: &Value) -> Vec<TimelineLine> {
    let models = payload
        .get("models")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if models.is_empty() {
        return vec![TimelineLine::new("warning", "remote models: none", true)];
    }
    models
        .into_iter()
        .map(|model| {
            let provider = model
                .get("provider_id")
                .and_then(Value::as_str)
                .unwrap_or("provider");
            let id = model.get("id").and_then(Value::as_str).unwrap_or("model");
            let name = model.get("name").and_then(Value::as_str).unwrap_or(id);
            TimelineLine::new("status", format!("{provider}/{id} - {name}"), false)
        })
        .collect()
}

fn agent_list_lines(payload: &Value) -> Vec<TimelineLine> {
    let agents = payload
        .get("agents")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if agents.is_empty() {
        return vec![TimelineLine::new("warning", "remote agents: none", true)];
    }
    agents
        .into_iter()
        .map(|agent| {
            let id = agent.get("id").and_then(Value::as_str).unwrap_or("agent");
            let name = agent.get("name").and_then(Value::as_str).unwrap_or(id);
            let description = agent
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or_default();
            TimelineLine::new("status", format!("{id} - {name}: {description}"), false)
        })
        .collect()
}

fn file_picker_lines(query: &str, matches: &[FilePickerMatch]) -> Vec<TimelineLine> {
    if matches.is_empty() {
        let suffix = if query.trim().is_empty() {
            String::new()
        } else {
            format!(" for `{}`", query.trim())
        };
        return vec![TimelineLine::new(
            "warning",
            format!("files: no matches{suffix}"),
            true,
        )];
    }
    let mut lines = vec![TimelineLine::new(
        "status",
        format!(
            "files: {} match(es){}",
            matches.len(),
            if query.trim().is_empty() {
                String::new()
            } else {
                format!(" for `{}`", query.trim())
            }
        ),
        false,
    )];
    lines.extend(matches.iter().enumerate().map(|(index, item)| {
        TimelineLine::new(
            "status",
            format!(
                "{}. {}  {}",
                index + 1,
                item.reference,
                if is_image_path(&item.path) {
                    "image"
                } else {
                    "file"
                }
            ),
            false,
        )
    }));
    lines
}

fn initialize_openagent_project_files(workspace: &Path) -> Result<Vec<String>, String> {
    let mut created = Vec::new();
    let agents_path = workspace.join("AGENTS.md");
    if !agents_path.exists() {
        fs::write(
            &agents_path,
            "# OpenAgent Project Guide\n\n- Use the existing project style.\n- Run focused tests after meaningful changes.\n- Keep unrelated worktree changes intact.\n",
        )
        .map_err(|error| error.to_string())?;
        created.push("AGENTS.md".to_string());
    }
    let config_dir = workspace.join(".openagent");
    fs::create_dir_all(&config_dir).map_err(|error| error.to_string())?;
    let tui_config = config_dir.join("tui.jsonc");
    if !tui_config.exists() {
        fs::write(
            &tui_config,
            "{\n  // OpenAgent TUI settings\n  \"theme\": \"default\",\n  \"leader_key\": \"\\\\\",\n  \"mouse\": true,\n  \"scroll\": 5,\n  \"diff_style\": \"unified\",\n  \"attention_notifications\": true,\n  \"sounds\": false,\n  \"keybinds\": {\n    \"editor\": \"ctrl+e\",\n    \"stash\": \"ctrl+s\",\n    \"unstash\": \"ctrl+y\"\n  }\n}\n",
        )
        .map_err(|error| error.to_string())?;
        created.push(".openagent/tui.jsonc".to_string());
    }
    if created.is_empty() {
        created.push("already up to date".to_string());
    }
    Ok(created)
}

#[derive(Clone, Debug, PartialEq)]
struct ExpandedPrompt {
    prompt: String,
    lines: Vec<TimelineLine>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FileAttachmentRef {
    query: String,
    range: Option<(usize, usize)>,
}

fn expand_file_attachments(workspace: &Path, prompt: &str) -> ExpandedPrompt {
    let refs = prompt
        .split_whitespace()
        .filter_map(parse_file_attachment_ref)
        .take(8)
        .collect::<Vec<_>>();
    if refs.is_empty() {
        return ExpandedPrompt {
            prompt: prompt.to_string(),
            lines: Vec::new(),
        };
    }
    let mut rendered_prompt = prompt.to_string();
    let mut lines = Vec::new();
    for reference in refs {
        let Some(path) = resolve_attachment_path(workspace, &reference.query) else {
            lines.push(TimelineLine::new(
                "warning",
                format!("attachment not found: @{}", reference.query),
                true,
            ));
            continue;
        };
        match render_attachment(workspace, &path, reference.range) {
            Ok(section) => {
                rendered_prompt.push_str("\n\n");
                rendered_prompt.push_str(&section.prompt_section);
                lines.push(TimelineLine::new(
                    "status",
                    format!("attached {}", section.label),
                    false,
                ));
            }
            Err(error) => lines.push(TimelineLine::new("warning", error, true)),
        }
    }
    ExpandedPrompt {
        prompt: rendered_prompt,
        lines,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AttachmentSection {
    label: String,
    prompt_section: String,
}

fn parse_file_attachment_ref(token: &str) -> Option<FileAttachmentRef> {
    let raw = token.strip_prefix('@')?;
    if raw.is_empty() || raw.starts_with('@') {
        return None;
    }
    let trimmed = raw.trim_matches(|ch: char| matches!(ch, ',' | ';' | ')' | '(' | '"' | '\''));
    if trimmed.is_empty() || trimmed.contains("://") {
        return None;
    }
    let (query, range) = if let Some((path, suffix)) = trimmed.split_once("#L") {
        (path.to_string(), parse_line_range(suffix))
    } else if let Some((path, suffix)) = trimmed.rsplit_once(':') {
        if suffix.chars().all(|ch| ch.is_ascii_digit() || ch == '-')
            && suffix.chars().any(|ch| ch.is_ascii_digit())
        {
            (path.to_string(), parse_line_range(suffix))
        } else {
            (trimmed.to_string(), None)
        }
    } else {
        (trimmed.to_string(), None)
    };
    (!query.is_empty()).then_some(FileAttachmentRef { query, range })
}

fn parse_line_range(value: &str) -> Option<(usize, usize)> {
    let (start, end) = value
        .split_once('-')
        .map_or((value, value), |(start, end)| (start, end));
    let start = start.parse::<usize>().ok()?.max(1);
    let end = end.parse::<usize>().ok().unwrap_or(start).max(start);
    Some((start, end.min(start + 400)))
}

fn resolve_attachment_path(workspace: &Path, query: &str) -> Option<PathBuf> {
    let raw = PathBuf::from(query);
    let exact = if raw.is_absolute() {
        raw
    } else {
        workspace.join(&raw)
    };
    if exact.is_file() {
        return Some(exact);
    }
    fuzzy_find_files(workspace, query, 1)
        .into_iter()
        .next()
        .map(|item| item.path)
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FilePickerMatch {
    path: PathBuf,
    reference: String,
    score: usize,
}

fn fuzzy_find_files(workspace: &Path, query: &str, limit: usize) -> Vec<FilePickerMatch> {
    let query = query
        .trim()
        .trim_start_matches('@')
        .trim_matches(|ch: char| matches!(ch, '"' | '\'' | '`'))
        .to_ascii_lowercase();
    let mut stack = vec![workspace.to_path_buf()];
    let mut matches = Vec::new();
    let mut visited = 0_usize;
    while let Some(path) = stack.pop() {
        visited += 1;
        if visited > 5000 {
            break;
        }
        if path.is_dir() {
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or_default();
            if matches!(
                name,
                ".git" | "node_modules" | "target" | "dist" | "build" | ".venv" | "__pycache__"
            ) {
                continue;
            }
            if let Ok(entries) = fs::read_dir(&path) {
                for entry in entries.flatten() {
                    stack.push(entry.path());
                }
            }
            continue;
        }
        if !path.is_file() {
            continue;
        }
        let relative = relative_display_path(workspace, &path);
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        if let Some(score) = fuzzy_file_score(&relative, name, &query) {
            let Some(reference) = normalize_attachment_reference_token(&relative) else {
                continue;
            };
            matches.push(FilePickerMatch {
                path,
                reference,
                score,
            });
        }
    }
    matches.sort_by(|left, right| {
        left.score
            .cmp(&right.score)
            .then_with(|| left.reference.len().cmp(&right.reference.len()))
            .then_with(|| left.reference.cmp(&right.reference))
    });
    matches.truncate(limit);
    matches
}

fn composer_candidate_from_match(item: FilePickerMatch) -> ComposerFileCandidate {
    let kind = if is_image_path(&item.path) {
        "image"
    } else {
        "file"
    };
    ComposerFileCandidate {
        reference: item.reference,
        kind: kind.to_string(),
    }
}

fn fuzzy_file_score(relative: &str, name: &str, query: &str) -> Option<usize> {
    let relative = relative.to_ascii_lowercase();
    let name = name.to_ascii_lowercase();
    if query.is_empty() {
        return Some(100 + relative.matches('/').count());
    }
    if relative == query {
        Some(0)
    } else if name == query {
        Some(1)
    } else if relative.ends_with(query) {
        Some(2)
    } else if name.contains(query) {
        Some(3)
    } else if relative.contains(query) {
        Some(4)
    } else if fuzzy_subsequence(&relative, query) {
        Some(10 + relative.len().saturating_sub(query.len()))
    } else {
        None
    }
}

fn fuzzy_subsequence(value: &str, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let mut query_chars = query.chars();
    let Some(mut expected) = query_chars.next() else {
        return true;
    };
    for ch in value.chars() {
        if ch == expected {
            match query_chars.next() {
                Some(next) => expected = next,
                None => return true,
            }
        }
    }
    false
}

fn attachment_reference_from_parts(
    path: &str,
    line: Option<usize>,
    start: Option<usize>,
    end: Option<usize>,
) -> String {
    let mut reference = path.trim().trim_start_matches('@').to_string();
    if let Some(line) = line.filter(|value| *value > 0) {
        reference.push_str(&format!(":{line}"));
    } else if let Some(start) = start.filter(|value| *value > 0) {
        let end = end.unwrap_or(start).max(start);
        if end == start {
            reference.push_str(&format!(":{start}"));
        } else {
            reference.push_str(&format!(":{start}-{end}"));
        }
    }
    reference
}

fn normalize_attachment_reference_token(reference: &str) -> Option<String> {
    let reference = reference
        .trim()
        .trim_start_matches('@')
        .trim_matches(|ch: char| matches!(ch, ',' | ';' | ')' | '(' | '"' | '\'' | '`'));
    if reference.is_empty()
        || reference.contains("://")
        || reference.chars().any(char::is_whitespace)
    {
        return None;
    }
    Some(format!("@{reference}"))
}

fn render_attachment(
    workspace: &Path,
    path: &Path,
    range: Option<(usize, usize)>,
) -> Result<AttachmentSection, String> {
    let label = match range {
        Some((start, end)) if start == end => {
            format!("{}:{start}", relative_display_path(workspace, path))
        }
        Some((start, end)) => format!("{}:{start}-{end}", relative_display_path(workspace, path)),
        None => relative_display_path(workspace, path),
    };
    if is_image_path(path) {
        let bytes = fs::metadata(path).map_err(|error| error.to_string())?.len();
        return Ok(AttachmentSection {
            label: label.clone(),
            prompt_section: format!("Attached image: {label}\n\n(binary image, {bytes} bytes)"),
        });
    }
    let mut content = fs::read_to_string(path)
        .map_err(|error| format!("failed to attach {}: {error}", path.display()))?;
    if let Some((start, end)) = range {
        let lines = content.lines().collect::<Vec<_>>();
        let start_index = start.saturating_sub(1).min(lines.len());
        let end_index = end.min(lines.len());
        content = lines[start_index..end_index].join("\n");
    }
    if content.len() > 24_000 {
        let mut end = 24_000.min(content.len());
        while end > 0 && !content.is_char_boundary(end) {
            end -= 1;
        }
        content.truncate(end);
        content.push_str("\n... attachment truncated ...");
    }
    Ok(AttachmentSection {
        label: label.clone(),
        prompt_section: format!("Attached file: {label}\n\n```text\n{content}\n```"),
    })
}

fn is_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .map(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg"
            )
        })
        .unwrap_or(false)
}

fn relative_display_path(workspace: &Path, path: &Path) -> String {
    path.strip_prefix(workspace)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn diff_detail_lines(payload: &Value) -> Vec<TimelineLine> {
    let undo_count = payload
        .get("undo_count")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let redo_count = payload
        .get("redo_count")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let latest = payload.get("latest").cloned().unwrap_or(Value::Null);
    let mut lines = vec![TimelineLine::new(
        "patch",
        format!(
            "diff stack: undo={undo_count} redo={redo_count}{}{}",
            if undo_count > 0 { "  /undo" } else { "" },
            if redo_count > 0 { "  /redo" } else { "" }
        ),
        false,
    )];
    if latest.is_object() {
        lines.extend(patch_lines("latest patch", &latest, false));
    } else {
        lines.push(TimelineLine::new("status", "latest patch: none", false));
    }
    lines
}

fn patch_result_lines(action: &str, payload: &Value) -> Vec<TimelineLine> {
    let patch = payload.get("patch").cloned().unwrap_or(Value::Null);
    if patch.is_object() {
        patch_lines(action, &patch, true)
    } else {
        vec![TimelineLine::new(
            "warning",
            format!("{action}: {}", compact_json(payload)),
            true,
        )]
    }
}

fn patch_lines(label: &str, patch: &Value, highlight: bool) -> Vec<TimelineLine> {
    let path = patch
        .get("path")
        .and_then(Value::as_str)
        .unwrap_or("<unknown>");
    let id = patch.get("id").and_then(Value::as_str).unwrap_or("-");
    let status = patch
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("applied");
    let mut lines = vec![TimelineLine::new(
        "patch",
        format!("{label}: {path} ({status}, {id})  actions: /undo /redo"),
        highlight,
    )];
    if let Some(diff) = patch.get("diff").and_then(Value::as_str) {
        if !diff.trim().is_empty() {
            lines.push(TimelineLine::new("diff-meta", "diff:", false));
            lines.extend(trim_lines(diff, 80).into_iter().map(|line| {
                let kind = rendered_diff_line_kind(&line);
                TimelineLine::new(kind, line, false)
            }));
        }
    }
    lines
}

fn rendered_diff_line_kind(line: &str) -> &'static str {
    if line.starts_with("@@") {
        "diff-hunk"
    } else if line.starts_with("+++") || line.starts_with("---") {
        "diff-meta"
    } else if line.starts_with('+') {
        "diff-add"
    } else if line.starts_with('-') {
        "diff-del"
    } else {
        "diff"
    }
}

fn event_identity_key(event: &Value) -> String {
    let method = event
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("event");
    let params = event.get("params").cloned().unwrap_or(Value::Null);
    let turn_id = params
        .get("turn_id")
        .or_else(|| {
            params
                .get("approval")
                .and_then(|value| value.get("turn_id"))
        })
        .and_then(Value::as_str)
        .unwrap_or("-");
    let request_id = params
        .get("request_id")
        .or_else(|| {
            params
                .get("approval")
                .and_then(|value| value.get("request_id"))
        })
        .or_else(|| {
            params
                .get("question")
                .and_then(|value| value.get("request_id"))
        })
        .and_then(Value::as_str)
        .unwrap_or("-");
    let sequence = event
        .get("global_sequence")
        .or_else(|| event.get("sequence"))
        .and_then(Value::as_u64)
        .unwrap_or_default();
    if sequence > 0 {
        format!("{method}:{turn_id}:{request_id}:{sequence}")
    } else {
        format!("{method}:{turn_id}:{request_id}:{}", compact_json(event))
    }
}

#[must_use]
pub fn tui_control_fixture() -> Value {
    let mut state = TuiState::new();
    let requests = vec![
        json!({"path": "/tui/append-prompt", "body": {"text": "hello"}}),
        json!({"path": "/tui/publish", "body": {"type": "tui.prompt.append", "properties": {"text": " next"}}}),
        json!({"path": "/tui/show-toast", "body": {"title": "Saved", "message": "Session selected", "variant": "success"}}),
        json!({"path": "/tui/execute-command", "body": {"command": "help"}}),
        json!({"path": "/tui/open-themes", "body": {}}),
        json!({"path": "/tui/clear-prompt", "body": {}}),
    ];
    let steps = requests
        .into_iter()
        .map(|request| {
            let result = state.apply_control_request(&request);
            json!({
                "request": request,
                "result": result,
                "status": state.status,
                "input_buffer": state.input_buffer,
                "timeline": state.timeline,
            })
        })
        .collect::<Vec<_>>();
    let mut invalid_state = TuiState::new();
    let invalid_select =
        invalid_state.apply_control_request(&json!({"path": "/tui/select-session", "body": {}}));

    json!({
        "action_map": action_map_fixture(),
        "steps": steps,
        "invalid_select": {
            "result": invalid_select,
            "status": invalid_state.status,
            "timeline": invalid_state.timeline,
        },
    })
}

fn action_map_fixture() -> Value {
    let mut object = Map::new();
    for name in [
        "append-prompt",
        "submit-prompt",
        "clear-prompt",
        "open-help",
        "open-sessions",
        "open-themes",
        "open-models",
        "open-agents",
        "open-variants",
        "open-thinking",
        "open-palette",
        "open-files",
        "select-session",
        "rename-session",
        "archive-session",
        "unarchive-session",
        "delete-session",
        "fork-session",
        "session-children",
        "parent-session",
        "share-session",
        "unshare-session",
        "compact-session",
        "session-details",
        "undo-session",
        "redo-session",
        "select-model",
        "select-agent",
        "select-variant",
        "select-thinking",
        "select-theme",
        "select-file",
        "attach-file",
        "show-toast",
        "execute-command",
        "execute-palette",
        "custom.action",
    ] {
        object.insert(
            name.to_string(),
            Value::String(normalize_control_action(name)),
        );
    }
    Value::Object(object)
}

fn object_value(value: Option<&Value>) -> Value {
    value
        .and_then(Value::as_object)
        .cloned()
        .map(Value::Object)
        .unwrap_or_else(|| json!({}))
}

trait IfEmptyThen {
    fn if_empty_then<F>(self, fallback: F) -> String
    where
        F: FnOnce() -> String;
}

impl IfEmptyThen for String {
    fn if_empty_then<F>(self, fallback: F) -> String
    where
        F: FnOnce() -> String,
    {
        if self.is_empty() { fallback() } else { self }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use std::{
        error::Error,
        fs,
        io::{ErrorKind, Read, Write},
        net::{TcpListener, TcpStream},
        path::PathBuf,
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, Ordering},
        },
        thread,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn exposes_command_boundary() {
        assert_eq!(crate_name(), "openagent-tui");
        assert_eq!(command_name(), "openagent-tui");
        assert_eq!(client_crate_name(), "openagent-app-server-client");
        assert_eq!(server_crate_name(), "openagent-app-server");
    }

    #[test]
    fn approval_events_render_diff_preview_and_support_allow_always() {
        let mut state = TuiState::new();
        let applied = state.apply_app_event(&json!({
            "method": "turn/approval_requested",
            "params": {
                "approval": {
                    "request_id": "approval_1",
                    "turn_id": "turn_1",
                    "session_id": "session_1",
                    "tool_name": "write",
                    "tool_input": {"file_path": "src/lib.rs"},
                    "preview": {
                        "kind": "file",
                        "path": "src/lib.rs",
                        "status": "modified",
                        "diff": "--- a/src/lib.rs\n+++ b/src/lib.rs\n+hello"
                    }
                }
            }
        }));

        assert_eq!(applied["applied"], json!(true));
        assert_eq!(state.status, "approval pending");
        let timeline = state
            .timeline
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(timeline.contains("approval required: write"));
        assert!(timeline.contains("diff:"));
        assert!(timeline.contains("+hello"));

        let response = state.respond_active_approval("allow", Some("always"), None);

        assert_eq!(response["applied"], json!(true));
        assert_eq!(response["payload"]["action"], json!("allow"));
        assert_eq!(response["payload"]["scope"], json!("always"));
        assert_eq!(response["payload"]["request_id"], json!("approval_1"));
        assert!(state.active_approval.is_none());
    }

    #[test]
    fn patch_events_render_structured_diff_and_undo_redo_markers() {
        let mut state = TuiState::new();
        let applied = state.apply_app_event(&json!({
            "method": "patch/detected",
            "params": {
                "session_id": "session_1",
                "turn_id": "turn_1",
                "patch": {
                    "id": "patch_1",
                    "path": "src/lib.rs",
                    "status": "modified",
                    "diff": "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,1 +1,1 @@\n-old\n+new"
                }
            }
        }));

        assert_eq!(applied["applied"], json!(true));
        assert_eq!(state.status, "patch detected");
        let kinds = state
            .timeline
            .iter()
            .map(|line| line.kind.as_str())
            .collect::<Vec<_>>();
        assert!(kinds.contains(&"patch"));
        assert!(kinds.contains(&"diff-meta"));
        assert!(kinds.contains(&"diff-hunk"));
        assert!(kinds.contains(&"diff-del"));
        assert!(kinds.contains(&"diff-add"));
        assert!(
            state
                .timeline
                .iter()
                .any(|line| line.text.contains("actions: /undo /redo"))
        );

        let detail_lines = diff_detail_lines(&json!({
            "undo_count": 1,
            "redo_count": 1,
            "latest": {
                "id": "patch_1",
                "path": "src/lib.rs",
                "status": "modified",
                "diff": "+new"
            }
        }));
        assert_eq!(detail_lines[0].kind, "patch");
        assert!(detail_lines[0].text.contains("/undo"));
        assert!(detail_lines[0].text.contains("/redo"));
        assert!(detail_lines.iter().any(|line| line.kind == "diff-add"));
    }

    #[test]
    fn approval_can_be_denied_with_note_from_command() {
        let mut state = TuiState::new();
        state.apply_app_event(&json!({
            "method": "turn/approval_requested",
            "params": {
                "approval": {
                    "request_id": "approval_2",
                    "turn_id": "turn_1",
                    "tool_name": "bash",
                    "tool_input": {"command": "rm -rf target"}
                }
            }
        }));
        state.input_buffer = "/deny risky command".to_string();

        assert!(!state.submit());

        let response = state.approval_responses.last().expect("approval response");
        assert_eq!(response["action"], json!("deny"));
        assert_eq!(response["note"], json!("risky command"));
        assert_eq!(state.status, "approval resolved");
    }

    #[test]
    fn question_events_support_answer_and_dismiss() {
        let mut state = TuiState::new();
        let question_event = json!({
            "method": "item/question/requested",
            "params": {
                "event": {
                    "type": "question-request",
                    "request_id": "question_1",
                    "turn_id": "turn_1",
                    "session_id": "session_1",
                    "tool_call_id": "call_question",
                    "questions": [{
                        "header": "Plan",
                        "question": "Which option?",
                        "options": [
                            {"label": "Fast path", "description": "Move quickly"},
                            {"label": "Safe path", "description": "Be conservative"}
                        ]
                    }]
                }
            }
        });

        assert_eq!(
            state.apply_app_event(&question_event)["applied"],
            json!(true)
        );
        assert_eq!(state.status, "question pending");
        assert!(
            state
                .timeline
                .iter()
                .any(|line| line.text.contains("Fast path"))
        );

        state.input_buffer = "/answer Safe path".to_string();
        assert!(!state.submit());

        let response = state.question_responses.last().expect("question response");
        assert_eq!(response["answers"], json!([["Safe path"]]));
        assert_eq!(response["request_id"], json!("question_1"));
        assert!(state.active_question.is_none());

        state.apply_app_event(&question_event);
        let dismissed = state.dismiss_active_question(Some("not needed"));
        assert_eq!(dismissed["payload"]["dismissed"], json!(true));
        assert_eq!(dismissed["payload"]["note"], json!("not needed"));
    }

    #[test]
    fn control_requests_can_respond_to_active_interactions() {
        let mut state = TuiState::new();
        state.apply_app_event(&json!({
            "method": "turn/approval_requested",
            "params": {"approval": {"request_id": "approval_3", "tool_name": "write"}}
        }));

        let approval = state.apply_control_request(&json!({
            "path": "/tui/respond-approval",
            "body": {"action": "allow_always"}
        }));

        assert_eq!(approval["applied"], json!(true));
        assert_eq!(approval["payload"]["scope"], json!("always"));

        state.apply_app_event(&json!({
            "method": "item/question/requested",
            "params": {"event": {"request_id": "question_2", "questions": [{"question": "Mode?"}]}}
        }));
        let answer = state.apply_control_request(&json!({
            "path": "/tui/reply-question",
            "body": {"answer": "Fast"}
        }));

        assert_eq!(answer["applied"], json!(true));
        assert_eq!(answer["payload"]["answers"], json!([["Fast"]]));
    }

    #[test]
    fn control_requests_open_model_theme_and_palette_surfaces() {
        let mut state = TuiState::new();
        let models = state.apply_control_request(&json!({
            "path": "/tui/open-models",
            "body": {"models": [{"id": "gpt-test", "name": "GPT Test"}]}
        }));
        assert_eq!(models["applied"], json!(true));
        assert_eq!(state.status, "model picker");
        assert!(
            state
                .timeline
                .iter()
                .any(|line| line.text.contains("gpt-test"))
        );

        let theme = state.apply_control_request(&json!({
            "path": "/tui/select-theme",
            "body": {"theme": "midnight"}
        }));
        assert_eq!(theme["applied"], json!(true));
        assert_eq!(state.config.theme, "midnight");

        let themes = state.apply_control_request(&json!({
            "path": "/tui/open-themes",
            "body": {"themes": ["default", "midnight", "high-contrast"]}
        }));
        assert_eq!(themes["applied"], json!(true));
        assert_eq!(state.status, "theme picker");
        let picker = state.choice_picker.as_ref().expect("theme picker");
        assert_eq!(picker.kind, ChoicePickerKind::Theme);
        assert_eq!(
            picker.candidates,
            vec![
                "default".to_string(),
                "midnight".to_string(),
                "high-contrast".to_string()
            ]
        );

        let palette = state.apply_control_request(&json!({
            "path": "/tui/open-palette",
            "body": {"query": "model"}
        }));
        assert_eq!(palette["applied"], json!(true));
        assert_eq!(state.status, "palette open");
        assert!(
            state
                .timeline
                .iter()
                .any(|line| line.text.contains("/models"))
        );

        let agents = state.apply_control_request(&json!({
            "path": "/tui/open-agents",
            "body": {"agents": [{"id": "reviewer", "name": "Reviewer", "description": "Review code"}]}
        }));
        assert_eq!(agents["applied"], json!(true));
        assert_eq!(state.status, "agent picker");
        assert!(
            state
                .timeline
                .iter()
                .any(|line| line.text.contains("reviewer"))
        );

        let variants = state.apply_control_request(&json!({
            "path": "/tui/open-variants",
            "body": {"variants": ["default", "deep"]}
        }));
        assert_eq!(variants["applied"], json!(true));
        assert_eq!(state.status, "variant picker");
        assert!(
            variants["variants"]
                .as_array()
                .is_some_and(|items| items.len() == 2)
        );

        let thinking = state.apply_control_request(&json!({
            "path": "/tui/open-thinking",
            "body": {"levels": ["low", "high"]}
        }));
        assert_eq!(thinking["applied"], json!(true));
        assert_eq!(state.status, "thinking picker");
        assert!(
            thinking["levels"]
                .as_array()
                .is_some_and(|items| items.len() == 2)
        );
    }

    #[test]
    fn remote_control_select_model_dispatches_handler_command() {
        #[derive(Default)]
        struct CaptureHandler {
            commands: Vec<String>,
            responses: Vec<Value>,
        }

        impl TerminalEventHandler for CaptureHandler {
            fn handle_submit(&mut self, _prompt: &str) -> Result<Vec<TimelineLine>, String> {
                Ok(Vec::new())
            }

            fn handle_command(&mut self, command: &str) -> Result<Vec<TimelineLine>, String> {
                self.commands.push(command.to_string());
                Ok(vec![TimelineLine::new(
                    "status",
                    "command dispatched",
                    true,
                )])
            }

            fn record_control_response(&mut self, payload: &Value) -> Result<(), String> {
                self.responses.push(payload.clone());
                Ok(())
            }
        }

        let mut state = TuiState::new();
        let mut handler = CaptureHandler::default();
        handle_remote_control_request(
            &mut state,
            &mut handler,
            &json!({"path": "/tui/select-model", "body": {"model": "gpt-test"}}),
        );

        assert_eq!(handler.commands, vec!["/models gpt-test".to_string()]);
        assert_eq!(handler.responses.len(), 1);
        assert_eq!(handler.responses[0]["ok"], json!(true));
        assert!(
            state
                .timeline
                .iter()
                .any(|line| line.text.contains("command dispatched"))
        );
    }

    #[test]
    fn remote_control_open_models_dispatches_picker_fetch() {
        #[derive(Default)]
        struct CaptureHandler {
            model_fetches: usize,
            responses: Vec<Value>,
        }

        impl TerminalEventHandler for CaptureHandler {
            fn handle_submit(&mut self, _prompt: &str) -> Result<Vec<TimelineLine>, String> {
                Ok(Vec::new())
            }

            fn handle_command(&mut self, _command: &str) -> Result<Vec<TimelineLine>, String> {
                Ok(Vec::new())
            }

            fn list_models(&mut self) -> Result<Value, String> {
                self.model_fetches += 1;
                Ok(json!({
                    "models": [
                        {"id": "server-local", "provider_id": "openagent", "name": "Server Local"},
                        {"id": "deep-model", "provider_id": "openagent", "name": "Deep Model"}
                    ]
                }))
            }

            fn record_control_response(&mut self, payload: &Value) -> Result<(), String> {
                self.responses.push(payload.clone());
                Ok(())
            }
        }

        let mut state = TuiState::new();
        let mut handler = CaptureHandler::default();
        handle_remote_control_request(
            &mut state,
            &mut handler,
            &json!({"path": "/tui/open-models", "body": {}}),
        );

        assert_eq!(handler.model_fetches, 1);
        assert_eq!(
            state
                .model_picker
                .as_ref()
                .expect("model picker")
                .candidates
                .len(),
            2
        );
        assert_eq!(handler.responses.len(), 1);
        assert_eq!(handler.responses[0]["ok"], json!(true));
    }

    #[test]
    fn remote_control_open_agents_dispatches_picker_fetch() {
        #[derive(Default)]
        struct CaptureHandler {
            agent_fetches: usize,
            responses: Vec<Value>,
        }

        impl TerminalEventHandler for CaptureHandler {
            fn handle_submit(&mut self, _prompt: &str) -> Result<Vec<TimelineLine>, String> {
                Ok(Vec::new())
            }

            fn handle_command(&mut self, _command: &str) -> Result<Vec<TimelineLine>, String> {
                Ok(Vec::new())
            }

            fn list_agents(&mut self) -> Result<Value, String> {
                self.agent_fetches += 1;
                Ok(json!({
                    "agents": [
                        {"id": "server", "name": "Server", "description": "Default server agent"},
                        {"id": "reviewer", "name": "Reviewer", "description": "Review code"}
                    ]
                }))
            }

            fn record_control_response(&mut self, payload: &Value) -> Result<(), String> {
                self.responses.push(payload.clone());
                Ok(())
            }
        }

        let mut state = TuiState::new();
        let mut handler = CaptureHandler::default();
        handle_remote_control_request(
            &mut state,
            &mut handler,
            &json!({"path": "/tui/open-agents", "body": {}}),
        );

        assert_eq!(handler.agent_fetches, 1);
        assert_eq!(
            state
                .agent_picker
                .as_ref()
                .expect("agent picker")
                .candidates
                .len(),
            2
        );
        assert_eq!(handler.responses.len(), 1);
        assert_eq!(handler.responses[0]["ok"], json!(true));
    }

    #[test]
    fn remote_control_open_variant_and_thinking_dispatch_picker_fetch() {
        #[derive(Default)]
        struct CaptureHandler {
            model_fetches: usize,
            responses: Vec<Value>,
        }

        impl TerminalEventHandler for CaptureHandler {
            fn handle_submit(&mut self, _prompt: &str) -> Result<Vec<TimelineLine>, String> {
                Ok(Vec::new())
            }

            fn handle_command(&mut self, _command: &str) -> Result<Vec<TimelineLine>, String> {
                Ok(Vec::new())
            }

            fn list_models(&mut self) -> Result<Value, String> {
                self.model_fetches += 1;
                Ok(json!({
                    "models": [],
                    "variants": ["default", "deep"],
                    "thinking": ["low", "high"]
                }))
            }

            fn record_control_response(&mut self, payload: &Value) -> Result<(), String> {
                self.responses.push(payload.clone());
                Ok(())
            }
        }

        let mut state = TuiState::new();
        let mut handler = CaptureHandler::default();
        handle_remote_control_request(
            &mut state,
            &mut handler,
            &json!({"path": "/tui/open-variants", "body": {}}),
        );

        assert_eq!(handler.model_fetches, 1);
        let picker = state.choice_picker.as_ref().expect("variant picker");
        assert_eq!(picker.kind, ChoicePickerKind::Variant);
        assert_eq!(
            picker.candidates,
            vec!["default".to_string(), "deep".to_string()]
        );
        assert_eq!(handler.responses.len(), 1);
        assert_eq!(handler.responses[0]["ok"], json!(true));

        handle_remote_control_request(
            &mut state,
            &mut handler,
            &json!({"path": "/tui/open-thinking", "body": {}}),
        );

        assert_eq!(handler.model_fetches, 2);
        let picker = state.choice_picker.as_ref().expect("thinking picker");
        assert_eq!(picker.kind, ChoicePickerKind::Thinking);
        assert_eq!(
            picker.candidates,
            vec!["low".to_string(), "high".to_string()]
        );
        assert_eq!(handler.responses.len(), 2);
        assert_eq!(handler.responses[1]["ok"], json!(true));
    }

    #[test]
    fn remote_control_agent_variant_and_thinking_dispatch_handler_commands() {
        #[derive(Default)]
        struct CaptureHandler {
            commands: Vec<String>,
            responses: Vec<Value>,
        }

        impl TerminalEventHandler for CaptureHandler {
            fn handle_submit(&mut self, _prompt: &str) -> Result<Vec<TimelineLine>, String> {
                Ok(Vec::new())
            }

            fn handle_command(&mut self, command: &str) -> Result<Vec<TimelineLine>, String> {
                self.commands.push(command.to_string());
                Ok(vec![TimelineLine::new(
                    "status",
                    format!("handled {command}"),
                    true,
                )])
            }

            fn record_control_response(&mut self, payload: &Value) -> Result<(), String> {
                self.responses.push(payload.clone());
                Ok(())
            }
        }

        let mut state = TuiState::new();
        let mut handler = CaptureHandler::default();
        for request in [
            json!({"path": "/tui/select-agent", "body": {"agent": "reviewer"}}),
            json!({"path": "/tui/select-variant", "body": {"variant": "deep"}}),
            json!({"path": "/tui/select-thinking", "body": {"level": "high"}}),
            json!({"path": "/tui/publish", "body": {"type": "tui.agent.select", "properties": {"id": "server"}}}),
            json!({"path": "/tui/publish", "body": {"type": "tui.variant.select", "properties": {"value": "fast"}}}),
            json!({"path": "/tui/publish", "body": {"type": "tui.thinking.select", "properties": {"value": "low"}}}),
        ] {
            handle_remote_control_request(&mut state, &mut handler, &request);
        }

        assert_eq!(
            handler.commands,
            vec![
                "/agent reviewer",
                "/variant deep",
                "/thinking high",
                "/agent server",
                "/variant fast",
                "/thinking low",
            ]
        );
        assert_eq!(handler.responses.len(), handler.commands.len());
        assert!(
            handler
                .responses
                .iter()
                .all(|payload| payload["ok"] == json!(true))
        );
    }

    #[test]
    fn remote_control_file_picker_dispatches_and_selects_into_composer() {
        #[derive(Default)]
        struct CaptureHandler {
            commands: Vec<String>,
            responses: Vec<Value>,
            searches: Vec<String>,
        }

        impl TerminalEventHandler for CaptureHandler {
            fn handle_submit(&mut self, _prompt: &str) -> Result<Vec<TimelineLine>, String> {
                Ok(Vec::new())
            }

            fn handle_command(&mut self, command: &str) -> Result<Vec<TimelineLine>, String> {
                self.commands.push(command.to_string());
                Ok(vec![TimelineLine::new(
                    "status",
                    format!("handled {command}"),
                    true,
                )])
            }

            fn search_files(&mut self, query: &str) -> Result<Vec<ComposerFileCandidate>, String> {
                self.searches.push(query.to_string());
                Ok(vec![
                    ComposerFileCandidate {
                        reference: "@src/lib.rs".to_string(),
                        kind: "file".to_string(),
                    },
                    ComposerFileCandidate {
                        reference: "@src/main.rs".to_string(),
                        kind: "file".to_string(),
                    },
                ])
            }

            fn record_control_response(&mut self, payload: &Value) -> Result<(), String> {
                self.responses.push(payload.clone());
                Ok(())
            }
        }

        let mut state = TuiState::new();
        let mut handler = CaptureHandler::default();
        handle_remote_control_request(
            &mut state,
            &mut handler,
            &json!({"path": "/tui/open-files", "body": {"query": "main"}}),
        );
        assert!(handler.commands.is_empty());
        assert_eq!(handler.searches, vec!["main".to_string()]);
        assert_eq!(
            state
                .file_picker
                .as_ref()
                .expect("file picker")
                .candidates
                .len(),
            2
        );

        state.input_buffer = "review".to_string();
        handle_remote_control_request(
            &mut state,
            &mut handler,
            &json!({"path": "/tui/publish", "body": {"type": "tui.file.select", "properties": {"path": "src/main.rs", "line": 7}}}),
        );

        assert!(handler.commands.is_empty());
        assert!(state.file_picker.is_none());
        assert_eq!(state.input_buffer, "review @src/main.rs:7 ");
        assert_eq!(handler.responses.len(), 2);
        assert!(
            handler
                .responses
                .iter()
                .all(|payload| payload["ok"] == json!(true))
        );
    }

    #[test]
    fn remote_control_open_sessions_dispatches_picker_search() {
        #[derive(Default)]
        struct CaptureHandler {
            searches: Vec<String>,
            responses: Vec<Value>,
        }

        impl TerminalEventHandler for CaptureHandler {
            fn handle_submit(&mut self, _prompt: &str) -> Result<Vec<TimelineLine>, String> {
                Ok(Vec::new())
            }

            fn handle_command(&mut self, _command: &str) -> Result<Vec<TimelineLine>, String> {
                Ok(Vec::new())
            }

            fn search_sessions(&mut self, query: &str) -> Result<Vec<Value>, String> {
                self.searches.push(query.to_string());
                Ok(vec![json!({
                    "session_id": "session_alpha",
                    "title": "Alpha",
                    "status": "idle",
                    "message_count": 2,
                    "workspace": "/tmp/work"
                })])
            }

            fn record_control_response(&mut self, payload: &Value) -> Result<(), String> {
                self.responses.push(payload.clone());
                Ok(())
            }
        }

        let mut state = TuiState::new();
        let mut handler = CaptureHandler::default();
        handle_remote_control_request(
            &mut state,
            &mut handler,
            &json!({"path": "/tui/open-sessions", "body": {"query": "alp"}}),
        );

        assert_eq!(handler.searches, vec!["alp".to_string()]);
        assert_eq!(
            state
                .session_picker
                .as_ref()
                .expect("session picker")
                .candidates
                .len(),
            1
        );
        assert_eq!(handler.responses.len(), 1);
        assert_eq!(handler.responses[0]["ok"], json!(true));
    }

    #[test]
    fn remote_control_session_actions_dispatch_handler_commands() {
        #[derive(Default)]
        struct CaptureHandler {
            commands: Vec<String>,
            responses: Vec<Value>,
        }

        impl TerminalEventHandler for CaptureHandler {
            fn handle_submit(&mut self, _prompt: &str) -> Result<Vec<TimelineLine>, String> {
                Ok(Vec::new())
            }

            fn handle_command(&mut self, command: &str) -> Result<Vec<TimelineLine>, String> {
                self.commands.push(command.to_string());
                Ok(vec![TimelineLine::new(
                    "status",
                    format!("handled {command}"),
                    true,
                )])
            }

            fn record_control_response(&mut self, payload: &Value) -> Result<(), String> {
                self.responses.push(payload.clone());
                Ok(())
            }
        }

        let mut state = TuiState::new();
        let mut handler = CaptureHandler::default();
        for request in [
            json!({"path": "/tui/select-session", "body": {"sessionID": "session_existing"}}),
            json!({"path": "/tui/rename-session", "body": {"title": "New title"}}),
            json!({"path": "/tui/archive-session", "body": {}}),
            json!({"path": "/tui/unarchive-session", "body": {}}),
            json!({"path": "/tui/fork-session", "body": {}}),
            json!({"path": "/tui/session-children", "body": {}}),
            json!({"path": "/tui/share-session", "body": {}}),
            json!({"path": "/tui/unshare-session", "body": {}}),
            json!({"path": "/tui/compact-session", "body": {}}),
            json!({"path": "/tui/session-details", "body": {}}),
            json!({"path": "/tui/undo-session", "body": {}}),
            json!({"path": "/tui/redo-session", "body": {}}),
            json!({"path": "/tui/publish", "body": {"type": "tui.session.delete"}}),
        ] {
            handle_remote_control_request(&mut state, &mut handler, &request);
        }

        assert_eq!(
            handler.commands,
            vec![
                "/resume session_existing",
                "/rename New title",
                "/archive",
                "/unarchive",
                "/fork",
                "/children",
                "/share",
                "/unshare",
                "/compact",
                "/details",
                "/undo",
                "/redo",
                "/delete",
            ]
        );
        assert_eq!(handler.responses.len(), handler.commands.len());
        assert!(
            handler
                .responses
                .iter()
                .all(|payload| payload["ok"] == json!(true))
        );
    }

    #[test]
    fn local_interaction_commands_are_forwarded_to_terminal_handler() {
        #[derive(Default)]
        struct CaptureHandler {
            approvals: Vec<Value>,
            questions: Vec<Value>,
        }

        impl TerminalEventHandler for CaptureHandler {
            fn handle_submit(&mut self, _prompt: &str) -> Result<Vec<TimelineLine>, String> {
                Ok(Vec::new())
            }

            fn handle_command(&mut self, _command: &str) -> Result<Vec<TimelineLine>, String> {
                Ok(Vec::new())
            }

            fn handle_approval_response(
                &mut self,
                payload: &Value,
            ) -> Result<Vec<TimelineLine>, String> {
                self.approvals.push(payload.clone());
                Ok(Vec::new())
            }

            fn handle_question_response(
                &mut self,
                payload: &Value,
            ) -> Result<Vec<TimelineLine>, String> {
                self.questions.push(payload.clone());
                Ok(Vec::new())
            }
        }

        let mut state = TuiState::new();
        let mut handler = CaptureHandler::default();
        state.apply_app_event(&json!({
            "method": "turn/approval_requested",
            "params": {
                "session_id": "session_1",
                "turn_id": "turn_1",
                "approval": {
                    "request_id": "approval_4",
                    "turn_id": "turn_1",
                    "session_id": "session_1",
                    "tool_name": "bash"
                }
            }
        }));

        assert!(
            handle_local_state_command("/allow always", &mut state, &mut handler)
                .expect("allow command should be handled")
        );

        assert_eq!(handler.approvals.len(), 1);
        assert_eq!(handler.approvals[0]["request_id"], json!("approval_4"));
        assert_eq!(handler.approvals[0]["turn_id"], json!("turn_1"));
        assert_eq!(handler.approvals[0]["action"], json!("allow"));
        assert_eq!(handler.approvals[0]["scope"], json!("always"));

        state.apply_app_event(&json!({
            "method": "item/question/requested",
            "params": {
                "event": {
                    "request_id": "question_4",
                    "turn_id": "turn_1",
                    "session_id": "session_1",
                    "questions": [{"question": "Mode?"}]
                }
            }
        }));

        assert!(
            handle_local_state_command("/answer Safe path", &mut state, &mut handler)
                .expect("answer command should be handled")
        );

        assert_eq!(handler.questions.len(), 1);
        assert_eq!(handler.questions[0]["request_id"], json!("question_4"));
        assert_eq!(handler.questions[0]["turn_id"], json!("turn_1"));
        assert_eq!(handler.questions[0]["answers"], json!([["Safe path"]]));
    }

    #[test]
    fn composer_history_and_stash_round_trip() {
        let mut state = TuiState::new();
        state.remember_history("first prompt");
        state.remember_history("second prompt");

        state.history_previous();
        assert_eq!(state.input_buffer, "second prompt");
        state.history_previous();
        assert_eq!(state.input_buffer, "first prompt");
        state.history_next();
        assert_eq!(state.input_buffer, "second prompt");
        state.history_next();
        assert_eq!(state.input_buffer, "");

        state.input_buffer = "draft body".to_string();
        state.stash_current_input();
        assert_eq!(state.input_buffer, "");
        assert_eq!(state.stash.len(), 1);
        state.restore_latest_stash();
        assert_eq!(state.input_buffer, "draft body");

        state.input_buffer = "/stash queued draft".to_string();
        assert!(!state.submit());
        assert_eq!(state.stash.last().map(String::as_str), Some("queued draft"));
        state.input_buffer = "/unstash".to_string();
        assert!(!state.submit());
        assert_eq!(state.input_buffer, "queued draft");
    }

    #[test]
    fn composer_expands_file_line_ranges_and_image_attachments() {
        let root =
            std::env::temp_dir().join(format!("openagent-tui-composer-{}", std::process::id()));
        let src = root.join("src");
        fs::create_dir_all(&src).expect("create src");
        fs::write(src.join("main.rs"), "line1\nline2\nline3\nline4\n").expect("write source");
        fs::write(root.join("logo.png"), [0_u8, 1, 2, 3]).expect("write image");

        let expanded = expand_file_attachments(&root, "review @main.rs:2-3 and @logo.png");

        assert!(expanded.prompt.contains("Attached file: src/main.rs:2-3"));
        assert!(expanded.prompt.contains("line2\nline3"));
        assert!(!expanded.prompt.contains("line1\n"));
        assert!(expanded.prompt.contains("Attached image: logo.png"));
        assert_eq!(expanded.lines.len(), 2);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn composer_file_picker_and_attach_controls_insert_references() {
        let root =
            std::env::temp_dir().join(format!("openagent-tui-file-picker-{}", std::process::id()));
        let src = root.join("src");
        let docs = root.join("docs");
        fs::create_dir_all(&src).expect("create src");
        fs::create_dir_all(&docs).expect("create docs");
        fs::write(src.join("main.rs"), "fn main() {}\n").expect("write main");
        fs::write(docs.join("guide.md"), "guide\n").expect("write guide");
        fs::write(root.join("logo.png"), [0_u8, 1, 2, 3]).expect("write image");

        let matches = fuzzy_find_files(&root, "main", 10);
        assert_eq!(
            matches.first().map(|item| item.reference.as_str()),
            Some("@src/main.rs")
        );
        let lines = file_picker_lines("main", &matches);
        assert!(lines.iter().any(|line| line.text.contains("@src/main.rs")));

        let mut state = TuiState::new();
        state.input_buffer = "/attach src/main.rs:2-3".to_string();
        assert!(!state.submit());
        assert_eq!(state.input_buffer, "@src/main.rs:2-3 ");

        state.input_buffer = "review".to_string();
        let selected = state.apply_control_request(&json!({
            "path": "/tui/select-file",
            "body": {"path": "docs/guide.md", "start": 4, "end": 6}
        }));
        assert_eq!(selected["applied"], json!(true));
        assert_eq!(selected["reference"], json!("@docs/guide.md:4-6"));
        assert_eq!(state.input_buffer, "review @docs/guide.md:4-6 ");

        let image = state.apply_control_request(&json!({
            "path": "/tui/publish",
            "body": {"type": "tui.file.attach", "properties": {"path": "logo.png"}}
        }));
        assert_eq!(image["applied"], json!(true));
        assert!(state.input_buffer.ends_with("@logo.png "));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn tui_config_loads_jsonc_and_theme_command_updates_state() {
        let root =
            std::env::temp_dir().join(format!("openagent-tui-config-{}", std::process::id()));
        let config_dir = root.join(".openagent");
        fs::create_dir_all(&config_dir).expect("create config dir");
        fs::write(
            config_dir.join("tui.jsonc"),
            r#"{
                // user theme
                "theme": "midnight",
                "keybinds": {"stash": "ctrl+g"},
                "leader_key": ",",
                "mouse": false,
                "scroll": 9,
                "diff_style": "split",
                "attention_notifications": false,
                "sounds": true
            }"#,
        )
        .expect("write config");

        let config = TuiConfig::load_from_workspace(&root);
        assert_eq!(config.theme, "midnight");
        assert_eq!(config.keybinds["stash"], "ctrl+g");
        assert_eq!(config.leader_key, ",");
        assert!(!config.mouse);
        assert_eq!(config.scroll, 9);
        assert_eq!(config.diff_style, "split");
        assert!(!config.attention_notifications);
        assert!(config.sounds);

        let mut state = TuiState::with_config(config);
        state.input_buffer = "/themes high-contrast".to_string();
        assert!(!state.submit());
        assert_eq!(state.config.theme, "high-contrast");

        state.input_buffer = "/config".to_string();
        assert!(!state.submit());
        assert!(
            state
                .timeline
                .iter()
                .any(|line| line.text.contains("diff_style"))
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn observability_usage_warnings_and_tool_details_render() {
        let mut state = TuiState::new();
        state.apply_app_event(&json!({
            "method": "turn/completed",
            "params": {
                "status": "completed",
                "usage": {"input_tokens": 10, "output_tokens": 5, "total_tokens": 15, "cost": 0.01},
                "trace": {"run_id": "turn_1", "model": "server-local"}
            }
        }));
        state.apply_app_event(&json!({
            "method": "turn/completed",
            "params": {
                "status": "completed",
                "usage": {"input_tokens": 2, "output_tokens": 3, "total_tokens": 5, "cost": 0.02}
            }
        }));
        assert_eq!(state.usage_totals["input_tokens"], json!(12));
        assert_eq!(state.usage_totals["output_tokens"], json!(8));
        assert_eq!(state.usage_totals["total_tokens"], json!(20));

        state.apply_app_event(&json!({
            "method": "runtime/warning",
            "params": {"message": "provider throttled"}
        }));
        assert_eq!(
            state.runtime_warnings,
            vec!["provider throttled".to_string()]
        );

        state.input_buffer = "/tool-details on".to_string();
        assert!(!state.submit());
        assert!(state.show_tool_details);
        state.apply_app_event(&json!({
            "method": "item/toolCall/completed",
            "params": {
                "name": "bash",
                "output": "ok",
                "metadata": {"returncode": 0}
            }
        }));
        assert!(
            state
                .timeline
                .iter()
                .any(|line| line.text.contains("tool details"))
        );

        state.input_buffer = "/warnings".to_string();
        assert!(!state.submit());
        assert!(
            state
                .timeline
                .iter()
                .any(|line| line.text.contains("provider throttled"))
        );
    }

    #[test]
    fn key_event_flow_submits_prompt_and_uses_history() {
        #[derive(Default)]
        struct CaptureHandler {
            prompts: Vec<String>,
        }

        impl TerminalEventHandler for CaptureHandler {
            fn handle_submit(&mut self, prompt: &str) -> Result<Vec<TimelineLine>, String> {
                self.prompts.push(prompt.to_string());
                Ok(vec![TimelineLine::new("status", "submitted", true)])
            }

            fn handle_command(&mut self, _command: &str) -> Result<Vec<TimelineLine>, String> {
                Ok(Vec::new())
            }
        }

        let mut state = TuiState::new();
        let mut handler = CaptureHandler::default();
        for ch in "hello".chars() {
            handle_key_event(
                KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE),
                &mut state,
                &mut handler,
            )
            .expect("char event");
        }
        handle_key_event(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut state,
            &mut handler,
        )
        .expect("enter event");
        assert_eq!(handler.prompts, vec!["hello".to_string()]);

        handle_key_event(
            KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
            &mut state,
            &mut handler,
        )
        .expect("history up");
        assert_eq!(state.input_buffer, "hello");
        handle_key_event(
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            &mut state,
            &mut handler,
        )
        .expect("history down");
        assert_eq!(state.input_buffer, "");
    }

    #[test]
    fn key_event_flow_opens_file_picker_filters_and_attaches() {
        #[derive(Default)]
        struct CaptureHandler {
            searches: Vec<String>,
        }

        impl TerminalEventHandler for CaptureHandler {
            fn handle_submit(&mut self, _prompt: &str) -> Result<Vec<TimelineLine>, String> {
                Ok(Vec::new())
            }

            fn handle_command(&mut self, _command: &str) -> Result<Vec<TimelineLine>, String> {
                Ok(Vec::new())
            }

            fn search_files(&mut self, query: &str) -> Result<Vec<ComposerFileCandidate>, String> {
                self.searches.push(query.to_string());
                let candidates = match query {
                    "ma" => vec![
                        ComposerFileCandidate {
                            reference: "@src/main.rs".to_string(),
                            kind: "file".to_string(),
                        },
                        ComposerFileCandidate {
                            reference: "@images/map.png".to_string(),
                            kind: "image".to_string(),
                        },
                    ],
                    "mam" => vec![ComposerFileCandidate {
                        reference: "@src/main.rs".to_string(),
                        kind: "file".to_string(),
                    }],
                    _ => vec![
                        ComposerFileCandidate {
                            reference: "@src/lib.rs".to_string(),
                            kind: "file".to_string(),
                        },
                        ComposerFileCandidate {
                            reference: "@docs/guide.md".to_string(),
                            kind: "file".to_string(),
                        },
                    ],
                };
                Ok(candidates)
            }
        }

        let mut state = TuiState::new();
        let mut handler = CaptureHandler::default();
        for ch in "/files ma".chars() {
            handle_key_event(
                KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE),
                &mut state,
                &mut handler,
            )
            .expect("char event");
        }
        handle_key_event(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut state,
            &mut handler,
        )
        .expect("open picker");
        assert_eq!(handler.searches, vec!["ma".to_string()]);
        assert_eq!(
            state.file_picker.as_ref().expect("picker").query.as_str(),
            "ma"
        );

        handle_key_event(
            KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE),
            &mut state,
            &mut handler,
        )
        .expect("filter picker");
        assert_eq!(
            state.file_picker.as_ref().expect("picker").query.as_str(),
            "mam"
        );
        assert_eq!(handler.searches, vec!["ma".to_string(), "mam".to_string()]);

        handle_key_event(
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
            &mut state,
            &mut handler,
        )
        .expect("backspace filter");
        handle_key_event(
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
            &mut state,
            &mut handler,
        )
        .expect("select image");
        handle_key_event(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut state,
            &mut handler,
        )
        .expect("attach selected file");

        assert!(state.file_picker.is_none());
        assert_eq!(state.input_buffer, "@images/map.png ");
    }

    #[test]
    fn key_event_flow_opens_session_picker_filters_and_resumes() {
        #[derive(Default)]
        struct CaptureHandler {
            searches: Vec<String>,
            commands: Vec<String>,
        }

        impl TerminalEventHandler for CaptureHandler {
            fn handle_submit(&mut self, _prompt: &str) -> Result<Vec<TimelineLine>, String> {
                Ok(Vec::new())
            }

            fn handle_command(&mut self, command: &str) -> Result<Vec<TimelineLine>, String> {
                self.commands.push(command.to_string());
                Ok(vec![TimelineLine::new(
                    "status",
                    format!("handled {command}"),
                    true,
                )])
            }

            fn search_sessions(&mut self, query: &str) -> Result<Vec<Value>, String> {
                self.searches.push(query.to_string());
                let sessions = match query {
                    "al" => vec![
                        json!({
                            "session_id": "session_alpha",
                            "title": "Alpha",
                            "status": "idle",
                            "message_count": 2,
                            "workspace": "/tmp/alpha"
                        }),
                        json!({
                            "session_id": "session_alpine",
                            "title": "Alpine",
                            "status": "idle",
                            "message_count": 3,
                            "workspace": "/tmp/alpine"
                        }),
                    ],
                    "alp" => vec![json!({
                        "session_id": "session_alpha",
                        "title": "Alpha",
                        "status": "idle",
                        "message_count": 2,
                        "workspace": "/tmp/alpha"
                    })],
                    _ => Vec::new(),
                };
                Ok(sessions)
            }
        }

        let mut state = TuiState::new();
        let mut handler = CaptureHandler::default();
        send_key_text("/sessions al", &mut state, &mut handler).expect("type sessions command");
        press_key(KeyCode::Enter, &mut state, &mut handler).expect("open session picker");

        assert_eq!(handler.searches, vec!["al".to_string()]);
        assert_eq!(
            state
                .session_picker
                .as_ref()
                .expect("session picker")
                .candidates
                .len(),
            2
        );

        press_key(KeyCode::Char('p'), &mut state, &mut handler).expect("filter picker");
        assert_eq!(handler.searches, vec!["al".to_string(), "alp".to_string()]);
        assert_eq!(
            state.session_picker.as_ref().expect("session picker").query,
            "alp"
        );

        press_key(KeyCode::Enter, &mut state, &mut handler).expect("resume selected session");

        assert!(state.session_picker.is_none());
        assert_eq!(state.session_id.as_deref(), Some("session_alpha"));
        assert_eq!(handler.commands, vec!["/resume session_alpha".to_string()]);
        assert!(
            state
                .timeline
                .iter()
                .any(|line| line.text.contains("handled /resume session_alpha"))
        );
    }

    #[test]
    fn key_event_flow_opens_model_picker_filters_and_selects() {
        #[derive(Default)]
        struct CaptureHandler {
            model_fetches: usize,
            commands: Vec<String>,
        }

        impl TerminalEventHandler for CaptureHandler {
            fn handle_submit(&mut self, _prompt: &str) -> Result<Vec<TimelineLine>, String> {
                Ok(Vec::new())
            }

            fn handle_command(&mut self, command: &str) -> Result<Vec<TimelineLine>, String> {
                self.commands.push(command.to_string());
                Ok(vec![TimelineLine::new(
                    "status",
                    format!("handled {command}"),
                    true,
                )])
            }

            fn list_models(&mut self) -> Result<Value, String> {
                self.model_fetches += 1;
                Ok(json!({
                    "models": [
                        {"id": "server-local", "provider_id": "openagent", "name": "Server Local"},
                        {"id": "deep-model", "provider_id": "openagent", "name": "Deep Model"}
                    ]
                }))
            }
        }

        let mut state = TuiState::new();
        let mut handler = CaptureHandler::default();
        send_key_text("/models", &mut state, &mut handler).expect("type models command");
        press_key(KeyCode::Enter, &mut state, &mut handler).expect("open model picker");

        assert_eq!(handler.model_fetches, 1);
        assert_eq!(
            state
                .model_picker
                .as_ref()
                .expect("model picker")
                .candidates
                .len(),
            2
        );

        press_key(KeyCode::Char('d'), &mut state, &mut handler).expect("filter picker");
        assert_eq!(
            state
                .model_picker
                .as_ref()
                .expect("model picker")
                .candidates
                .len(),
            1
        );
        assert_eq!(
            state
                .model_picker
                .as_ref()
                .expect("model picker")
                .candidates[0]["id"],
            json!("deep-model")
        );

        press_key(KeyCode::Enter, &mut state, &mut handler).expect("select model");

        assert!(state.model_picker.is_none());
        assert_eq!(handler.commands, vec!["/models deep-model".to_string()]);
        assert!(
            state
                .timeline
                .iter()
                .any(|line| line.text.contains("handled /models deep-model"))
        );
    }

    #[test]
    fn key_event_flow_opens_agent_picker_filters_and_selects() {
        #[derive(Default)]
        struct CaptureHandler {
            agent_fetches: usize,
            commands: Vec<String>,
        }

        impl TerminalEventHandler for CaptureHandler {
            fn handle_submit(&mut self, _prompt: &str) -> Result<Vec<TimelineLine>, String> {
                Ok(Vec::new())
            }

            fn handle_command(&mut self, command: &str) -> Result<Vec<TimelineLine>, String> {
                self.commands.push(command.to_string());
                Ok(vec![TimelineLine::new(
                    "status",
                    format!("handled {command}"),
                    true,
                )])
            }

            fn list_agents(&mut self) -> Result<Value, String> {
                self.agent_fetches += 1;
                Ok(json!({
                    "agents": [
                        {"id": "server", "name": "Server", "description": "Default server agent"},
                        {"id": "reviewer", "name": "Reviewer", "description": "Review code"}
                    ]
                }))
            }
        }

        let mut state = TuiState::new();
        let mut handler = CaptureHandler::default();
        send_key_text("/agents", &mut state, &mut handler).expect("type agents command");
        press_key(KeyCode::Enter, &mut state, &mut handler).expect("open agent picker");

        assert_eq!(handler.agent_fetches, 1);
        assert_eq!(
            state
                .agent_picker
                .as_ref()
                .expect("agent picker")
                .candidates
                .len(),
            2
        );

        send_key_text("rev", &mut state, &mut handler).expect("filter picker");
        assert_eq!(
            state
                .agent_picker
                .as_ref()
                .expect("agent picker")
                .candidates[0]["id"],
            json!("reviewer")
        );

        press_key(KeyCode::Enter, &mut state, &mut handler).expect("select agent");

        assert!(state.agent_picker.is_none());
        assert_eq!(handler.commands, vec!["/agent reviewer".to_string()]);
        assert!(
            state
                .timeline
                .iter()
                .any(|line| line.text.contains("handled /agent reviewer"))
        );
    }

    #[test]
    fn key_event_flow_opens_variant_and_thinking_pickers() {
        #[derive(Default)]
        struct CaptureHandler {
            model_fetches: usize,
            commands: Vec<String>,
        }

        impl TerminalEventHandler for CaptureHandler {
            fn handle_submit(&mut self, _prompt: &str) -> Result<Vec<TimelineLine>, String> {
                Ok(Vec::new())
            }

            fn handle_command(&mut self, command: &str) -> Result<Vec<TimelineLine>, String> {
                self.commands.push(command.to_string());
                Ok(vec![TimelineLine::new(
                    "status",
                    format!("handled {command}"),
                    true,
                )])
            }

            fn list_models(&mut self) -> Result<Value, String> {
                self.model_fetches += 1;
                Ok(json!({
                    "models": [],
                    "variants": ["default", "deep"],
                    "thinking": ["low", "high"]
                }))
            }
        }

        let mut state = TuiState::new();
        let mut handler = CaptureHandler::default();

        send_key_text("/variant", &mut state, &mut handler).expect("type variant command");
        press_key(KeyCode::Enter, &mut state, &mut handler).expect("open variant picker");
        assert_eq!(handler.model_fetches, 1);
        assert_eq!(
            state.choice_picker.as_ref().expect("variant picker").kind,
            ChoicePickerKind::Variant
        );

        send_key_text("dee", &mut state, &mut handler).expect("filter variant picker");
        assert_eq!(
            state
                .choice_picker
                .as_ref()
                .expect("variant picker")
                .candidates,
            vec!["deep".to_string()]
        );
        press_key(KeyCode::Enter, &mut state, &mut handler).expect("select variant");

        send_key_text("/thinking", &mut state, &mut handler).expect("type thinking command");
        press_key(KeyCode::Enter, &mut state, &mut handler).expect("open thinking picker");
        assert_eq!(handler.model_fetches, 2);
        assert_eq!(
            state.choice_picker.as_ref().expect("thinking picker").kind,
            ChoicePickerKind::Thinking
        );

        send_key_text("hi", &mut state, &mut handler).expect("filter thinking picker");
        assert_eq!(
            state
                .choice_picker
                .as_ref()
                .expect("thinking picker")
                .candidates,
            vec!["high".to_string()]
        );
        press_key(KeyCode::Enter, &mut state, &mut handler).expect("select thinking");

        assert!(state.choice_picker.is_none());
        assert_eq!(
            handler.commands,
            vec!["/variant deep".to_string(), "/thinking high".to_string()]
        );
        assert!(
            state
                .timeline
                .iter()
                .any(|line| line.text.contains("handled /thinking high"))
        );
    }

    #[test]
    fn key_event_flow_opens_theme_picker_filters_and_selects() {
        #[derive(Default)]
        struct CaptureHandler {
            commands: Vec<String>,
        }

        impl TerminalEventHandler for CaptureHandler {
            fn handle_submit(&mut self, _prompt: &str) -> Result<Vec<TimelineLine>, String> {
                Ok(Vec::new())
            }

            fn handle_command(&mut self, command: &str) -> Result<Vec<TimelineLine>, String> {
                self.commands.push(command.to_string());
                Ok(Vec::new())
            }
        }

        let mut state = TuiState::new();
        let mut handler = CaptureHandler::default();

        send_key_text("/themes", &mut state, &mut handler).expect("type themes command");
        press_key(KeyCode::Enter, &mut state, &mut handler).expect("open theme picker");

        let picker = state.choice_picker.as_ref().expect("theme picker");
        assert_eq!(picker.kind, ChoicePickerKind::Theme);
        assert_eq!(picker.candidates, default_theme_names());

        send_key_text("high", &mut state, &mut handler).expect("filter theme picker");
        assert_eq!(
            state
                .choice_picker
                .as_ref()
                .expect("theme picker")
                .candidates,
            vec!["high-contrast".to_string()]
        );
        press_key(KeyCode::Enter, &mut state, &mut handler).expect("select theme");

        assert!(state.choice_picker.is_none());
        assert_eq!(state.config.theme, "high-contrast");
        assert!(handler.commands.is_empty());
        assert!(
            state
                .timeline
                .iter()
                .any(|line| line.text.contains("theme set to high-contrast"))
        );
    }

    #[test]
    fn key_event_flow_at_opens_file_picker_without_touching_commands() {
        #[derive(Default)]
        struct CaptureHandler {
            searches: Vec<String>,
        }

        impl TerminalEventHandler for CaptureHandler {
            fn handle_submit(&mut self, _prompt: &str) -> Result<Vec<TimelineLine>, String> {
                Ok(Vec::new())
            }

            fn handle_command(&mut self, _command: &str) -> Result<Vec<TimelineLine>, String> {
                Ok(Vec::new())
            }

            fn search_files(&mut self, query: &str) -> Result<Vec<ComposerFileCandidate>, String> {
                self.searches.push(query.to_string());
                Ok(vec![
                    ComposerFileCandidate {
                        reference: "@src/main.rs".to_string(),
                        kind: "file".to_string(),
                    },
                    ComposerFileCandidate {
                        reference: "@docs/guide.md".to_string(),
                        kind: "file".to_string(),
                    },
                ])
            }
        }

        let mut state = TuiState::new();
        let mut handler = CaptureHandler::default();
        state.input_buffer = "review".to_string();
        handle_key_event(
            KeyEvent::new(KeyCode::Char('@'), KeyModifiers::SHIFT),
            &mut state,
            &mut handler,
        )
        .expect("@ opens file picker");

        assert_eq!(handler.searches, vec!["".to_string()]);
        assert_eq!(state.input_buffer, "review");
        assert_eq!(
            state
                .file_picker
                .as_ref()
                .expect("file picker")
                .candidates
                .len(),
            2
        );

        handle_key_event(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            &mut state,
            &mut handler,
        )
        .expect("attach first candidate");

        assert!(state.file_picker.is_none());
        assert_eq!(state.input_buffer, "review @src/main.rs ");

        state.input_buffer = "/rename ".to_string();
        handle_key_event(
            KeyEvent::new(KeyCode::Char('@'), KeyModifiers::SHIFT),
            &mut state,
            &mut handler,
        )
        .expect("@ stays literal in commands");
        assert_eq!(state.input_buffer, "/rename @");
        assert_eq!(handler.searches, vec!["".to_string()]);
    }

    #[test]
    fn app_bridge_terminal_keyflow_smoke_uses_real_remote_handler() -> Result<(), Box<dyn Error>> {
        let bridge = FakeAppBridge::start()?;
        let workspace = temp_test_dir("openagent-tui-bridge-keyflow")?;
        fs::write(workspace.join("notes.txt"), "hello from workspace\n")?;
        let mut handler = AppBridgeTerminalHandler::connect(AppBridgeTerminalOptions {
            server_url: bridge.server_url.clone(),
            auth: RemoteAuth::bearer("secret"),
            workspace: workspace.clone(),
            ..AppBridgeTerminalOptions::default()
        })
        .map_err(|error| std::io::Error::new(ErrorKind::Other, error))?;
        let mut state = TuiState::new();

        let initial = handler.initial_lines();
        apply_handler_output(&mut state, &mut handler, initial);
        assert!(
            state
                .timeline
                .iter()
                .any(|line| line.text.contains("connected to"))
        );

        send_key_text("/new", &mut state, &mut handler)?;
        press_key(KeyCode::Enter, &mut state, &mut handler)?;
        assert_eq!(handler.current_session(), Some("session_smoke"));
        assert!(
            state
                .timeline
                .iter()
                .any(|line| line.text.contains("created session: session_smoke"))
        );

        send_key_text("hello bridge", &mut state, &mut handler)?;
        press_key(KeyCode::Enter, &mut state, &mut handler)?;
        assert_eq!(state.session_id.as_deref(), Some("session_smoke"));
        assert_eq!(state.current_turn_id.as_deref(), Some("turn_smoke"));
        assert_eq!(state.status, "completed");
        assert_eq!(state.usage_totals["total_tokens"], json!(3));
        assert!(
            state
                .timeline
                .iter()
                .any(|line| line.kind == "assistant" && line.text.contains("bridge answer"))
        );

        let polled = handler
            .poll_app_events()
            .map_err(|error| std::io::Error::new(ErrorKind::Other, error))?;
        apply_app_event_values(&mut state, polled);
        assert!(
            state
                .runtime_warnings
                .iter()
                .any(|warning| warning.contains("bridge smoke warning"))
        );

        let recorded = bridge.requests();
        assert!(recorded.iter().any(|request| request == "GET /api/health"));
        assert!(
            recorded
                .iter()
                .any(|request| request == "GET /api/sessions")
        );
        assert!(
            recorded
                .iter()
                .any(|request| request == "POST /api/sessions")
        );
        assert!(
            recorded
                .iter()
                .any(|request| request == "POST /api/sessions/session_smoke/turns")
        );
        assert!(
            recorded
                .iter()
                .any(|request| request.starts_with("GET /api/events?last_event_id="))
        );
        assert!(
            bridge
                .turn_inputs()
                .iter()
                .any(|input| input == "hello bridge")
        );

        bridge.stop();
        let _ = fs::remove_dir_all(workspace);
        Ok(())
    }

    #[test]
    fn app_bridge_terminal_transcript_reads_real_session_messages() -> Result<(), Box<dyn Error>> {
        let bridge = FakeAppBridge::start()?;
        let workspace = temp_test_dir("openagent-tui-bridge-transcript")?;
        let mut handler = AppBridgeTerminalHandler::connect(AppBridgeTerminalOptions {
            server_url: bridge.server_url.clone(),
            auth: RemoteAuth::bearer("secret"),
            workspace: workspace.clone(),
            session_id: Some("session_smoke".to_string()),
            ..AppBridgeTerminalOptions::default()
        })
        .map_err(|error| std::io::Error::new(ErrorKind::Other, error))?;

        let lines = handler
            .handle_command("/transcript 2")
            .map_err(|error| std::io::Error::new(ErrorKind::Other, error))?;

        assert!(
            lines
                .iter()
                .any(|line| line.text.contains("transcript: 2 of 3 message(s)"))
        );
        assert!(
            lines
                .iter()
                .any(|line| line.kind == "message" && line.text.contains("#1 assistant"))
        );
        assert!(
            lines
                .iter()
                .any(|line| line.kind == "message" && line.text.contains("bridge answer"))
        );
        let recorded = bridge.requests();
        assert!(
            recorded
                .iter()
                .any(|request| { request == "GET /api/sessions/session_smoke/messages?limit=2" })
        );

        bridge.stop();
        let _ = fs::remove_dir_all(workspace);
        Ok(())
    }

    #[test]
    fn app_bridge_terminal_session_picker_searches_and_resumes() -> Result<(), Box<dyn Error>> {
        let bridge = FakeAppBridge::start()?;
        let workspace = temp_test_dir("openagent-tui-bridge-session-picker")?;
        let mut handler = AppBridgeTerminalHandler::connect(AppBridgeTerminalOptions {
            server_url: bridge.server_url.clone(),
            auth: RemoteAuth::bearer("secret"),
            workspace: workspace.clone(),
            ..AppBridgeTerminalOptions::default()
        })
        .map_err(|error| std::io::Error::new(ErrorKind::Other, error))?;
        let mut state = TuiState::new();

        send_key_text("/sessions smoke", &mut state, &mut handler)?;
        press_key(KeyCode::Enter, &mut state, &mut handler)?;

        assert_eq!(
            state
                .session_picker
                .as_ref()
                .expect("session picker")
                .candidates
                .len(),
            1
        );
        assert!(
            state
                .session_picker
                .as_ref()
                .expect("session picker")
                .candidates[0]["session_id"]
                == json!("session_smoke")
        );

        press_key(KeyCode::Enter, &mut state, &mut handler)?;

        assert!(state.session_picker.is_none());
        assert_eq!(state.session_id.as_deref(), Some("session_smoke"));
        assert_eq!(handler.current_session(), Some("session_smoke"));
        let recorded = bridge.requests();
        assert!(
            recorded
                .iter()
                .any(|request| request == "GET /api/sessions?query=smoke")
        );

        bridge.stop();
        let _ = fs::remove_dir_all(workspace);
        Ok(())
    }

    #[test]
    fn app_bridge_terminal_model_picker_fetches_and_sets_model() -> Result<(), Box<dyn Error>> {
        let bridge = FakeAppBridge::start()?;
        let workspace = temp_test_dir("openagent-tui-bridge-model-picker")?;
        let mut handler = AppBridgeTerminalHandler::connect(AppBridgeTerminalOptions {
            server_url: bridge.server_url.clone(),
            auth: RemoteAuth::bearer("secret"),
            workspace: workspace.clone(),
            session_id: Some("session_smoke".to_string()),
            ..AppBridgeTerminalOptions::default()
        })
        .map_err(|error| std::io::Error::new(ErrorKind::Other, error))?;
        let mut state = TuiState::new();

        send_key_text("/models", &mut state, &mut handler)?;
        press_key(KeyCode::Enter, &mut state, &mut handler)?;

        assert_eq!(
            state
                .model_picker
                .as_ref()
                .expect("model picker")
                .candidates
                .len(),
            2
        );

        press_key(KeyCode::Char('d'), &mut state, &mut handler)?;
        assert_eq!(
            state
                .model_picker
                .as_ref()
                .expect("model picker")
                .candidates[0]["id"],
            json!("deep-model")
        );

        press_key(KeyCode::Enter, &mut state, &mut handler)?;

        assert!(state.model_picker.is_none());
        assert!(
            state
                .timeline
                .iter()
                .any(|line| line.text.contains("model set to deep-model"))
        );
        let model_updates = bridge.model_update_payloads();
        assert_eq!(model_updates.len(), 1);
        assert_eq!(model_updates[0]["model"], json!("deep-model"));
        let recorded = bridge.requests();
        assert!(recorded.iter().any(|request| request == "GET /api/models"));
        assert!(
            recorded
                .iter()
                .any(|request| request == "PATCH /api/sessions/session_smoke")
        );

        bridge.stop();
        let _ = fs::remove_dir_all(workspace);
        Ok(())
    }

    #[test]
    fn app_bridge_terminal_agent_picker_fetches_and_sets_agent() -> Result<(), Box<dyn Error>> {
        let bridge = FakeAppBridge::start()?;
        let workspace = temp_test_dir("openagent-tui-bridge-agent-picker")?;
        let mut handler = AppBridgeTerminalHandler::connect(AppBridgeTerminalOptions {
            server_url: bridge.server_url.clone(),
            auth: RemoteAuth::bearer("secret"),
            workspace: workspace.clone(),
            session_id: Some("session_smoke".to_string()),
            ..AppBridgeTerminalOptions::default()
        })
        .map_err(|error| std::io::Error::new(ErrorKind::Other, error))?;
        let mut state = TuiState::new();

        send_key_text("/agents", &mut state, &mut handler)?;
        press_key(KeyCode::Enter, &mut state, &mut handler)?;

        assert_eq!(
            state
                .agent_picker
                .as_ref()
                .expect("agent picker")
                .candidates
                .len(),
            2
        );

        send_key_text("rev", &mut state, &mut handler)?;
        assert_eq!(
            state
                .agent_picker
                .as_ref()
                .expect("agent picker")
                .candidates[0]["id"],
            json!("reviewer")
        );

        press_key(KeyCode::Enter, &mut state, &mut handler)?;

        assert!(state.agent_picker.is_none());
        assert!(
            state
                .timeline
                .iter()
                .any(|line| line.text.contains("agent set to reviewer"))
        );
        let agent_updates = bridge.agent_update_payloads();
        assert_eq!(agent_updates.len(), 1);
        assert_eq!(agent_updates[0]["agent"], json!("reviewer"));
        let recorded = bridge.requests();
        assert!(recorded.iter().any(|request| request == "GET /api/agents"));
        assert!(
            recorded
                .iter()
                .any(|request| request == "PATCH /api/sessions/session_smoke")
        );

        bridge.stop();
        let _ = fs::remove_dir_all(workspace);
        Ok(())
    }

    #[test]
    fn app_bridge_terminal_variant_and_thinking_pickers_fetch_and_set() -> Result<(), Box<dyn Error>>
    {
        let bridge = FakeAppBridge::start()?;
        let workspace = temp_test_dir("openagent-tui-bridge-choice-picker")?;
        let mut handler = AppBridgeTerminalHandler::connect(AppBridgeTerminalOptions {
            server_url: bridge.server_url.clone(),
            auth: RemoteAuth::bearer("secret"),
            workspace: workspace.clone(),
            session_id: Some("session_smoke".to_string()),
            ..AppBridgeTerminalOptions::default()
        })
        .map_err(|error| std::io::Error::new(ErrorKind::Other, error))?;
        let mut state = TuiState::new();

        send_key_text("/variant", &mut state, &mut handler)?;
        press_key(KeyCode::Enter, &mut state, &mut handler)?;
        send_key_text("dee", &mut state, &mut handler)?;
        assert_eq!(
            state
                .choice_picker
                .as_ref()
                .expect("variant picker")
                .candidates,
            vec!["deep".to_string()]
        );
        press_key(KeyCode::Enter, &mut state, &mut handler)?;

        assert!(state.choice_picker.is_none());
        assert!(
            state
                .timeline
                .iter()
                .any(|line| line.text.contains("variant set to deep"))
        );
        let variant_updates = bridge.variant_update_payloads();
        assert_eq!(variant_updates.len(), 1);
        assert_eq!(variant_updates[0]["variant"], json!("deep"));

        send_key_text("/thinking", &mut state, &mut handler)?;
        press_key(KeyCode::Enter, &mut state, &mut handler)?;
        send_key_text("hi", &mut state, &mut handler)?;
        assert_eq!(
            state
                .choice_picker
                .as_ref()
                .expect("thinking picker")
                .candidates,
            vec!["high".to_string()]
        );
        press_key(KeyCode::Enter, &mut state, &mut handler)?;

        assert!(state.choice_picker.is_none());
        assert!(
            state
                .timeline
                .iter()
                .any(|line| line.text.contains("thinking set to high"))
        );
        let thinking_updates = bridge.thinking_update_payloads();
        assert_eq!(thinking_updates.len(), 1);
        assert_eq!(thinking_updates[0]["thinking"], json!("high"));
        let recorded = bridge.requests();
        assert_eq!(
            recorded
                .iter()
                .filter(|request| request.as_str() == "GET /api/models")
                .count(),
            2
        );
        assert!(
            recorded
                .iter()
                .any(|request| request == "PATCH /api/sessions/session_smoke")
        );

        bridge.stop();
        let _ = fs::remove_dir_all(workspace);
        Ok(())
    }

    #[test]
    fn app_bridge_terminal_interaction_keyflow_posts_real_responses() -> Result<(), Box<dyn Error>>
    {
        let bridge = FakeAppBridge::start()?;
        let workspace = temp_test_dir("openagent-tui-bridge-interactions")?;
        let mut handler = AppBridgeTerminalHandler::connect(AppBridgeTerminalOptions {
            server_url: bridge.server_url.clone(),
            auth: RemoteAuth::bearer("secret"),
            workspace: workspace.clone(),
            ..AppBridgeTerminalOptions::default()
        })
        .map_err(|error| std::io::Error::new(ErrorKind::Other, error))?;
        let mut state = TuiState::new();

        state.apply_app_event(&json!({
            "method": "turn/approval_requested",
            "params": {
                "session_id": "session_smoke",
                "thread_id": "session_smoke",
                "turn_id": "turn_approval",
                "status": "waiting_approval",
                "approval": {
                    "request_id": "approval_smoke",
                    "turn_id": "turn_approval",
                    "session_id": "session_smoke",
                    "tool_name": "bash",
                    "tool_input": {"command": "printf ok"}
                }
            }
        }));
        press_key(KeyCode::Char('1'), &mut state, &mut handler)?;

        assert!(state.active_approval.is_none());
        assert_eq!(state.status, "completed");
        assert!(
            state
                .timeline
                .iter()
                .any(|line| line.text.contains("approved through bridge"))
        );
        let approvals = bridge.approval_payloads();
        assert_eq!(approvals.len(), 1);
        assert_eq!(approvals[0]["action"], json!("allow"));
        assert_eq!(approvals[0]["scope"], json!("once"));
        assert_eq!(approvals[0]["request_id"], json!("approval_smoke"));

        state.apply_app_event(&json!({
            "method": "item/question/requested",
            "params": {
                "session_id": "session_smoke",
                "thread_id": "session_smoke",
                "turn_id": "turn_question",
                "event": {
                    "request_id": "question_smoke",
                    "turn_id": "turn_question",
                    "session_id": "session_smoke",
                    "questions": [{
                        "header": "Mode",
                        "question": "Which path?",
                        "options": [
                            {"label": "Fast"},
                            {"label": "Safe"}
                        ]
                    }]
                }
            }
        }));
        press_key(KeyCode::Char('2'), &mut state, &mut handler)?;

        assert!(state.active_question.is_none());
        assert_eq!(state.status, "completed");
        assert!(
            state
                .timeline
                .iter()
                .any(|line| line.text.contains("question bridge answer"))
        );
        let questions = bridge.question_payloads();
        assert_eq!(questions.len(), 1);
        assert_eq!(questions[0]["request_id"], json!("question_smoke"));
        assert_eq!(questions[0]["answers"], json!([["Safe"]]));

        let recorded = bridge.requests();
        assert!(
            recorded
                .iter()
                .any(|request| request == "POST /api/turns/turn_approval/approvals/approval_smoke")
        );
        assert!(recorded.iter().any(|request| {
            request == "POST /api/turns/turn_question/questions/question_smoke/reply"
        }));

        bridge.stop();
        let _ = fs::remove_dir_all(workspace);
        Ok(())
    }

    #[test]
    fn terminal_render_snapshot_contains_permission_overlay() {
        let backend = TestBackend::new(96, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut state = TuiState::new();
        state.apply_app_event(&json!({
            "method": "turn/approval_requested",
            "params": {
                "approval": {
                    "request_id": "approval_overlay",
                    "turn_id": "turn_1",
                    "tool_name": "write",
                    "tool_input": {"file_path": "src/lib.rs"},
                    "preview": {
                        "kind": "file",
                        "path": "src/lib.rs",
                        "diff": "+changed"
                    }
                }
            }
        }));

        terminal
            .draw(|frame| draw_terminal_frame(frame, "OpenAgent", &state))
            .expect("draw frame");
        let rendered = format!("{:?}", terminal.backend().buffer());

        assert!(rendered.contains("Interaction: Approval"));
        assert!(rendered.contains("Allow once"));
        assert!(rendered.contains("Always allow"));
        assert!(rendered.contains("Deny"));
        assert!(rendered.contains("write"));
    }

    #[test]
    fn terminal_render_snapshot_contains_file_picker_overlay() {
        let backend = TestBackend::new(96, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut state = TuiState::new();
        state.open_file_picker(
            "main",
            vec![
                ComposerFileCandidate {
                    reference: "@src/main.rs".to_string(),
                    kind: "file".to_string(),
                },
                ComposerFileCandidate {
                    reference: "@images/map.png".to_string(),
                    kind: "image".to_string(),
                },
            ],
        );

        terminal
            .draw(|frame| draw_terminal_frame(frame, "OpenAgent", &state))
            .expect("draw frame");
        let rendered = format!("{:?}", terminal.backend().buffer());

        assert!(rendered.contains("Composer: Files"));
        assert!(rendered.contains("Query"));
        assert!(rendered.contains("@src/main.rs"));
        assert!(rendered.contains("@images/map.png"));
    }

    #[test]
    fn terminal_render_snapshot_contains_session_picker_overlay() {
        let backend = TestBackend::new(96, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut state = TuiState::new();
        state.open_session_picker(
            "alp",
            vec![
                json!({
                    "session_id": "session_alpha",
                    "title": "Alpha",
                    "status": "idle",
                    "message_count": 2,
                    "workspace": "/tmp/alpha"
                }),
                json!({
                    "session_id": "session_alpine",
                    "title": "Alpine",
                    "status": "running",
                    "message_count": 4,
                    "workspace": "/tmp/alpine"
                }),
            ],
        );

        terminal
            .draw(|frame| draw_terminal_frame(frame, "OpenAgent", &state))
            .expect("draw frame");
        let rendered = format!("{:?}", terminal.backend().buffer());

        assert!(rendered.contains("Sessions"));
        assert!(rendered.contains("Query"));
        assert!(rendered.contains("session_alpha"));
        assert!(rendered.contains("Alpine"));
    }

    #[test]
    fn terminal_render_snapshot_contains_model_picker_overlay() {
        let backend = TestBackend::new(96, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut state = TuiState::new();
        state.open_model_picker(
            "deep",
            vec![
                json!({
                    "id": "server-local",
                    "provider_id": "openagent",
                    "name": "Server Local",
                    "default": true
                }),
                json!({
                    "id": "deep-model",
                    "provider_id": "openagent",
                    "name": "Deep Model"
                }),
            ],
        );

        terminal
            .draw(|frame| draw_terminal_frame(frame, "OpenAgent", &state))
            .expect("draw frame");
        let rendered = format!("{:?}", terminal.backend().buffer());

        assert!(rendered.contains("Models"));
        assert!(rendered.contains("Query"));
        assert!(rendered.contains("deep-model"));
        assert!(rendered.contains("Deep Model"));
    }

    #[test]
    fn terminal_render_snapshot_contains_agent_picker_overlay() {
        let backend = TestBackend::new(96, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut state = TuiState::new();
        state.open_agent_picker(
            "rev",
            vec![
                json!({
                    "id": "server",
                    "name": "Server",
                    "description": "Default server agent",
                    "default": true
                }),
                json!({
                    "id": "reviewer",
                    "name": "Reviewer",
                    "description": "Review code"
                }),
            ],
        );

        terminal
            .draw(|frame| draw_terminal_frame(frame, "OpenAgent", &state))
            .expect("draw frame");
        let rendered = format!("{:?}", terminal.backend().buffer());

        assert!(rendered.contains("Agents"));
        assert!(rendered.contains("Query"));
        assert!(rendered.contains("reviewer"));
        assert!(rendered.contains("Reviewer"));
    }

    #[test]
    fn terminal_render_snapshot_contains_choice_picker_overlay() {
        let backend = TestBackend::new(96, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut state = TuiState::new();
        state.open_choice_picker(
            ChoicePickerKind::Variant,
            "dee",
            vec!["default".to_string(), "deep".to_string()],
        );

        terminal
            .draw(|frame| draw_terminal_frame(frame, "OpenAgent", &state))
            .expect("draw frame");
        let rendered = format!("{:?}", terminal.backend().buffer());

        assert!(rendered.contains("Variants"));
        assert!(rendered.contains("Query"));
        assert!(rendered.contains("deep"));
        assert!(rendered.contains("Enter select"));
    }

    #[test]
    fn terminal_render_snapshot_contains_theme_picker_overlay() {
        let backend = TestBackend::new(96, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut state = TuiState::new();
        state.set_theme("midnight");
        state.open_choice_picker(ChoicePickerKind::Theme, "", default_theme_names());

        terminal
            .draw(|frame| draw_terminal_frame(frame, "OpenAgent", &state))
            .expect("draw frame");
        let rendered = format!("{:?}", terminal.backend().buffer());

        assert!(rendered.contains("Themes"));
        assert!(rendered.contains("Query"));
        assert!(rendered.contains("midnight  current"));
        assert!(rendered.contains("high-contrast"));
    }

    #[test]
    fn key_event_flow_answers_question_option_from_dock() {
        #[derive(Default)]
        struct CaptureHandler {
            question_payloads: Vec<Value>,
        }

        impl TerminalEventHandler for CaptureHandler {
            fn handle_submit(&mut self, _prompt: &str) -> Result<Vec<TimelineLine>, String> {
                Ok(Vec::new())
            }

            fn handle_command(&mut self, _command: &str) -> Result<Vec<TimelineLine>, String> {
                Ok(Vec::new())
            }

            fn handle_question_response(
                &mut self,
                payload: &Value,
            ) -> Result<Vec<TimelineLine>, String> {
                self.question_payloads.push(payload.clone());
                Ok(vec![TimelineLine::new("status", "question sent", true)])
            }
        }

        let mut state = TuiState::new();
        let mut handler = CaptureHandler::default();
        state.apply_app_event(&json!({
            "method": "item/question/requested",
            "params": {
                "event": {
                    "request_id": "question_dock",
                    "turn_id": "turn_1",
                    "questions": [{
                        "header": "Plan",
                        "question": "Which option?",
                        "options": [
                            {"label": "Fast path", "description": "Move quickly"},
                            {"label": "Safe path", "description": "Be conservative"}
                        ]
                    }]
                }
            }
        }));

        handle_key_event(
            KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE),
            &mut state,
            &mut handler,
        )
        .expect("question dock key");

        assert_eq!(handler.question_payloads.len(), 1);
        assert_eq!(
            handler.question_payloads[0]["answers"],
            json!([["Safe path"]])
        );
        assert!(state.active_question.is_none());
        assert!(
            state
                .timeline
                .iter()
                .any(|line| line.text.contains("question sent"))
        );
    }

    #[test]
    fn key_event_flow_approves_from_dock() {
        #[derive(Default)]
        struct CaptureHandler {
            approval_payloads: Vec<Value>,
        }

        impl TerminalEventHandler for CaptureHandler {
            fn handle_submit(&mut self, _prompt: &str) -> Result<Vec<TimelineLine>, String> {
                Ok(Vec::new())
            }

            fn handle_command(&mut self, _command: &str) -> Result<Vec<TimelineLine>, String> {
                Ok(Vec::new())
            }

            fn handle_approval_response(
                &mut self,
                payload: &Value,
            ) -> Result<Vec<TimelineLine>, String> {
                self.approval_payloads.push(payload.clone());
                Ok(vec![TimelineLine::new("status", "approval sent", true)])
            }
        }

        let mut state = TuiState::new();
        let mut handler = CaptureHandler::default();
        state.apply_app_event(&json!({
            "method": "turn/approval_requested",
            "params": {
                "approval": {
                    "request_id": "approval_dock",
                    "turn_id": "turn_1",
                    "tool_name": "bash",
                    "tool_input": {"command": "printf ok"}
                }
            }
        }));

        handle_key_event(
            KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE),
            &mut state,
            &mut handler,
        )
        .expect("approval dock key");

        assert_eq!(handler.approval_payloads.len(), 1);
        assert_eq!(handler.approval_payloads[0]["action"], json!("allow"));
        assert_eq!(handler.approval_payloads[0]["scope"], json!("once"));
        assert!(state.active_approval.is_none());
    }

    #[test]
    fn terminal_render_snapshot_contains_core_regions() {
        let backend = TestBackend::new(80, 18);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let mut state = TuiState::new();
        state.status = "running".to_string();
        state
            .timeline
            .push(TimelineLine::new("user", "> hello", true));
        state
            .timeline
            .push(TimelineLine::new("assistant", "world", true));
        state.input_buffer = "next".to_string();

        terminal
            .draw(|frame| draw_terminal_frame(frame, "OpenAgent", &state))
            .expect("draw frame");
        let rendered = format!("{:?}", terminal.backend().buffer());

        assert!(rendered.contains("App Bridge"));
        assert!(rendered.contains("Timeline"));
        assert!(rendered.contains("Prompt"));
        assert!(rendered.contains("OpenAgent"));
        assert!(rendered.contains("world"));
    }

    fn send_key_text<H: TerminalEventHandler>(
        text: &str,
        state: &mut TuiState,
        handler: &mut H,
    ) -> Result<(), Box<dyn Error>> {
        for ch in text.chars() {
            press_key(KeyCode::Char(ch), state, handler)?;
        }
        Ok(())
    }

    fn press_key<H: TerminalEventHandler>(
        key: KeyCode,
        state: &mut TuiState,
        handler: &mut H,
    ) -> Result<(), Box<dyn Error>> {
        let exit = handle_key_event(KeyEvent::new(key, KeyModifiers::NONE), state, handler)
            .map_err(|error| std::io::Error::new(ErrorKind::Other, error))?;
        assert!(!exit, "test key unexpectedly requested TUI exit");
        Ok(())
    }

    fn temp_test_dir(prefix: &str) -> Result<PathBuf, Box<dyn Error>> {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_nanos()
            .to_string();
        let path = std::env::temp_dir().join(format!("{prefix}-{suffix}"));
        fs::create_dir_all(&path)?;
        Ok(path)
    }

    #[derive(Default)]
    struct FakeBridgeState {
        requests: Vec<String>,
        turn_inputs: Vec<String>,
        approval_payloads: Vec<Value>,
        question_payloads: Vec<Value>,
        model_update_payloads: Vec<Value>,
        agent_update_payloads: Vec<Value>,
        variant_update_payloads: Vec<Value>,
        thinking_update_payloads: Vec<Value>,
    }

    struct FakeAppBridge {
        server_url: String,
        state: Arc<Mutex<FakeBridgeState>>,
        shutdown: Arc<AtomicBool>,
        handle: Option<thread::JoinHandle<()>>,
    }

    impl FakeAppBridge {
        fn start() -> Result<Self, Box<dyn Error>> {
            let listener = TcpListener::bind(("127.0.0.1", 0))?;
            listener.set_nonblocking(true)?;
            let port = listener.local_addr()?.port();
            let state = Arc::new(Mutex::new(FakeBridgeState::default()));
            let shutdown = Arc::new(AtomicBool::new(false));
            let thread_state = Arc::clone(&state);
            let thread_shutdown = Arc::clone(&shutdown);
            let handle = thread::spawn(move || {
                while !thread_shutdown.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _addr)) => {
                            let _ = handle_fake_bridge_connection(stream, &thread_state);
                        }
                        Err(error) if error.kind() == ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(10));
                        }
                        Err(_error) => break,
                    }
                }
            });
            Ok(Self {
                server_url: format!("http://127.0.0.1:{port}"),
                state,
                shutdown,
                handle: Some(handle),
            })
        }

        fn requests(&self) -> Vec<String> {
            self.state.lock().expect("bridge state").requests.clone()
        }

        fn turn_inputs(&self) -> Vec<String> {
            self.state.lock().expect("bridge state").turn_inputs.clone()
        }

        fn approval_payloads(&self) -> Vec<Value> {
            self.state
                .lock()
                .expect("bridge state")
                .approval_payloads
                .clone()
        }

        fn question_payloads(&self) -> Vec<Value> {
            self.state
                .lock()
                .expect("bridge state")
                .question_payloads
                .clone()
        }

        fn model_update_payloads(&self) -> Vec<Value> {
            self.state
                .lock()
                .expect("bridge state")
                .model_update_payloads
                .clone()
        }

        fn agent_update_payloads(&self) -> Vec<Value> {
            self.state
                .lock()
                .expect("bridge state")
                .agent_update_payloads
                .clone()
        }

        fn variant_update_payloads(&self) -> Vec<Value> {
            self.state
                .lock()
                .expect("bridge state")
                .variant_update_payloads
                .clone()
        }

        fn thinking_update_payloads(&self) -> Vec<Value> {
            self.state
                .lock()
                .expect("bridge state")
                .thinking_update_payloads
                .clone()
        }

        fn stop(mut self) {
            self.shutdown.store(true, Ordering::Relaxed);
            let _ = TcpStream::connect(self.server_url.trim_start_matches("http://"));
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
        }
    }

    fn handle_fake_bridge_connection(
        mut stream: TcpStream,
        state: &Arc<Mutex<FakeBridgeState>>,
    ) -> Result<(), Box<dyn Error>> {
        let (method, path, body) = read_http_request(&mut stream)?;
        state
            .lock()
            .expect("bridge state")
            .requests
            .push(format!("{method} {path}"));
        match (method.as_str(), path.as_str()) {
            ("GET", "/api/health") => write_json(&mut stream, json!({"ok": true})),
            ("GET", "/api/models") => write_json(&mut stream, fake_models_payload()),
            ("GET", "/api/agents") => write_json(&mut stream, fake_agents_payload()),
            ("GET", "/api/sessions") => write_json(&mut stream, json!({"sessions": []})),
            ("GET", "/api/sessions?query=smoke") => {
                write_json(&mut stream, fake_session_search_payload())
            }
            ("PATCH", "/api/sessions/session_smoke") => {
                let payload = serde_json::from_str::<Value>(&body).unwrap_or_else(|_| json!({}));
                if payload.get("agent").is_some() {
                    state
                        .lock()
                        .expect("bridge state")
                        .agent_update_payloads
                        .push(payload.clone());
                }
                if payload.get("model").is_some() {
                    state
                        .lock()
                        .expect("bridge state")
                        .model_update_payloads
                        .push(payload.clone());
                }
                if payload.get("variant").is_some() {
                    state
                        .lock()
                        .expect("bridge state")
                        .variant_update_payloads
                        .push(payload.clone());
                }
                if payload.get("thinking").is_some() {
                    state
                        .lock()
                        .expect("bridge state")
                        .thinking_update_payloads
                        .push(payload.clone());
                }
                write_json(
                    &mut stream,
                    json!({
                        "session_id": "session_smoke",
                        "updated": true,
                        "session": {
                            "session_id": "session_smoke",
                            "id": "session_smoke",
                            "status": "idle",
                            "model": payload.get("model").cloned().unwrap_or(Value::Null),
                            "agent": payload.get("agent").cloned().unwrap_or(Value::Null),
                            "variant": payload.get("variant").cloned().unwrap_or(Value::Null),
                            "thinking": payload.get("thinking").cloned().unwrap_or(Value::Null),
                            "metadata": {
                                "model": payload.get("model").cloned().unwrap_or(Value::Null),
                                "agent": payload.get("agent").cloned().unwrap_or(Value::Null),
                                "variant": payload.get("variant").cloned().unwrap_or(Value::Null),
                                "thinking": payload.get("thinking").cloned().unwrap_or(Value::Null)
                            }
                        }
                    }),
                )
            }
            ("GET", "/api/sessions/session_smoke/messages?limit=2") => {
                write_json(&mut stream, fake_transcript_payload())
            }
            ("POST", "/api/sessions") => write_json(
                &mut stream,
                json!({
                    "session_id": "session_smoke",
                    "session": {
                        "session_id": "session_smoke",
                        "status": "ready",
                        "message_count": 0
                    }
                }),
            ),
            ("POST", "/api/sessions/session_smoke/turns") => {
                let input = serde_json::from_str::<Value>(&body)
                    .ok()
                    .and_then(|value| {
                        value
                            .get("input")
                            .and_then(Value::as_str)
                            .map(ToString::to_string)
                    })
                    .unwrap_or_default();
                state.lock().expect("bridge state").turn_inputs.push(input);
                write_json(
                    &mut stream,
                    json!({
                        "session_id": "session_smoke",
                        "turn_id": "turn_smoke",
                        "status": "completed",
                        "events": fake_turn_events(),
                    }),
                )
            }
            ("POST", "/api/turns/turn_approval/approvals/approval_smoke") => {
                let payload = serde_json::from_str::<Value>(&body).unwrap_or_else(|_| json!({}));
                state
                    .lock()
                    .expect("bridge state")
                    .approval_payloads
                    .push(payload);
                write_json(
                    &mut stream,
                    json!({
                        "session_id": "session_smoke",
                        "turn_id": "turn_approval",
                        "status": "completed",
                        "events": fake_approval_response_events(),
                    }),
                )
            }
            ("POST", "/api/turns/turn_question/questions/question_smoke/reply") => {
                let payload = serde_json::from_str::<Value>(&body).unwrap_or_else(|_| json!({}));
                state
                    .lock()
                    .expect("bridge state")
                    .question_payloads
                    .push(payload);
                write_json(
                    &mut stream,
                    json!({
                        "session_id": "session_smoke",
                        "turn_id": "turn_question",
                        "status": "completed",
                        "events": fake_question_response_events(),
                    }),
                )
            }
            _ if method == "GET" && path.starts_with("/api/events?last_event_id=") => {
                let last_event_id = path
                    .rsplit_once('=')
                    .and_then(|(_, value)| value.parse::<u64>().ok())
                    .unwrap_or_default();
                let events = if last_event_id < 4 {
                    vec![json!({
                        "method": "runtime/warning",
                        "global_sequence": 4,
                        "sequence": 4,
                        "params": {
                            "session_id": "session_smoke",
                            "turn_id": "turn_smoke",
                            "message": "bridge smoke warning"
                        }
                    })]
                } else {
                    Vec::new()
                };
                write_sse(&mut stream, &events)
            }
            _ => write_response(
                &mut stream,
                "404 Not Found",
                "application/json",
                &json!({"error": format!("unexpected route: {method} {path}")}).to_string(),
            ),
        }?;
        Ok(())
    }

    fn fake_turn_events() -> Vec<Value> {
        vec![
            json!({
                "method": "turn/started",
                "global_sequence": 1,
                "sequence": 1,
                "params": {
                    "session_id": "session_smoke",
                    "thread_id": "session_smoke",
                    "turn_id": "turn_smoke",
                    "status": "running"
                }
            }),
            json!({
                "method": "item/agentMessage/delta",
                "global_sequence": 2,
                "sequence": 2,
                "params": {
                    "session_id": "session_smoke",
                    "thread_id": "session_smoke",
                    "turn_id": "turn_smoke",
                    "delta": "bridge answer"
                }
            }),
            json!({
                "method": "turn/completed",
                "global_sequence": 3,
                "sequence": 3,
                "params": {
                    "session_id": "session_smoke",
                    "thread_id": "session_smoke",
                    "turn_id": "turn_smoke",
                    "status": "completed",
                    "final_answer": "bridge answer",
                    "usage": {
                        "input_tokens": 1,
                        "output_tokens": 2,
                        "total_tokens": 3,
                        "cost": 0.0
                    }
                }
            }),
        ]
    }

    fn fake_models_payload() -> Value {
        json!({
            "models": [
                {
                    "id": "server-local",
                    "provider_id": "openagent",
                    "name": "Server Local",
                    "default": true
                },
                {
                    "id": "deep-model",
                    "provider_id": "openagent",
                    "name": "Deep Model"
                }
            ],
            "variants": ["default", "deep"],
            "thinking": ["low", "high"]
        })
    }

    fn fake_agents_payload() -> Value {
        json!({
            "agents": [
                {
                    "id": "server",
                    "name": "Server",
                    "description": "Default server-backed coding agent",
                    "default": true
                },
                {
                    "id": "reviewer",
                    "name": "Reviewer",
                    "description": "Review code"
                }
            ]
        })
    }

    fn fake_session_search_payload() -> Value {
        json!({
            "sessions": [{
                "session_id": "session_smoke",
                "title": "Smoke Session",
                "status": "idle",
                "message_count": 3,
                "workspace": "/tmp/openagent-smoke"
            }]
        })
    }

    fn fake_transcript_payload() -> Value {
        json!({
            "session_id": "session_smoke",
            "message_count": 3,
            "limit": 2,
            "messages": [
                {
                    "index": 1,
                    "role": "assistant",
                    "content": "bridge answer",
                    "name": null,
                    "tool_call_id": null,
                    "metadata": {}
                },
                {
                    "index": 2,
                    "role": "user",
                    "content": "next question",
                    "name": null,
                    "tool_call_id": null,
                    "metadata": {}
                }
            ]
        })
    }

    fn fake_approval_response_events() -> Vec<Value> {
        vec![
            json!({
                "method": "turn/approval_resolved",
                "global_sequence": 10,
                "sequence": 10,
                "params": {
                    "session_id": "session_smoke",
                    "thread_id": "session_smoke",
                    "turn_id": "turn_approval",
                    "status": "running",
                    "approval": {
                        "request_id": "approval_smoke",
                        "turn_id": "turn_approval",
                        "session_id": "session_smoke",
                        "tool_name": "bash",
                        "action": "allow",
                        "scope": "once"
                    }
                }
            }),
            json!({
                "method": "turn/completed",
                "global_sequence": 11,
                "sequence": 11,
                "params": {
                    "session_id": "session_smoke",
                    "thread_id": "session_smoke",
                    "turn_id": "turn_approval",
                    "status": "completed",
                    "final_answer": "approved through bridge"
                }
            }),
        ]
    }

    fn fake_question_response_events() -> Vec<Value> {
        vec![
            json!({
                "method": "item/question/resolved",
                "global_sequence": 12,
                "sequence": 12,
                "params": {
                    "session_id": "session_smoke",
                    "thread_id": "session_smoke",
                    "turn_id": "turn_question",
                    "status": "answered",
                    "question": {
                        "request_id": "question_smoke",
                        "turn_id": "turn_question",
                        "session_id": "session_smoke"
                    }
                }
            }),
            json!({
                "method": "turn/completed",
                "global_sequence": 13,
                "sequence": 13,
                "params": {
                    "session_id": "session_smoke",
                    "thread_id": "session_smoke",
                    "turn_id": "turn_question",
                    "status": "completed",
                    "final_answer": "question bridge answer"
                }
            }),
        ]
    }

    fn read_http_request(
        stream: &mut TcpStream,
    ) -> Result<(String, String, String), Box<dyn Error>> {
        stream.set_read_timeout(Some(Duration::from_millis(500)))?;
        let mut raw = Vec::new();
        let mut buffer = [0_u8; 1024];
        let mut expected_len = None;
        loop {
            match stream.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => raw.extend_from_slice(&buffer[..count]),
                Err(error)
                    if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) =>
                {
                    break;
                }
                Err(error) => return Err(error.into()),
            }
            if expected_len.is_none()
                && let Some(header_end) = find_header_end(&raw)
            {
                let headers = String::from_utf8_lossy(&raw[..header_end]).to_string();
                let content_len = headers
                    .lines()
                    .find_map(|line| {
                        let (key, value) = line.split_once(':')?;
                        key.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .unwrap_or_default();
                expected_len = Some(header_end + content_len);
            }
            if let Some(expected_len) = expected_len
                && raw.len() >= expected_len
            {
                break;
            }
        }
        let header_end = find_header_end(&raw).ok_or("missing HTTP headers")?;
        let headers = String::from_utf8_lossy(&raw[..header_end]).to_string();
        let mut lines = headers.lines();
        let request_line = lines.next().ok_or("missing request line")?;
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or_default().to_string();
        let path = parts.next().unwrap_or_default().to_string();
        let body = String::from_utf8_lossy(&raw[header_end..]).to_string();
        Ok((method, path, body))
    }

    fn find_header_end(raw: &[u8]) -> Option<usize> {
        raw.windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|index| index + 4)
    }

    fn write_json(stream: &mut TcpStream, body: Value) -> Result<(), Box<dyn Error>> {
        write_response(stream, "200 OK", "application/json", &body.to_string())
    }

    fn write_sse(stream: &mut TcpStream, events: &[Value]) -> Result<(), Box<dyn Error>> {
        let mut body = String::new();
        for event in events {
            let method = event
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or("event");
            let id = event
                .get("global_sequence")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            body.push_str(&format!("event: {method}\nid: {id}\ndata: {event}\n\n"));
        }
        write_response(stream, "200 OK", "text/event-stream", &body)
    }

    fn write_response(
        stream: &mut TcpStream,
        status: &str,
        content_type: &str,
        body: &str,
    ) -> Result<(), Box<dyn Error>> {
        let response = format!(
            "HTTP/1.1 {status}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes())?;
        Ok(())
    }
}
