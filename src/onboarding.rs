use crate::provider::{XAI_DEFAULT_BASE_URL, default_model_for, detect_provider};
use crate::settings::{ApiKeySaveLocation, SettingsManager};
use anyhow::{Context, Result, bail};
use std::io::{self, BufRead, Write};

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
        if let Some(provider) = parse_provider_choice(&selection) {
            return Ok(provider);
        }
        println!("Enter 1 or 2.");
    }
}

fn prompt_with_default(prompt: &str, default: &str) -> Result<String> {
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut output = io::stdout();
    prompt_with_default_io(prompt, default, &mut input, &mut output)
}

fn prompt_with_default_io<R: BufRead, W: Write>(
    prompt: &str,
    default: &str,
    input: &mut R,
    output: &mut W,
) -> Result<String> {
    write!(output, "{prompt} [{default}]: ").context("Failed writing prompt")?;
    output.flush().context("Failed flushing stdout")?;

    let mut raw = String::new();
    input.read_line(&mut raw).context("Failed reading input")?;
    let value = raw.trim();
    if value.is_empty() {
        if default.trim().is_empty() {
            bail!("{prompt} cannot be empty");
        }
        Ok(default.to_string())
    } else {
        Ok(value.to_string())
    }
}

fn parse_provider_choice(selection: &str) -> Option<ProviderType> {
    match selection.trim() {
        "1" => Some(ProviderType::Xai),
        "2" => Some(ProviderType::OpenAiCompatible),
        _ => None,
    }
}

#[derive(Clone, Copy)]
enum ProviderType {
    Xai,
    OpenAiCompatible,
}

#[cfg(test)]
mod tests {
    use super::{ProviderType, parse_provider_choice, prompt_with_default_io};
    use std::io::Cursor;

    #[test]
    fn parse_provider_choice_accepts_known_options() {
        assert!(matches!(
            parse_provider_choice("1"),
            Some(ProviderType::Xai)
        ));
        assert!(matches!(
            parse_provider_choice(" 2 "),
            Some(ProviderType::OpenAiCompatible)
        ));
        assert!(parse_provider_choice("x").is_none());
    }

    #[test]
    fn prompt_with_default_uses_default_for_blank_input() {
        let mut input = Cursor::new("\n");
        let mut output = Vec::<u8>::new();
        let value =
            prompt_with_default_io("Base URL", "https://api.x.ai/v1", &mut input, &mut output)
                .expect("prompt value");
        assert_eq!(value, "https://api.x.ai/v1");
        let rendered = String::from_utf8(output).expect("utf8 output");
        assert!(rendered.contains("Base URL [https://api.x.ai/v1]: "));
    }

    #[test]
    fn prompt_with_default_returns_trimmed_input_when_provided() {
        let mut input = Cursor::new("  custom-value  \n");
        let mut output = Vec::<u8>::new();
        let value =
            prompt_with_default_io("Provider id", "xai", &mut input, &mut output).expect("value");
        assert_eq!(value, "custom-value");
    }

    #[test]
    fn prompt_with_default_errors_when_both_input_and_default_are_empty() {
        let mut input = Cursor::new("\n");
        let mut output = Vec::<u8>::new();
        let err = prompt_with_default_io("Provider id", "", &mut input, &mut output)
            .expect_err("expected empty default error");
        assert!(err.to_string().contains("cannot be empty"));
    }
}
