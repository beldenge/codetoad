mod agent;
mod cli;
mod confirmation;
mod custom_instructions;
mod git_ops;
mod grok_client;
mod inline_feedback;
mod inline_markdown;
mod inline_prompt;
mod inline_ui;
mod protocol;
mod responses_adapter;
mod settings;
mod slash_commands;
mod tool_catalog;
mod tool_context;
mod tools;

use crate::agent::Agent;
use crate::cli::{Cli, Commands, GitCommands};
use crate::git_ops::{
    CommitAndPushEvent, CommitAndPushOptions, CommitAndPushOutcome, CommitAndPushStep,
    run_commit_and_push,
};
use crate::settings::SettingsManager;
use anyhow::{Context, Result, bail};
use clap::Parser;
use crossterm::event::DisableMouseCapture;
use crossterm::execute;
use crossterm::terminal::disable_raw_mode;
use std::io;
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
    tool_context::initialize(cwd.clone())?;

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

    inline_ui::run_inline(agent, settings, initial_message).await
}

fn install_ctrlc_handler() -> Result<()> {
    ctrlc::set_handler(|| {
        let _ = disable_raw_mode();
        let mut stdout = io::stdout();
        let _ = execute!(stdout, DisableMouseCapture);
        println!();
        std::process::exit(0);
    })
    .context("Failed to install Ctrl+C handler")
}

async fn headless_commit_and_push(agent: Arc<Mutex<Agent>>) -> Result<()> {
    println!("Processing commit-and-push...");
    let outcome = run_commit_and_push(
        agent,
        CommitAndPushOptions::default(),
        |event| match event {
            CommitAndPushEvent::NoChanges => {}
            CommitAndPushEvent::ChangesDetected => {
                println!("git status: changes detected");
            }
            CommitAndPushEvent::GeneratedMessage(message) => {
                println!("generated commit message: \"{message}\"");
            }
            CommitAndPushEvent::ToolResult { step, result, .. } => {
                if !result.success {
                    return;
                }

                match step {
                    CommitAndPushStep::Add => {
                        println!("git add: staged");
                    }
                    CommitAndPushStep::Commit => {
                        println!("git commit: {}", first_line(result.output.unwrap_or_default()));
                    }
                    CommitAndPushStep::Push => {
                        println!("git push: {}", first_line(result.output.unwrap_or_default()));
                    }
                }
            }
        },
    )
    .await?;

    if matches!(outcome, CommitAndPushOutcome::NoChanges) {
        bail!("No changes to commit. Working directory is clean.");
    }

    Ok(())
}

fn first_line(text: String) -> String {
    text.lines().next().unwrap_or_default().to_string()
}
