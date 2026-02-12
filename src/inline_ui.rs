use crate::agent::{Agent, AgentEvent, ToolCallSummary};
use crate::settings::SettingsManager;
use crate::tools::{ToolResult, execute_bash_command};
use anyhow::Result;
use crossterm::event::DisableMouseCapture;
use crossterm::execute;
use crossterm::style::Stylize;
use crossterm::terminal::disable_raw_mode;
use std::io::{self, Write};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, mpsc};
use tokio::time::{self, Duration};
use tokio_util::sync::CancellationToken;

const DIRECT_COMMANDS: &[&str] = &[
    "ls", "pwd", "cd", "cat", "mkdir", "touch", "echo", "grep", "find", "cp", "mv", "rm",
];
const STATUS_FRAMES: &[&str] = &["-", "\\", "|", "/"];

pub async fn run_inline(
    agent: Arc<Mutex<Agent>>,
    settings: Arc<Mutex<SettingsManager>>,
    initial_message: Option<String>,
) -> Result<()> {
    recover_terminal_state();
    print_logo_and_tips();

    if let Some(initial) = initial_message {
        handle_input(&initial, agent.clone(), settings.clone()).await?;
    }

    loop {
        print!("{} ", ">".cyan());
        io::stdout().flush()?;

        let mut line = String::new();
        let read = io::stdin().read_line(&mut line)?;
        if read == 0 {
            break;
        }
        let input = line.trim().to_string();
        if input.is_empty() {
            continue;
        }
        if input == "exit" || input == "quit" || input == "/exit" {
            break;
        }
        handle_input(&input, agent.clone(), settings.clone()).await?;
    }

    Ok(())
}

fn recover_terminal_state() {
    let _ = disable_raw_mode();
    let mut stdout = io::stdout();
    let _ = execute!(stdout, DisableMouseCapture);
}

async fn handle_input(
    input: &str,
    agent: Arc<Mutex<Agent>>,
    settings: Arc<Mutex<SettingsManager>>,
) -> Result<()> {
    if input == "/help" {
        println!("{}", help_text());
        return Ok(());
    }

    if input == "/clear" {
        agent.lock().await.reset_conversation();
        clear_screen();
        print_logo_and_tips();
        return Ok(());
    }

    if input == "/models" {
        let available = settings.lock().await.get_available_models();
        let current = agent.lock().await.current_model().to_string();
        println!("Available models (choose number, model name, or blank to cancel):");
        for (idx, model) in available.iter().enumerate() {
            if model == &current {
                println!("{}. {} (current)", idx + 1, model);
            } else {
                println!("{}. {}", idx + 1, model);
            }
        }
        print!("model> ");
        io::stdout().flush()?;
        let mut selected = String::new();
        io::stdin().read_line(&mut selected)?;
        let selected = selected.trim();
        if selected.is_empty() {
            println!("Model selection cancelled.");
            return Ok(());
        }
        let candidate = if let Ok(index) = selected.parse::<usize>() {
            if index == 0 || index > available.len() {
                println!("Invalid model selection index: {selected}");
                return Ok(());
            }
            available[index - 1].clone()
        } else {
            selected.to_string()
        };

        if available.iter().any(|m| m == &candidate) {
            agent.lock().await.set_model(candidate.clone());
            settings.lock().await.update_project_model(&candidate)?;
            println!("Switched to model: {candidate}");
        } else {
            println!("Invalid model: {candidate}");
            println!("Available: {}", available.join(", "));
        }
        return Ok(());
    }

    if let Some(model) = input.strip_prefix("/models ").map(str::trim) {
        let available = settings.lock().await.get_available_models();
        if available.iter().any(|m| m == model) {
            agent.lock().await.set_model(model.to_string());
            settings.lock().await.update_project_model(model)?;
            println!("Switched to model: {model}");
        } else {
            println!("Invalid model: {model}");
            println!("Available: {}", available.join(", "));
        }
        return Ok(());
    }

    if input == "/commit-and-push" {
        run_commit_and_push(agent).await?;
        return Ok(());
    }

    if input.starts_with('/') {
        println!("Unknown slash command: {input}");
        println!("Use /help to see available commands.");
        return Ok(());
    }

    if is_direct_command(input) {
        let result = execute_bash_command(input).await?;
        print_tool_result(
            ToolCallSummary {
                id: "bash_inline".to_string(),
                name: "bash".to_string(),
                arguments: format!(r#"{{"command":"{}"}}"#, input.replace('"', "\\\"")),
            },
            result,
        );
        return Ok(());
    }

    stream_agent_message(input.to_string(), agent).await
}

async fn stream_agent_message(message: String, agent: Arc<Mutex<Agent>>) -> Result<()> {
    let cancel_token = CancellationToken::new();
    let (agent_tx, mut agent_rx) = mpsc::unbounded_channel::<AgentEvent>();

    let agent_for_task = agent.clone();
    tokio::spawn(async move {
        let result = agent_for_task
            .lock()
            .await
            .process_user_message_stream(message, cancel_token, agent_tx)
            .await;
        if let Err(err) = result {
            eprintln!("Error: {err:#}");
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

    loop {
        let event = tokio::select! {
            _ = status_tick.tick(), if !started_content => {
                let elapsed = started_at.elapsed().as_secs();
                let status = format!(
                    "{} {}... {}s",
                    STATUS_FRAMES[frame_idx % STATUS_FRAMES.len()],
                    phase,
                    elapsed
                );
                frame_idx = frame_idx.wrapping_add(1);
                render_status_line(&status, &mut status_width)?;
                continue;
            }
            maybe_event = agent_rx.recv() => maybe_event,
        };

        let Some(event) = event else {
            break;
        };

        match event {
            AgentEvent::Content(chunk) => {
                if !started_content {
                    clear_status_line(&mut status_width)?;
                    print!("{} ", "●".white());
                    started_content = true;
                }
                print!("{chunk}");
                io::stdout().flush()?;
            }
            AgentEvent::ToolCalls(calls) => {
                if started_content {
                    println!();
                    started_content = false;
                }
                phase = "running tools";
                tool_calls_seen += calls.len();
                clear_status_line(&mut status_width)?;
                for call in calls {
                    println!(
                        "{} {}",
                        "●".magenta(),
                        format!("{}({})", pretty_tool_name(&call.name), tool_target(&call)).white()
                    );
                    println!("{}", "  -> Executing...".cyan());
                }
            }
            AgentEvent::ToolResult { tool_call, result } => {
                if started_content {
                    println!();
                    started_content = false;
                }
                tool_results_seen = tool_results_seen.saturating_add(1);
                phase = if tool_calls_seen == 0 {
                    "running tools"
                } else if tool_results_seen >= tool_calls_seen {
                    "finalizing"
                } else {
                    "running tools"
                };
                clear_status_line(&mut status_width)?;
                print_tool_result(tool_call, result);
            }
            AgentEvent::Done => {
                clear_status_line(&mut status_width)?;
                if started_content {
                    println!();
                }
                let elapsed = started_at.elapsed();
                println!(
                    "{}",
                    format!(
                        "● completed in {}.{:01}s",
                        elapsed.as_secs(),
                        elapsed.subsec_millis() / 100
                    )
                    .dark_grey()
                );
                break;
            }
        }
    }

    Ok(())
}

async fn run_commit_and_push(agent: Arc<Mutex<Agent>>) -> Result<()> {
    println!("Running commit-and-push...");

    let status = execute_bash_command("git status --porcelain").await?;
    if !status.success
        || status
            .output
            .as_deref()
            .unwrap_or_default()
            .trim()
            .is_empty()
    {
        println!("No changes to commit.");
        return Ok(());
    }

    let add = execute_bash_command("git add .").await?;
    print_tool_result(
        ToolCallSummary {
            id: "git_add_inline".to_string(),
            name: "bash".to_string(),
            arguments: r#"{"command":"git add ."}"#.to_string(),
        },
        add,
    );

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

    let message = match agent.lock().await.generate_plain_text(&prompt).await {
        Ok(text) if !text.trim().is_empty() => text.trim().trim_matches('"').to_string(),
        _ => "chore: update project files".to_string(),
    };
    println!("Generated commit message: \"{message}\"");

    let commit_cmd = format!("git commit -m \"{}\"", message.replace('"', "\\\""));
    let commit = execute_bash_command(&commit_cmd).await?;
    let commit_success = commit.success;
    print_tool_result(
        ToolCallSummary {
            id: "git_commit_inline".to_string(),
            name: "bash".to_string(),
            arguments: format!(r#"{{"command":"{}"}}"#, commit_cmd.replace('"', "\\\"")),
        },
        commit,
    );

    if commit_success {
        let mut push_cmd = "git push".to_string();
        let mut push = execute_bash_command(&push_cmd).await?;
        if !push.success
            && push
                .error
                .as_deref()
                .map(|e| e.contains("no upstream branch"))
                .unwrap_or(false)
        {
            push_cmd = "git push -u origin HEAD".to_string();
            push = execute_bash_command(&push_cmd).await?;
        }

        print_tool_result(
            ToolCallSummary {
                id: "git_push_inline".to_string(),
                name: "bash".to_string(),
                arguments: format!(r#"{{"command":"{}"}}"#, push_cmd.replace('"', "\\\"")),
            },
            push,
        );
    }

    Ok(())
}

fn print_logo_and_tips() {
    let logo = [
        "  /\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\",
        " /\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/",
        " \\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\",
        "  /\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\/\\",
    ];
    for line in logo {
        println!("{line}");
    }
    println!();
    println!("Tips for getting started:");
    println!("1. Ask questions, edit files, or run commands.");
    println!("2. Use /help for slash commands.");
    println!("3. Scrollback is native in inline mode (no alternate screen).");
    println!();
}

fn print_tool_result(call: ToolCallSummary, result: ToolResult) {
    println!(
        "{} {}",
        "●".magenta(),
        format!("{}({})", pretty_tool_name(&call.name), tool_target(&call)).white()
    );
    if result.success {
        if let Some(output) = result.output {
            for line in output.replace("\r\n", "\n").split('\n') {
                println!("{}", format!("  -> {line}").dark_grey());
            }
        } else {
            println!("{}", "  -> Success".dark_grey());
        }
    } else if let Some(error) = result.error {
        for line in error.replace("\r\n", "\n").split('\n') {
            println!("{}", format!("  -> {line}").red());
        }
    } else {
        println!("{}", "  -> Error".red());
    }
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

fn tool_target(tool: &ToolCallSummary) -> String {
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

fn is_direct_command(input: &str) -> bool {
    let first = input.split_whitespace().next().unwrap_or_default();
    DIRECT_COMMANDS.contains(&first)
}

fn help_text() -> &'static str {
    "Grok Build Help:\n\n/clear\n/help\n/models\n/models <name>\n/commit-and-push\n/exit\n\nInline mode keeps native terminal scrollback, shows live elapsed status while working, and preserves output after Ctrl+C."
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
