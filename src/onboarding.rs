use crate::provider::{XAI_DEFAULT_BASE_URL, default_model_for, detect_provider};
use crate::settings::{ApiKeySaveLocation, SettingsManager};
use anyhow::{Context, Result, bail};
use std::io::{self, Write};

pub fn run_first_time_setup(settings: &mut SettingsManager) -> Result<()> {
    println!("No API key is configured for the active provider.");
    println!("Starting first-run setup...");
    let provider_id = run_add_or_update_provider(settings, true)?;
    println!("Setup complete. Active provider: {provider_id}");
    Ok(())
}

pub fn run_add_or_update_provider(
    settings: &mut SettingsManager,
    activate_after_add: bool,
) -> Result<String> {
    println!("Add provider profile:");
    let provider_type = prompt_provider_type()?;
    let default_id = match provider_type {
        ProviderType::Xai => "xai",
        ProviderType::OpenAiCompatible => "openai",
    };
    let provider_id = if activate_after_add {
        default_id.to_string()
    } else {
        prompt_with_default("Provider id", default_id)?
    };

    let default_base_url = match provider_type {
        ProviderType::Xai => XAI_DEFAULT_BASE_URL.to_string(),
        ProviderType::OpenAiCompatible => "https://api.openai.com/v1".to_string(),
    };
    let base_url = prompt_with_default("Base URL", &default_base_url)?;

    let model_default = default_model_for(detect_provider(&base_url));
    let model = prompt_with_default("Default model", model_default)?;

    let provider_id =
        settings.add_or_update_provider(&provider_id, &base_url, Some(model), None)?;
    if activate_after_add {
        settings.switch_active_provider(&provider_id)?;
    }

    if activate_after_add {
        ensure_active_provider_api_key(settings)?;
    } else {
        println!("Provider profile saved. Switch to it first (`/providers`) before storing a key.");
    }

    Ok(provider_id)
}

pub fn ensure_active_provider_api_key(settings: &mut SettingsManager) -> Result<()> {
    if settings.get_api_key().is_some() {
        return Ok(());
    }

    let api_key = loop {
        let value = rpassword::prompt_password("API key: ")
            .context("Failed to read API key from terminal")?;
        if !value.trim().is_empty() {
            break value;
        }
        println!("API key cannot be empty.");
    };

    match settings.update_user_api_key(&api_key)? {
        ApiKeySaveLocation::Keychain => {
            println!("Saved API key to secure OS keychain.");
        }
        ApiKeySaveLocation::SessionOnly => {
            println!(
                "Could not persist API key to keychain. Using it only for this run; you will be prompted again next launch."
            );
        }
        ApiKeySaveLocation::Plaintext => {
            println!("Saved API key to ~/.grok/user-settings.json (plaintext mode).");
        }
    }

    Ok(())
}

fn prompt_provider_type() -> Result<ProviderType> {
    println!("Select provider type:");
    println!("  1. xAI");
    println!("  2. OpenAI-compatible");
    loop {
        let selection = prompt_with_default("Choice", "1")?;
        match selection.trim() {
            "1" => return Ok(ProviderType::Xai),
            "2" => return Ok(ProviderType::OpenAiCompatible),
            _ => println!("Enter 1 or 2."),
        }
    }
}

fn prompt_with_default(prompt: &str, default: &str) -> Result<String> {
    print!("{prompt} [{default}]: ");
    io::stdout().flush().context("Failed flushing stdout")?;
    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .context("Failed reading input")?;
    let value = input.trim();
    if value.is_empty() {
        if default.trim().is_empty() {
            bail!("{prompt} cannot be empty");
        }
        Ok(default.to_string())
    } else {
        Ok(value.to_string())
    }
}

#[derive(Clone, Copy)]
enum ProviderType {
    Xai,
    OpenAiCompatible,
}
