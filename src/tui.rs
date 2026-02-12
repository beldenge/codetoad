use crate::agent::{Agent, AgentEvent, ToolCallSummary};
use crate::settings::SettingsManager;
use crate::tools::{ToolResult, execute_bash_command};
use anyhow::{Context, Result};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event as CEvent, KeyCode, KeyEventKind,
    KeyModifiers, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap,
};
use ratatui::{Frame, Terminal};
use std::io::{self, Stdout};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;

const LOGO: &[&str] = &[
    "  /\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\",
    " /\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/",
    " \\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\",
    "  /\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\",
];
const DIRECT_COMMANDS: &[&str] = &[
    "ls", "pwd", "cd", "cat", "mkdir", "touch", "echo", "grep", "find", "cp", "mv", "rm",
];
const SUGGESTIONS: &[(&str, &str)] = &[
    ("/help", "Show help information"),
    ("/clear", "Clear chat history"),
    ("/models", "Switch model"),
    ("/commit-and-push", "AI commit and push"),
    ("/exit", "Exit application"),
];

#[derive(Debug, Clone)]
enum EntryKind {
    User,
    Assistant,
    ToolCall,
    ToolResult,
}

#[derive(Debug, Clone)]
struct Entry {
    kind: EntryKind,
    content: String,
    streaming: bool,
    tool: Option<ToolCallSummary>,
    success: Option<bool>,
}

impl Entry {
    fn user(content: String) -> Self {
        Self {
            kind: EntryKind::User,
            content,
            streaming: false,
            tool: None,
            success: None,
        }
    }
    fn assistant(content: String) -> Self {
        Self {
            kind: EntryKind::Assistant,
            content,
            streaming: false,
            tool: None,
            success: None,
        }
    }
    fn tool_call(tool: ToolCallSummary) -> Self {
        Self {
            kind: EntryKind::ToolCall,
            content: "Executing...".to_string(),
            streaming: false,
            tool: Some(tool),
            success: None,
        }
    }
    fn tool_result(tool: ToolCallSummary, result: ToolResult) -> Self {
        Self {
            kind: EntryKind::ToolResult,
            content: result.output.or(result.error).unwrap_or_default(),
            streaming: false,
            tool: Some(tool),
            success: Some(result.success),
        }
    }
}

#[derive(Debug)]
enum UiEvent {
    Agent(AgentEvent),
    Push(Entry),
    Done,
}

struct App {
    entries: Vec<Entry>,
    input: String,
    cursor: usize,
    history: Vec<String>,
    history_idx: Option<usize>,
    show_suggestions: bool,
    suggestion_idx: usize,
    show_models: bool,
    model_idx: usize,
    available_models: Vec<String>,
    current_model: String,
    processing: bool,
    started: Option<Instant>,
    auto_edit: bool,
    stream_idx: Option<usize>,
    cancel: Option<CancellationToken>,
    chat_scroll: usize,
    follow_tail: bool,
    quit: bool,
}

impl App {
    fn new(current_model: String, available_models: Vec<String>) -> Self {
        Self {
            entries: Vec::new(),
            input: String::new(),
            cursor: 0,
            history: Vec::new(),
            history_idx: None,
            show_suggestions: false,
            suggestion_idx: 0,
            show_models: false,
            model_idx: 0,
            available_models,
            current_model,
            processing: false,
            started: None,
            auto_edit: false,
            stream_idx: None,
            cancel: None,
            chat_scroll: 0,
            follow_tail: true,
            quit: false,
        }
    }
    fn reset_processing(&mut self) {
        self.processing = false;
        self.started = None;
        self.stream_idx = None;
        self.cancel = None;
    }
    fn filtered_suggestions(&self) -> Vec<(&'static str, &'static str)> {
        if !self.input.starts_with('/') {
            return Vec::new();
        }
        SUGGESTIONS
            .iter()
            .copied()
            .filter(|(cmd, _)| cmd.starts_with(&self.input))
            .collect()
    }
    fn refresh_suggestions(&mut self) {
        self.show_suggestions = self.input.starts_with('/');
        self.suggestion_idx = 0;
    }
}

pub async fn run_interactive(
    agent: Arc<Mutex<Agent>>,
    settings: Arc<Mutex<SettingsManager>>,
    initial_message: Option<String>,
) -> Result<()> {
    let current_model = agent.lock().await.current_model().to_string();
    let available_models = settings.lock().await.get_available_models();
    let mut app = App::new(current_model, available_models);

    let (input_tx, mut input_rx) = mpsc::unbounded_channel::<CEvent>();
    let (ui_tx, mut ui_rx) = mpsc::unbounded_channel::<UiEvent>();
    spawn_input_thread(input_tx);
    let mut terminal = setup_terminal()?;

    if let Some(msg) = initial_message {
        submit(
            msg,
            &mut app,
            agent.clone(),
            settings.clone(),
            ui_tx.clone(),
        )
        .await?;
    }

    let mut tick = tokio::time::interval(Duration::from_millis(60));
    let result = async {
        loop {
            terminal.draw(|frame| draw(frame, &app))?;
            tokio::select! {
                _ = tick.tick() => {}
                Some(event) = input_rx.recv() => {
                    on_input_event(event, &mut app, agent.clone(), settings.clone(), ui_tx.clone()).await?;
                }
                Some(event) = ui_rx.recv() => {
                    on_ui_event(event, &mut app);
                }
            }
            if app.quit {
                break;
            }
        }
        Ok::<(), anyhow::Error>(())
    }
    .await;

    restore_terminal(&mut terminal)?;
    print_session_transcript(&app.entries);
    result
}

fn spawn_input_thread(tx: mpsc::UnboundedSender<CEvent>) {
    thread::spawn(move || {
        while let Ok(event) = event::read() {
            if tx.send(event).is_err() {
                break;
            }
        }
    });
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    Ok(Terminal::new(CrosstermBackend::new(stdout))?)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode().context("disable raw mode")?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )
    .context("leave alt screen")?;
    terminal.show_cursor()?;
    Ok(())
}

async fn on_input_event(
    event: CEvent,
    app: &mut App,
    agent: Arc<Mutex<Agent>>,
    settings: Arc<Mutex<SettingsManager>>,
    ui_tx: mpsc::UnboundedSender<UiEvent>,
) -> Result<()> {
    if let CEvent::Mouse(mouse) = event {
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                app.follow_tail = false;
                app.chat_scroll = app.chat_scroll.saturating_sub(3);
            }
            MouseEventKind::ScrollDown => {
                if app.follow_tail {
                    // stay pinned at tail
                } else {
                    app.chat_scroll = app.chat_scroll.saturating_add(3);
                    let total = history_line_count(app);
                    if app.chat_scroll >= total.saturating_sub(1) {
                        app.follow_tail = true;
                    }
                }
            }
            _ => {}
        }
        return Ok(());
    }

    let CEvent::Key(key) = event else {
        return Ok(());
    };
    if key.kind != KeyEventKind::Press {
        return Ok(());
    }

    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        if app.input.is_empty() {
            app.quit = true;
        } else {
            app.input.clear();
            app.cursor = 0;
            app.refresh_suggestions();
        }
        return Ok(());
    }

    if key.code == KeyCode::BackTab {
        app.auto_edit = !app.auto_edit;
        return Ok(());
    }

    if app.show_models {
        match key.code {
            KeyCode::Esc => app.show_models = false,
            KeyCode::Up => {
                app.model_idx = if app.model_idx == 0 {
                    app.available_models.len().saturating_sub(1)
                } else {
                    app.model_idx.saturating_sub(1)
                }
            }
            KeyCode::Down => {
                if !app.available_models.is_empty() {
                    app.model_idx = (app.model_idx + 1) % app.available_models.len();
                }
            }
            KeyCode::Enter | KeyCode::Tab => {
                if !app.available_models.is_empty() {
                    let model = app.available_models[app.model_idx].clone();
                    agent.lock().await.set_model(model.clone());
                    settings.lock().await.update_project_model(&model)?;
                    app.current_model = model.clone();
                    app.entries
                        .push(Entry::assistant(format!("Switched to model: {model}")));
                    app.show_models = false;
                }
            }
            _ => {}
        }
        return Ok(());
    }

    if key.code == KeyCode::Esc {
        if app.processing {
            if let Some(token) = app.cancel.take() {
                token.cancel();
            }
            app.reset_processing();
            return Ok(());
        }
        app.show_suggestions = false;
        return Ok(());
    }

    if app.show_suggestions {
        let filtered = app.filtered_suggestions();
        if !filtered.is_empty() {
            match key.code {
                KeyCode::Up => {
                    app.suggestion_idx = if app.suggestion_idx == 0 {
                        filtered.len().saturating_sub(1)
                    } else {
                        app.suggestion_idx.saturating_sub(1)
                    };
                    return Ok(());
                }
                KeyCode::Down => {
                    app.suggestion_idx = (app.suggestion_idx + 1) % filtered.len();
                    return Ok(());
                }
                KeyCode::Tab => {
                    let command = filtered[app.suggestion_idx].0.to_string();
                    app.input = format!("{command} ");
                    app.cursor = app.input.len();
                    app.show_suggestions = false;
                    return Ok(());
                }
                _ => {}
            }
        }
    }

    match key.code {
        KeyCode::PageUp => {
            page_up_history(app);
        }
        KeyCode::PageDown => {
            page_down_history(app);
        }
        KeyCode::Enter => {
            let input = std::mem::take(&mut app.input);
            app.cursor = 0;
            app.refresh_suggestions();
            submit(input, app, agent, settings, ui_tx).await?;
        }
        KeyCode::Backspace => {
            if app.cursor > 0 {
                app.cursor -= 1;
                app.input.remove(app.cursor);
                app.refresh_suggestions();
            }
        }
        KeyCode::Delete => {
            if app.cursor < app.input.len() {
                app.input.remove(app.cursor);
                app.refresh_suggestions();
            }
        }
        KeyCode::Left => app.cursor = app.cursor.saturating_sub(1),
        KeyCode::Right => app.cursor = (app.cursor + 1).min(app.input.len()),
        KeyCode::Home => app.cursor = 0,
        KeyCode::End => app.cursor = app.input.len(),
        KeyCode::Up => history_up(app),
        KeyCode::Down => history_down(app),
        KeyCode::Char(c) => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                if c == 'u' {
                    app.input.clear();
                    app.cursor = 0;
                    app.refresh_suggestions();
                }
            } else {
                app.input.insert(app.cursor, c);
                app.cursor += 1;
                app.refresh_suggestions();
            }
        }
        _ => {}
    }

    Ok(())
}

async fn submit(
    raw: String,
    app: &mut App,
    agent: Arc<Mutex<Agent>>,
    settings: Arc<Mutex<SettingsManager>>,
    ui_tx: mpsc::UnboundedSender<UiEvent>,
) -> Result<()> {
    let message = raw.trim().to_string();
    if message.is_empty() {
        return Ok(());
    }
    if message == "exit" || message == "quit" || message == "/exit" {
        app.quit = true;
        return Ok(());
    }

    app.history.push(message.clone());
    app.history_idx = None;

    if message == "/clear" {
        app.entries.clear();
        app.reset_processing();
        return Ok(());
    }
    if message == "/help" {
        app.entries.push(Entry::assistant(help_text().to_string()));
        return Ok(());
    }
    if message == "/models" {
        app.show_models = true;
        app.model_idx = 0;
        return Ok(());
    }
    if let Some(model) = message.strip_prefix("/models ").map(str::trim) {
        if app.available_models.iter().any(|m| m == model) {
            agent.lock().await.set_model(model.to_string());
            settings.lock().await.update_project_model(model)?;
            app.current_model = model.to_string();
            app.entries
                .push(Entry::assistant(format!("Switched to model: {model}")));
        } else {
            app.entries.push(Entry::assistant(format!(
                "Invalid model: {model}\nAvailable models: {}",
                app.available_models.join(", ")
            )));
        }
        return Ok(());
    }
    if message == "/commit-and-push" {
        app.entries.push(Entry::user(message.clone()));
        start_commit_and_push(app, agent, ui_tx);
        return Ok(());
    }

    if is_direct_command(&message) {
        app.entries.push(Entry::user(message.clone()));
        app.processing = true;
        app.started = Some(Instant::now());
        tokio::spawn(async move {
            match execute_bash_command(&message).await {
                Ok(result) => {
                    let call = ToolCallSummary {
                        id: format!("bash_{}", now_millis()),
                        name: "bash".to_string(),
                        arguments: format!(r#"{{"command":"{}"}}"#, message.replace('"', "\\\"")),
                    };
                    ui_tx
                        .send(UiEvent::Push(Entry::tool_result(call, result)))
                        .ok();
                }
                Err(err) => {
                    ui_tx
                        .send(UiEvent::Push(Entry::assistant(format!(
                            "Error executing command: {err:#}"
                        ))))
                        .ok();
                }
            }
            ui_tx.send(UiEvent::Done).ok();
        });
        return Ok(());
    }

    app.entries.push(Entry::user(message.clone()));
    app.processing = true;
    app.started = Some(Instant::now());
    let cancel = CancellationToken::new();
    app.cancel = Some(cancel.clone());

    let (agent_tx, mut agent_rx) = mpsc::unbounded_channel::<AgentEvent>();
    let forward = ui_tx.clone();
    tokio::spawn(async move {
        while let Some(event) = agent_rx.recv().await {
            forward.send(UiEvent::Agent(event)).ok();
        }
    });

    tokio::spawn(async move {
        let result = agent
            .lock()
            .await
            .process_user_message_stream(message, cancel, agent_tx)
            .await;
        if let Err(err) = result {
            ui_tx
                .send(UiEvent::Agent(AgentEvent::Error(format!("{err:#}"))))
                .ok();
            ui_tx.send(UiEvent::Agent(AgentEvent::Done)).ok();
        }
    });

    Ok(())
}

fn start_commit_and_push(
    app: &mut App,
    agent: Arc<Mutex<Agent>>,
    ui_tx: mpsc::UnboundedSender<UiEvent>,
) {
    app.processing = true;
    app.started = Some(Instant::now());
    tokio::spawn(async move {
        let status = match execute_bash_command("git status --porcelain").await {
            Ok(r) => r,
            Err(err) => {
                ui_tx
                    .send(UiEvent::Push(Entry::assistant(format!(
                        "git status failed: {err:#}"
                    ))))
                    .ok();
                ui_tx.send(UiEvent::Done).ok();
                return;
            }
        };
        if !status.success
            || status
                .output
                .as_deref()
                .unwrap_or_default()
                .trim()
                .is_empty()
        {
            ui_tx
                .send(UiEvent::Push(Entry::assistant(
                    "No changes to commit.".to_string(),
                )))
                .ok();
            ui_tx.send(UiEvent::Done).ok();
            return;
        }

        let add = execute_bash_command("git add .").await.ok();
        if let Some(add) = add {
            let call = ToolCallSummary {
                id: format!("git_add_{}", now_millis()),
                name: "bash".to_string(),
                arguments: r#"{"command":"git add ."}"#.to_string(),
            };
            ui_tx
                .send(UiEvent::Push(Entry::tool_result(call, add)))
                .ok();
        }

        let diff = execute_bash_command("git diff --cached")
            .await
            .ok()
            .and_then(|r| r.output)
            .unwrap_or_default();
        let prompt = format!(
            "Generate a concise conventional commit message under 72 characters.\n\nGit Status:\n{}\n\nGit Diff:\n{}\n\nRespond with only the commit message.",
            status.output.unwrap_or_default(),
            diff
        );
        let commit_message = match agent.lock().await.generate_plain_text(&prompt).await {
            Ok(text) if !text.trim().is_empty() => text.trim().trim_matches('"').to_string(),
            _ => "chore: update project files".to_string(),
        };
        ui_tx
            .send(UiEvent::Push(Entry::assistant(format!(
                "Generated commit message: \"{commit_message}\""
            ))))
            .ok();

        let commit_cmd = format!("git commit -m \"{}\"", commit_message.replace('"', "\\\""));
        if let Ok(commit) = execute_bash_command(&commit_cmd).await {
            let call = ToolCallSummary {
                id: format!("git_commit_{}", now_millis()),
                name: "bash".to_string(),
                arguments: format!(r#"{{"command":"{}"}}"#, commit_cmd.replace('"', "\\\"")),
            };
            let success = commit.success;
            ui_tx
                .send(UiEvent::Push(Entry::tool_result(call, commit)))
                .ok();
            if success {
                let mut push_cmd = "git push".to_string();
                let mut push = execute_bash_command(&push_cmd).await.ok();
                if let Some(ref r) = push
                    && !r.success
                    && r.error
                        .as_deref()
                        .map(|e| e.contains("no upstream branch"))
                        .unwrap_or(false)
                {
                    push_cmd = "git push -u origin HEAD".to_string();
                    push = execute_bash_command(&push_cmd).await.ok();
                }
                if let Some(push) = push {
                    let call = ToolCallSummary {
                        id: format!("git_push_{}", now_millis()),
                        name: "bash".to_string(),
                        arguments: format!(r#"{{"command":"{}"}}"#, push_cmd.replace('"', "\\\"")),
                    };
                    ui_tx
                        .send(UiEvent::Push(Entry::tool_result(call, push)))
                        .ok();
                }
            }
        }
        ui_tx.send(UiEvent::Done).ok();
    });
}

fn on_ui_event(event: UiEvent, app: &mut App) {
    match event {
        UiEvent::Push(entry) => app.entries.push(entry),
        UiEvent::Done => app.reset_processing(),
        UiEvent::Agent(event) => match event {
            AgentEvent::Content(chunk) => {
                if let Some(index) = app.stream_idx {
                    if let Some(entry) = app.entries.get_mut(index) {
                        entry.content.push_str(&chunk);
                        entry.streaming = true;
                    }
                } else {
                    let mut entry = Entry::assistant(chunk);
                    entry.streaming = true;
                    app.entries.push(entry);
                    app.stream_idx = Some(app.entries.len().saturating_sub(1));
                }
            }
            AgentEvent::ToolCalls(calls) => {
                if let Some(index) = app.stream_idx.take()
                    && let Some(entry) = app.entries.get_mut(index)
                {
                    entry.streaming = false;
                }
                for call in calls {
                    app.entries.push(Entry::tool_call(call));
                }
            }
            AgentEvent::ToolResult { tool_call, result } => {
                let mut replaced = false;
                for entry in app.entries.iter_mut().rev() {
                    if matches!(entry.kind, EntryKind::ToolCall)
                        && entry
                            .tool
                            .as_ref()
                            .map(|c| c.id == tool_call.id)
                            .unwrap_or(false)
                    {
                        *entry = Entry::tool_result(tool_call.clone(), result.clone());
                        replaced = true;
                        break;
                    }
                }
                if !replaced {
                    app.entries.push(Entry::tool_result(tool_call, result));
                }
            }
            AgentEvent::Done => {
                if let Some(index) = app.stream_idx.take()
                    && let Some(entry) = app.entries.get_mut(index)
                {
                    entry.streaming = false;
                }
                app.reset_processing();
            }
            AgentEvent::Error(err) => {
                app.entries.push(Entry::assistant(format!("Error: {err}")));
                app.reset_processing();
            }
        },
    }
}

fn draw(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(LOGO.len() as u16 + 1),
            Constraint::Min(8),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(frame.area());

    let logo_lines = LOGO
        .iter()
        .enumerate()
        .map(|(i, line)| gradient(line, i))
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(logo_lines), chunks[0]);

    let lines = build_history_lines(app);
    let max_scroll = lines.len().saturating_sub(chunks[1].height as usize);
    let scroll = if app.follow_tail {
        max_scroll
    } else {
        app.chat_scroll.min(max_scroll)
    } as u16;
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        chunks[1],
    );

    let border = if app.processing {
        Color::Yellow
    } else {
        Color::Blue
    };
    let input = if app.input.is_empty() {
        Line::from(vec![
            Span::styled(">", Style::default().fg(Color::Cyan)),
            Span::raw(" "),
            Span::styled("Ask me anything...", Style::default().fg(Color::DarkGray)),
        ])
    } else {
        Line::from(vec![
            Span::styled(">", Style::default().fg(Color::Cyan)),
            Span::raw(format!(" {}", app.input)),
        ])
    };
    frame.render_widget(
        Paragraph::new(vec![input]).block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(border)),
        ),
        chunks[2],
    );

    let elapsed = app.started.map(|s| s.elapsed().as_secs()).unwrap_or(0);
    frame.render_widget(
        Paragraph::new(format!(
            "{} auto-edit: {} (shift + tab)   ~= {}{}",
            if app.auto_edit { "on" } else { "off" },
            if app.auto_edit { "enabled" } else { "disabled" },
            app.current_model,
            if app.processing {
                format!("   processing: {}s", elapsed)
            } else {
                String::new()
            }
        ))
        .style(Style::default().fg(Color::Cyan)),
        chunks[3],
    );

    if app.show_suggestions {
        let filtered = app.filtered_suggestions();
        if !filtered.is_empty() {
            let area = centered_rect(70, (filtered.len() as u16 + 2).min(8), frame.area());
            let items = filtered
                .iter()
                .map(|(cmd, desc)| {
                    ListItem::new(Line::from(vec![
                        Span::styled(*cmd, Style::default().fg(Color::Cyan)),
                        Span::raw(" "),
                        Span::styled(*desc, Style::default().fg(Color::DarkGray)),
                    ]))
                })
                .collect::<Vec<_>>();
            let mut state = ListState::default();
            state.select(Some(
                app.suggestion_idx.min(filtered.len().saturating_sub(1)),
            ));
            frame.render_widget(Clear, area);
            frame.render_stateful_widget(
                List::new(items)
                    .block(
                        Block::default()
                            .title("Commands")
                            .borders(Borders::ALL)
                            .border_type(BorderType::Rounded),
                    )
                    .highlight_style(Style::default().fg(Color::Yellow)),
                area,
                &mut state,
            );
        }
    }

    if app.show_models {
        let area = centered_rect(70, 12, frame.area());
        let items = app
            .available_models
            .iter()
            .map(|model| ListItem::new(Line::from(model.clone())))
            .collect::<Vec<_>>();
        let mut state = ListState::default();
        state.select(Some(
            app.model_idx
                .min(app.available_models.len().saturating_sub(1)),
        ));
        frame.render_widget(Clear, area);
        frame.render_stateful_widget(
            List::new(items)
                .block(
                    Block::default()
                        .title("Select Model")
                        .borders(Borders::ALL)
                        .border_type(BorderType::Rounded),
                )
                .highlight_style(Style::default().fg(Color::Yellow)),
            area,
            &mut state,
        );
    }
}

fn centered_rect(percent_x: u16, height: u16, area: Rect) -> Rect {
    let width = area.width.saturating_mul(percent_x).saturating_div(100);
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect::new(x, y, width, height.min(area.height))
}

fn build_history_lines(app: &App) -> Vec<Line<'static>> {
    let mut lines = Vec::<Line>::new();
    if app.entries.is_empty() {
        lines.push(Line::from(vec![Span::styled(
            "Tips for getting started:",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )]));
        lines.push(Line::from("1. Ask questions, edit files, or run commands."));
        lines.push(Line::from("2. Be specific for the best results."));
        lines.push(Line::from("3. Create GROK.md files to customize behavior."));
        lines.push(Line::from("4. Press Shift+Tab to toggle auto-edit mode."));
        lines.push(Line::from("5. /help for more information."));
    } else {
        for entry in &app.entries {
            lines.extend(entry_lines(entry));
        }
    }
    lines
}

fn gradient(line: &str, index: usize) -> Line<'static> {
    let spans = line
        .chars()
        .enumerate()
        .map(|(i, ch)| {
            let t = (i + (index * 3)) as f32 / (line.len().max(1) as f32 + 8.0);
            let r = (255.0 * (1.0 - t)).clamp(0.0, 255.0) as u8;
            let g = (255.0 * t).clamp(0.0, 255.0) as u8;
            Span::styled(ch.to_string(), Style::default().fg(Color::Rgb(r, g, 255)))
        })
        .collect::<Vec<_>>();
    Line::from(spans)
}

fn entry_lines(entry: &Entry) -> Vec<Line<'static>> {
    match entry.kind {
        EntryKind::User => split_prefixed_lines("> ", &entry.content, Color::Reset),
        EntryKind::Assistant => {
            let mut lines = split_prefixed_lines("o ", &entry.content, Color::Reset);
            if entry.streaming {
                lines.push(Line::from("|"));
            }
            lines
        }
        EntryKind::ToolCall | EntryKind::ToolResult => {
            let tool = entry.tool.clone().unwrap_or(ToolCallSummary {
                id: "unknown".to_string(),
                name: "tool".to_string(),
                arguments: String::new(),
            });
            let label = format!("{}({})", pretty_tool_name(&tool.name), tool_path(&tool));
            let color = if entry.success.unwrap_or(true) {
                Color::Gray
            } else {
                Color::Red
            };
            let mut lines = vec![Line::from(format!("o {label}"))];
            lines.extend(split_prefixed_lines("  -> ", &entry.content, color));
            lines
        }
    }
}

fn split_prefixed_lines(prefix: &str, content: &str, color: Color) -> Vec<Line<'static>> {
    if content.is_empty() {
        return vec![Line::from(prefix.to_string())];
    }

    let mut lines = Vec::new();
    let normalized = content.replace("\r\n", "\n");
    for (index, line) in normalized.split('\n').enumerate() {
        let current_prefix = if index == 0 {
            prefix.to_string()
        } else {
            " ".repeat(prefix.len())
        };

        let text = format!("{current_prefix}{line}");
        if color == Color::Reset {
            lines.push(Line::from(text));
        } else {
            lines.push(Line::from(vec![Span::styled(
                text,
                Style::default().fg(color),
            )]));
        }
    }

    lines
}

fn pretty_tool_name(name: &str) -> &str {
    match name {
        "view_file" => "Read",
        "str_replace_editor" => "Update",
        "create_file" => "Create",
        "bash" => "Bash",
        _ => "Tool",
    }
}

fn tool_path(tool: &ToolCallSummary) -> String {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&tool.arguments) {
        return value
            .get("path")
            .and_then(serde_json::Value::as_str)
            .or_else(|| value.get("command").and_then(serde_json::Value::as_str))
            .unwrap_or("")
            .to_string();
    }
    String::new()
}

fn history_up(app: &mut App) {
    if app.history.is_empty() {
        return;
    }
    let idx = app
        .history_idx
        .map(|i| i.saturating_sub(1))
        .unwrap_or_else(|| app.history.len().saturating_sub(1));
    app.history_idx = Some(idx);
    app.input = app.history[idx].clone();
    app.cursor = app.input.len();
    app.refresh_suggestions();
}

fn history_down(app: &mut App) {
    if app.history.is_empty() {
        return;
    }
    match app.history_idx {
        None => {}
        Some(i) if i + 1 >= app.history.len() => {
            app.history_idx = None;
            app.input.clear();
            app.cursor = 0;
        }
        Some(i) => {
            app.history_idx = Some(i + 1);
            app.input = app.history[i + 1].clone();
            app.cursor = app.input.len();
        }
    }
    app.refresh_suggestions();
}

fn page_up_history(app: &mut App) {
    let total = history_line_count(app);
    if total == 0 {
        return;
    }
    let current = if app.follow_tail {
        total.saturating_sub(1)
    } else {
        app.chat_scroll
    };
    app.chat_scroll = current.saturating_sub(10);
    app.follow_tail = false;
}

fn page_down_history(app: &mut App) {
    let total = history_line_count(app);
    if total == 0 {
        app.follow_tail = true;
        app.chat_scroll = 0;
        return;
    }
    let current = if app.follow_tail {
        total.saturating_sub(1)
    } else {
        app.chat_scroll
    };
    let next = current.saturating_add(10);
    if next >= total.saturating_sub(1) {
        app.follow_tail = true;
    } else {
        app.chat_scroll = next;
        app.follow_tail = false;
    }
}

fn history_line_count(app: &App) -> usize {
    if app.entries.is_empty() {
        6
    } else {
        app.entries
            .iter()
            .map(|entry| entry_lines(entry).len())
            .sum()
    }
}

fn is_direct_command(input: &str) -> bool {
    let first = input.split_whitespace().next().unwrap_or_default();
    DIRECT_COMMANDS.contains(&first)
}

fn help_text() -> &'static str {
    "Grok Build Help:\n\n/clear\n/help\n/models\n/models <name>\n/commit-and-push\n/exit\n\nNavigation: mouse wheel + PageUp/PageDown scroll history, Esc cancels active generation\n\nDirect commands: ls, pwd, cd, cat, mkdir, touch, echo, grep, find, cp, mv, rm"
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn print_session_transcript(entries: &[Entry]) {
    if entries.is_empty() {
        return;
    }

    println!();
    println!("Session transcript:");
    for entry in entries {
        match entry.kind {
            EntryKind::User => {
                println!("> {}", entry.content);
            }
            EntryKind::Assistant => {
                if entry.content.is_empty() {
                    println!("o");
                } else {
                    for (i, line) in entry.content.replace("\r\n", "\n").split('\n').enumerate() {
                        if i == 0 {
                            println!("o {line}");
                        } else {
                            println!("  {line}");
                        }
                    }
                }
            }
            EntryKind::ToolCall | EntryKind::ToolResult => {
                let label = entry
                    .tool
                    .as_ref()
                    .map(|tool| format!("{}({})", pretty_tool_name(&tool.name), tool_path(tool)))
                    .unwrap_or_else(|| "Tool".to_string());
                println!("o {label}");
                for line in entry.content.replace("\r\n", "\n").split('\n') {
                    println!("  -> {line}");
                }
            }
        }
    }
}
