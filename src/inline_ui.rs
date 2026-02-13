use crate::agent::{AgentEvent, ConfirmationDecision, ToolCallSummary};
use crate::app_context::AppContext;
use crate::confirmation::ConfirmationOperation;
use crate::git_ops::{
    CommitAndPushEvent, CommitAndPushOptions, CommitAndPushStep,
    run_commit_and_push as run_commit_and_push_flow,
};
use crate::image_input::prepare_user_input;
use crate::inline_feedback::{
    print_logo_and_tips, print_tool_result, prompt_tool_confirmation, tool_label,
};
use crate::inline_markdown::{
    MarkdownStreamRenderer, flush_markdown_pending, stream_markdown_chunk,
};
use crate::inline_prompt::{read_prompt_line, select_model_inline, select_option_inline};
use crate::session_store::{list_sessions, load_session};
use crate::slash_commands::{
    CommandGroup, ParsedSlashCommand, append_help_section, parse_slash_command,
};
use crate::tools::ToolResult;
use anyhow::Result;
use crossterm::event::{
    self, DisableMouseCapture, Event as CEvent, KeyCode, KeyEventKind, KeyModifiers,
};
use crossterm::execute;
use crossterm::style::Stylize;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use std::collections::HashMap;
use std::io::{self, Write};
use std::time::Instant;
use tokio::sync::mpsc;
use tokio::time::{self, Duration};
use tokio_util::sync::CancellationToken;

const DIRECT_COMMANDS: &[&str] = &[
    "ls", "pwd", "cd", "cat", "mkdir", "touch", "echo", "grep", "find", "cp", "mv", "rm",
];
const STATUS_FRAMES: &[&str] = &["-", "\\", "|", "/"];

struct StreamRawModeGuard;

impl StreamRawModeGuard {
    fn new() -> Result<Self> {
        enable_raw_mode()?;
        Ok(Self)
    }
}

impl Drop for StreamRawModeGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
    }
}

pub async fn run_inline(app: AppContext, initial_message: Option<String>) -> Result<()> {
    recover_terminal_state();
    print_logo_and_tips();
    println!("{}", format!("cwd: {}", app.cwd().display()).dark_grey());
    println!();
    let _ = app.autosave_session().await?;
    let mut history: Vec<String> = Vec::new();
    let mut auto_edit = app.auto_edit_enabled().await;
    let mut synced_auto_edit = auto_edit;
    let mut current_model = app.agent().lock().await.current_model().to_string();

    if let Some(initial) = initial_message {
        history.push(initial.clone());
        if auto_edit != synced_auto_edit {
            app.set_auto_edit_enabled(auto_edit).await;
            synced_auto_edit = auto_edit;
        }
        handle_input(&initial, auto_edit, app.clone()).await?;
        let _ = app.autosave_session().await?;
        current_model = app.agent().lock().await.current_model().to_string();
    }

    loop {
        let Some(input) = read_prompt_line(&history, &mut auto_edit, &current_model)? else {
            break;
        };
        let input = input.trim().to_string();
        if input.is_empty() {
            continue;
        }
        if input == "exit" || input == "quit" || input == "/exit" {
            break;
        }
        history.push(input.clone());
        if auto_edit != synced_auto_edit {
            app.set_auto_edit_enabled(auto_edit).await;
            synced_auto_edit = auto_edit;
        }
        handle_input(&input, auto_edit, app.clone()).await?;
        let _ = app.autosave_session().await?;
        current_model = app.agent().lock().await.current_model().to_string();
    }

    Ok(())
}

fn recover_terminal_state() {
    let _ = disable_raw_mode();
    let mut stdout = io::stdout();
    let _ = execute!(stdout, DisableMouseCapture);
}

async fn handle_input(input: &str, auto_edit_enabled: bool, app: AppContext) -> Result<()> {
    if let Some(command) = parse_slash_command(input) {
        return handle_slash_command(command, app).await;
    }

    if is_direct_command(input) {
        return handle_direct_command(input, auto_edit_enabled, app).await;
    }

    let prepared = prepare_user_input(input, app.cwd());
    for warning in prepared.warnings {
        println!("{}", format!("warning: {warning}").yellow());
    }
    if !prepared.attachments.is_empty() {
        for attachment in &prepared.attachments {
            println!(
                "{} {}",
                "◦".magenta(),
                format!(
                    "attached image: {} ({})",
                    attachment.display_path,
                    format_bytes(attachment.size_bytes)
                )
                .dark_grey()
            );
        }
    }

    stream_agent_message(
        prepared.text,
        prepared
            .attachments
            .into_iter()
            .map(|attachment| attachment.chat_attachment)
            .collect(),
        app,
    )
    .await
}

async fn handle_slash_command(command: ParsedSlashCommand, app: AppContext) -> Result<()> {
    match command {
        ParsedSlashCommand::Help => {
            println!("{}", help_text());
        }
        ParsedSlashCommand::Clear => {
            app.agent().lock().await.reset_conversation();
            clear_screen();
            print_logo_and_tips();
        }
        ParsedSlashCommand::Models => {
            let settings = app.settings();
            let agent = app.agent();
            let available = settings.lock().await.get_available_models();
            let current = agent.lock().await.current_model().to_string();
            match select_model_inline(&available, &current)? {
                Some(model) => {
                    set_active_model(model, app.clone()).await?;
                }
                None => {
                    println!("Model selection cancelled.");
                }
            }
        }
        ParsedSlashCommand::SetModel(model) => {
            let settings = app.settings();
            let available = settings.lock().await.get_available_models();
            if available.iter().any(|candidate| candidate == &model) {
                set_active_model(model, app.clone()).await?;
            } else {
                println!("Invalid model: {model}");
                println!("Available: {}", available.join(", "));
            }
        }
        ParsedSlashCommand::Resume => {
            let sessions = list_sessions(app.cwd())?;
            let current = app.active_session_name().await;
            let Some(selected) = select_option_inline(
                "Resume session",
                &sessions,
                current.as_deref(),
                "No saved sessions.",
            )?
            else {
                println!("Resume cancelled.");
                return Ok(());
            };
            let name = selected;
            let snapshot = load_session(app.cwd(), &name)?;
            app.agent()
                .lock()
                .await
                .restore_session_snapshot(snapshot)?;
            app.sync_auto_edit_from_agent().await;
            app.set_active_session_name(name.clone()).await;
            println!("Loaded session: {name}");
        }
        ParsedSlashCommand::CommitAndPush => {
            run_commit_and_push(app.clone()).await?;
        }
        ParsedSlashCommand::Exit => {}
    }
    Ok(())
}

async fn set_active_model(model: String, app: AppContext) -> Result<()> {
    let agent = app.agent();
    let settings = app.settings();
    agent.lock().await.set_model(model.clone());
    settings.lock().await.update_project_model(&model)?;
    println!("Switched to model: {model}");
    Ok(())
}

async fn handle_direct_command(
    input: &str,
    auto_edit_enabled: bool,
    app: AppContext,
) -> Result<()> {
    let agent = app.agent();
    let tool_call = ToolCallSummary {
        id: "bash_inline_direct".to_string(),
        name: "bash".to_string(),
        arguments: format!(r#"{{"command":"{}"}}"#, input.replace('"', "\\\"")),
    };

    let auto_approved = {
        agent
            .lock()
            .await
            .is_operation_auto_approved(ConfirmationOperation::Bash)
    };
    if !auto_edit_enabled && !auto_approved {
        match prompt_tool_confirmation(&tool_call, ConfirmationOperation::Bash)? {
            ConfirmationDecision::Approve {
                remember_for_session,
                ..
            } => {
                if remember_for_session {
                    agent
                        .lock()
                        .await
                        .remember_operation_for_session(ConfirmationOperation::Bash);
                }
            }
            ConfirmationDecision::Reject { .. } => {
                print_tool_result(tool_call, ToolResult::err("Operation cancelled by user"));
                return Ok(());
            }
        }
    }

    let result = agent.lock().await.execute_bash_command(input).await?;
    print_tool_result(tool_call, result);
    Ok(())
}

async fn stream_agent_message(
    message: String,
    attachments: Vec<crate::protocol::ChatImageAttachment>,
    app: AppContext,
) -> Result<()> {
    let _raw_guard = StreamRawModeGuard::new()?;
    let cancel_token = CancellationToken::new();
    let (agent_tx, mut agent_rx) = mpsc::unbounded_channel::<AgentEvent>();
    let (confirm_tx, confirm_rx) = mpsc::unbounded_channel::<ConfirmationDecision>();
    let confirm_rx = std::sync::Arc::new(tokio::sync::Mutex::new(confirm_rx));

    let error_tx = agent_tx.clone();
    let task_cancel_token = cancel_token.clone();
    let task_confirm_rx = confirm_rx.clone();
    let agent_for_task = app.agent();
    tokio::spawn(async move {
        let result = agent_for_task
            .lock()
            .await
            .process_user_message_stream(
                message,
                attachments,
                task_cancel_token,
                agent_tx,
                Some(task_confirm_rx),
            )
            .await;
        if let Err(err) = result {
            error_tx.send(AgentEvent::Error(format!("{err:#}"))).ok();
            error_tx.send(AgentEvent::Done).ok();
        }
    });

    let mut started_content = false;
    let mut phase = "thinking";
    let mut tool_calls_seen = 0usize;
    let mut tool_results_seen = 0usize;
    let mut frame_idx = 0usize;
    let mut status_width = 0usize;
    let started_at = Instant::now();
    let mut status_tick = time::interval(Duration::from_millis(120));
    let mut renderer = MarkdownStreamRenderer::default();
    let mut tool_started_at: HashMap<String, Instant> = HashMap::new();
    let mut tool_succeeded = 0usize;
    let mut tool_failed = 0usize;
    let mut cancel_requested = false;
    let mut token_count = 0usize;

    loop {
        let event = tokio::select! {
            _ = status_tick.tick() => {
                if !cancel_requested && poll_cancel_request()? {
                    cancel_requested = true;
                    phase = "cancelling";
                    cancel_token.cancel();
                }

                if !started_content {
                    let elapsed = started_at.elapsed().as_secs();
                    let progress = if phase == "running tools" && tool_calls_seen > 0 {
                        format!(
                            " ({}/{})",
                            tool_results_seen.min(tool_calls_seen),
                            tool_calls_seen
                        )
                    } else {
                        String::new()
                    };
                    let status = format!(
                        "{} {}{}... {}s · ↑ {} tok",
                        STATUS_FRAMES[frame_idx % STATUS_FRAMES.len()],
                        phase,
                        progress,
                        elapsed,
                        format_token_count(token_count)
                    );
                    frame_idx = frame_idx.wrapping_add(1);
                    render_status_line(&status, &mut status_width)?;
                }
                continue;
            }
            maybe_event = agent_rx.recv() => maybe_event,
        };

        let Some(event) = event else {
            finalize_stream_output(&mut started_content, &mut renderer, &mut status_width)?;
            break;
        };

        match event {
            AgentEvent::Content(chunk) => {
                if !started_content {
                    clear_status_line(&mut status_width)?;
                    print!("{} ", "●".white());
                    started_content = true;
                }
                stream_markdown_chunk(&mut renderer, &chunk)?;
            }
            AgentEvent::TokenCount(count) => {
                token_count = count;
            }
            AgentEvent::ConfirmationRequest {
                tool_call,
                operation,
            } => {
                prepare_for_aux_output(&mut started_content, &mut renderer, &mut status_width)?;
                let decision = prompt_tool_confirmation(&tool_call, operation)?;
                confirm_tx.send(decision).ok();
            }
            AgentEvent::ToolCalls(calls) => {
                prepare_for_aux_output(&mut started_content, &mut renderer, &mut status_width)?;
                phase = "running tools";
                tool_calls_seen += calls.len();
                for call in calls {
                    let label = tool_label(&call);
                    println!("{} {}", "◦".magenta(), format!("start {label}").dark_grey());
                    println!("{} {}", "●".magenta(), label.white());
                    println!("{}", "  -> Executing...".cyan());
                    tool_started_at.insert(call.id.clone(), Instant::now());
                }
            }
            AgentEvent::ToolResult { tool_call, result } => {
                prepare_for_aux_output(&mut started_content, &mut renderer, &mut status_width)?;
                tool_results_seen = tool_results_seen.saturating_add(1);
                phase = if tool_calls_seen == 0 {
                    "running tools"
                } else if tool_results_seen >= tool_calls_seen {
                    "finalizing"
                } else {
                    "running tools"
                };
                let label = tool_label(&tool_call);
                let elapsed = tool_started_at
                    .remove(&tool_call.id)
                    .map(|start| format_elapsed(start.elapsed()))
                    .unwrap_or_else(|| "n/a".to_string());
                if result.success {
                    tool_succeeded = tool_succeeded.saturating_add(1);
                    println!(
                        "{} {}",
                        "◦".magenta(),
                        format!("done {label} in {elapsed}").dark_green()
                    );
                } else {
                    tool_failed = tool_failed.saturating_add(1);
                    println!(
                        "{} {}",
                        "◦".magenta(),
                        format!("failed {label} in {elapsed}").red()
                    );
                }
                print_tool_result(tool_call, result);
            }
            AgentEvent::Done => {
                finalize_stream_output(&mut started_content, &mut renderer, &mut status_width)?;
                if tool_calls_seen > 0 {
                    println!(
                        "{}",
                        format!(
                            "◦ tools summary: {} total, {} succeeded, {} failed",
                            tool_calls_seen, tool_succeeded, tool_failed
                        )
                        .dark_grey()
                    );
                }
                let elapsed = started_at.elapsed();
                println!(
                    "{}",
                    format!(
                        "● completed in {}.{:01}s · ↑ {} tok",
                        elapsed.as_secs(),
                        elapsed.subsec_millis() / 100,
                        format_token_count(token_count)
                    )
                    .dark_grey()
                );
                break;
            }
            AgentEvent::Error(err) => {
                finalize_stream_output(&mut started_content, &mut renderer, &mut status_width)?;
                if tool_calls_seen > 0 {
                    println!(
                        "{}",
                        format!(
                            "◦ tools summary: {} total, {} succeeded, {} failed",
                            tool_calls_seen, tool_succeeded, tool_failed
                        )
                        .dark_grey()
                    );
                }
                println!("{}", format!("Error: {err}").red());
                break;
            }
        }
    }

    Ok(())
}

fn prepare_for_aux_output(
    started_content: &mut bool,
    renderer: &mut MarkdownStreamRenderer,
    status_width: &mut usize,
) -> io::Result<()> {
    if *started_content {
        flush_markdown_pending(renderer)?;
        println!();
        *started_content = false;
    }
    clear_status_line(status_width)
}

fn finalize_stream_output(
    started_content: &mut bool,
    renderer: &mut MarkdownStreamRenderer,
    status_width: &mut usize,
) -> io::Result<()> {
    clear_status_line(status_width)?;
    if *started_content {
        flush_markdown_pending(renderer)?;
        println!();
        *started_content = false;
    }
    Ok(())
}

async fn run_commit_and_push(app: AppContext) -> Result<()> {
    println!("Running commit-and-push...");

    let outcome = run_commit_and_push_flow(
        app.agent(),
        CommitAndPushOptions {
            default_commit_message: Some("chore: update project files".to_string()),
        },
        |event| match event {
            CommitAndPushEvent::NoChanges => {
                println!("No changes to commit.");
            }
            CommitAndPushEvent::ChangesDetected => {}
            CommitAndPushEvent::GeneratedMessage(message) => {
                println!("Generated commit message: \"{message}\"");
            }
            CommitAndPushEvent::ToolResult {
                step,
                command,
                result,
            } => {
                let id = match step {
                    CommitAndPushStep::Add => "git_add_inline",
                    CommitAndPushStep::Commit => "git_commit_inline",
                    CommitAndPushStep::Push => "git_push_inline",
                };
                print_tool_result(
                    ToolCallSummary {
                        id: id.to_string(),
                        name: "bash".to_string(),
                        arguments: format!(r#"{{"command":"{}"}}"#, command.replace('"', "\\\"")),
                    },
                    result,
                );
            }
        },
    )
    .await?;

    if matches!(outcome, crate::git_ops::CommitAndPushOutcome::NoChanges) {
        return Ok(());
    }

    Ok(())
}

fn is_direct_command(input: &str) -> bool {
    let first = input.split_whitespace().next().unwrap_or_default();
    DIRECT_COMMANDS.contains(&first)
}

fn help_text() -> String {
    let mut output = String::from("Grok Build Help:\n\n");
    append_help_section(&mut output, "Built-in Commands", CommandGroup::BuiltIn);
    output.push('\n');
    append_help_section(&mut output, "Git Commands", CommandGroup::Git);
    output.push_str(
        "\nDirect Commands:\n  ls, pwd, cd, cat, mkdir, touch, echo, grep, find, cp, mv, rm\n\n\
Input Controls:\n  Up/Down       History (or command suggestion selection)\n  Left/Right    Move cursor\n  Tab           Accept command suggestion\n  Shift+Tab     Toggle auto-edit mode (bypass confirmations)\n  Enter         Submit input (or accept suggestion when / command hints are visible)\n  Ctrl+A/E      Start/end of line\n  Ctrl+U/W      Delete to start / delete previous word\n  Ctrl+C        Clear input (press twice on empty input to exit)\n\n\
Image Input:\n  Drag/drop image paths or include markdown image links (`![alt](path/to/image.png)`)\n  Detected images are attached automatically and listed before submit\n\n\
Confirmation Controls:\n  y             Approve operation once\n  a             Approve this operation type for session\n  n / Esc       Reject operation\n\n\
Active Generation Controls:\n  Esc or Ctrl+C Cancel the current generation/tool loop\n\n\
Inline mode keeps native terminal scrollback, shows live elapsed + token status while working, and preserves output after Ctrl+C.",
    );
    output
}

fn clear_screen() {
    print!("\x1b[2J\x1b[H");
    let _ = io::stdout().flush();
}

fn render_status_line(text: &str, prev_width: &mut usize) -> io::Result<()> {
    let width = text.chars().count();
    let padding = prev_width.saturating_sub(width);
    print!("\r{}{}", text.dark_grey(), " ".repeat(padding));
    io::stdout().flush()?;
    *prev_width = width;
    Ok(())
}

fn clear_status_line(prev_width: &mut usize) -> io::Result<()> {
    if *prev_width > 0 {
        print!("\r{}\r", " ".repeat(*prev_width));
        io::stdout().flush()?;
        *prev_width = 0;
    }
    Ok(())
}

fn format_elapsed(elapsed: std::time::Duration) -> String {
    let secs = elapsed.as_secs();
    let tenths = elapsed.subsec_millis() / 100;
    format!("{secs}.{tenths}s")
}

fn format_token_count(tokens: usize) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}k", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

fn format_bytes(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

fn poll_cancel_request() -> Result<bool> {
    if !event::poll(std::time::Duration::from_millis(0))? {
        return Ok(false);
    }
    let event = event::read()?;
    let CEvent::Key(key) = event else {
        return Ok(false);
    };
    if key.kind != KeyEventKind::Press {
        return Ok(false);
    }

    Ok(key.code == KeyCode::Esc
        || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)))
}
