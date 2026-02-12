use crate::agent::{Agent, AgentEvent, ToolCallSummary};
use crate::settings::SettingsManager;
use crate::tools::{ToolResult, execute_bash_command};
use anyhow::Result;
use crossterm::event::DisableMouseCapture;
use crossterm::execute;
use crossterm::style::Stylize;
use crossterm::terminal::{LeaveAlternateScreen, disable_raw_mode};
use std::io::{self, Write};
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;

const DIRECT_COMMANDS: &[&str] = &[
    "ls", "pwd", "cd", "cat", "mkdir", "touch", "echo", "grep", "find", "cp", "mv", "rm",
];

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
    let _ = execute!(stdout, LeaveAlternateScreen, DisableMouseCapture);
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
        println!("Conversation cleared.");
        return Ok(());
    }

    if input == "/models" {
        let available = settings.lock().await.get_available_models();
        let current = agent.lock().await.current_model().to_string();
        println!("Available models:");
        for model in available {
            if model == current {
                println!("* {model} (current)");
            } else {
                println!("  {model}");
            }
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
    while let Some(event) = agent_rx.recv().await {
        match event {
            AgentEvent::Content(chunk) => {
                if !started_content {
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
                print_tool_result(tool_call, result);
            }
            AgentEvent::Done => {
                if started_content {
                    println!();
                }
                break;
            }
            AgentEvent::Error(err) => {
                if started_content {
                    println!();
                }
                println!("Error: {err}");
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
    "Grok Build Help:\n\n/clear\n/help\n/models\n/models <name>\n/commit-and-push\n/exit\n\nInline mode keeps native terminal scrollback and preserves output after Ctrl+C."
}
