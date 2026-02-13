use anyhow::{Context, Result, bail};
use clap::Parser;
use crossterm::event::DisableMouseCapture;
use crossterm::execute;
use crossterm::terminal::disable_raw_mode;
use grok_build::agent::Agent;
use grok_build::app_context::AppContext;
use grok_build::cli::{ApiKeyStorageArg, Cli, Commands, GitCommands};
use grok_build::git_ops::{
    CommitAndPushEvent, CommitAndPushOptions, CommitAndPushOutcome, CommitAndPushStep,
    run_commit_and_push,
};
use grok_build::image_input::prepare_user_input;
use grok_build::inline_ui;
use grok_build::settings::{ApiKeySaveLocation, ApiKeyStorageMode, SettingsManager};
use std::io;

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
    if let Some(storage_arg) = cli.api_key_storage {
        let mode = match storage_arg {
            ApiKeyStorageArg::Keychain => ApiKeyStorageMode::Keychain,
            ApiKeyStorageArg::Plaintext => ApiKeyStorageMode::Plaintext,
        };
        settings.update_api_key_storage_mode(mode)?;
        println!("Set API key storage mode: {}", mode.as_str());
    }

    if let Some(api_key) = &cli.api_key {
        match settings.update_user_api_key(api_key)? {
            ApiKeySaveLocation::Keychain => {
                println!("Saved API key to secure OS keychain.");
            }
            ApiKeySaveLocation::PlaintextFallback => {
                println!(
                    "Saved API key to ~/.grok/user-settings.json (keychain unavailable; fallback active)."
                );
            }
            ApiKeySaveLocation::Plaintext => {
                println!("Saved API key to ~/.grok/user-settings.json (plaintext mode).");
            }
        }
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
                "API key required. Set GROK_API_KEY/XAI_API_KEY/OPENAI_API_KEY, use --api-key, or configure settings/keychain."
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

    let app = AppContext::new(
        cwd.clone(),
        Agent::new(api_key, base_url, model, cli.max_tool_rounds, &cwd)?,
        settings,
    );

    if let Some(Commands::Git { command }) = cli.command {
        match command {
            GitCommands::CommitAndPush => {
                headless_commit_and_push(app.clone()).await?;
                return Ok(());
            }
        }
    }

    if let Some(prompt) = cli.prompt {
        let prepared = prepare_user_input(&prompt, &cwd);
        for warning in &prepared.warnings {
            eprintln!("warning: {warning}");
        }
        let (message, attachments) = prepared.into_chat_request();
        let agent = app.agent();
        let mut guard = agent.lock().await;
        let output = guard
            .process_user_message_with_attachments(&message, attachments)
            .await?;
        println!("{output}");
        return Ok(());
    }

    let initial_message = if cli.message.is_empty() {
        None
    } else {
        Some(cli.message.join(" "))
    };

    inline_ui::run_inline(app, initial_message).await
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

async fn headless_commit_and_push(app: AppContext) -> Result<()> {
    println!("Processing commit-and-push...");
    let outcome =
        run_commit_and_push(
            app.agent(),
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
                            println!(
                                "git commit: {}",
                                first_line(result.output.unwrap_or_default())
                            );
                        }
                        CommitAndPushStep::Push => {
                            println!(
                                "git push: {}",
                                first_line(result.output.unwrap_or_default())
                            );
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
