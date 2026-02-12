mod agent;
mod cli;
mod custom_instructions;
mod grok_client;
mod inline_ui;
mod protocol;
mod settings;
mod tools;
mod tui;

use crate::agent::Agent;
use crate::cli::{Cli, Commands, GitCommands, UiMode};
use crate::settings::SettingsManager;
use crate::tools::execute_bash_command;
use anyhow::{Context, Result, bail};
use clap::Parser;
use crossterm::event::DisableMouseCapture;
use crossterm::execute;
use crossterm::terminal::{LeaveAlternateScreen, disable_raw_mode};
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

#[tokio::main]
async fn main() -> Result<()> {
    install_ctrlc_handler()?;
    let cli = Cli::parse();

    if let Some(directory) = cli.directory.as_ref() {
        std::env::set_current_dir(directory)
            .with_context(|| format!("Failed to change directory to {}", directory.display()))?;
    }
    let cwd = std::env::current_dir().context("Failed to determine current directory")?;

    let mut settings = SettingsManager::load(&cwd)?;
    if let Some(api_key) = &cli.api_key {
        settings.update_user_api_key(api_key)?;
        println!("Saved API key to ~/.grok/user-settings.json");
    }
    if let Some(base_url) = &cli.base_url {
        settings.update_user_base_url(base_url)?;
        println!("Saved base URL to ~/.grok/user-settings.json");
    }

    let api_key = cli
        .api_key
        .clone()
        .or_else(|| settings.get_api_key())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "API key required. Set GROK_API_KEY, use --api-key, or set ~/.grok/user-settings.json"
            )
        })?;
    let base_url = cli
        .base_url
        .clone()
        .unwrap_or_else(|| settings.get_base_url());
    let model = cli
        .model
        .clone()
        .or_else(|| std::env::var("GROK_MODEL").ok())
        .unwrap_or_else(|| settings.get_current_model());

    let settings = Arc::new(Mutex::new(settings));
    let agent = Arc::new(Mutex::new(Agent::new(
        api_key,
        base_url,
        model,
        cli.max_tool_rounds,
        &cwd,
    )?));

    if let Some(Commands::Git { command }) = cli.command {
        match command {
            GitCommands::CommitAndPush => {
                headless_commit_and_push(agent).await?;
                return Ok(());
            }
        }
    }

    if let Some(prompt) = cli.prompt {
        let mut guard = agent.lock().await;
        let output = guard.process_user_message(&prompt).await?;
        println!("{output}");
        return Ok(());
    }

    let initial_message = if cli.message.is_empty() {
        None
    } else {
        Some(cli.message.join(" "))
    };

    if cli.ui == UiMode::Tui {
        tui::run_interactive(agent, settings, initial_message).await
    } else {
        inline_ui::run_inline(agent, settings, initial_message).await
    }
}

fn install_ctrlc_handler() -> Result<()> {
    ctrlc::set_handler(|| {
        let _ = disable_raw_mode();
        let mut stdout = io::stdout();
        let _ = execute!(stdout, LeaveAlternateScreen, DisableMouseCapture);
        std::process::exit(0);
    })
    .context("Failed to install Ctrl+C handler")
}

async fn headless_commit_and_push(agent: Arc<Mutex<Agent>>) -> Result<()> {
    println!("Processing commit-and-push...");

    let status = execute_bash_command("git status --porcelain").await?;
    if !status.success
        || status
            .output
            .as_deref()
            .unwrap_or_default()
            .trim()
            .is_empty()
    {
        bail!("No changes to commit. Working directory is clean.");
    }
    println!("git status: changes detected");

    let add = execute_bash_command("git add .").await?;
    if !add.success {
        bail!("git add failed: {}", add.error.unwrap_or_default());
    }
    println!("git add: staged");

    let diff = execute_bash_command("git diff --cached")
        .await
        .ok()
        .and_then(|r| r.output)
        .unwrap_or_default();

    let prompt = format!(
        "Generate a concise professional git commit message for these changes.\nUse conventional commit prefixes and keep under 72 chars.\nReturn only the message.\n\nGit Status:\n{}\n\nGit Diff:\n{}",
        status.output.unwrap_or_default(),
        diff
    );
    let commit_message = {
        let guard = agent.lock().await;
        guard.generate_plain_text(&prompt).await?
    };
    let clean_message = commit_message.trim().trim_matches('"').to_string();
    if clean_message.is_empty() {
        bail!("Failed to generate commit message");
    }
    println!("generated commit message: \"{clean_message}\"");

    let commit_cmd = format!("git commit -m \"{}\"", clean_message.replace('"', "\\\""));
    let commit = execute_bash_command(&commit_cmd).await?;
    if !commit.success {
        bail!("git commit failed: {}", commit.error.unwrap_or_default());
    }
    println!(
        "git commit: {}",
        first_line(commit.output.unwrap_or_default())
    );

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

    if !push.success {
        bail!("git push failed: {}", push.error.unwrap_or_default());
    }
    println!("git push: {}", first_line(push.output.unwrap_or_default()));
    Ok(())
}

fn first_line(text: String) -> String {
    text.lines().next().unwrap_or_default().to_string()
}

#[allow(dead_code)]
fn _cwd_from_arg(directory: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(directory) = directory {
        Ok(directory)
    } else {
        std::env::current_dir().context("Failed to determine current directory")
    }
}
