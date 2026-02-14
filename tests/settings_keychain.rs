#![cfg(feature = "keychain-integration-tests")]

use anyhow::{Result, bail};
use grok_build::settings::{ApiKeySaveLocation, ApiKeyStorageMode, SettingsManager};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn keychain_roundtrip_persists_for_active_provider_when_backend_is_available() -> Result<()> {
    if has_api_key_env_override() {
        eprintln!("Skipping keychain integration test: API key env vars are set.");
        return Ok(());
    }

    let temp = TempDir::new("settings-keychain");
    let home = temp.path().join("home");
    let cwd = temp.path().join("project");
    std::fs::create_dir_all(&home)?;
    std::fs::create_dir_all(&cwd)?;

    let mut settings = SettingsManager::load_with_home(&cwd, &home)?;
    settings.update_api_key_storage_mode(ApiKeyStorageMode::Keychain)?;

    let provider_id = format!("keychain-test-{}", unique_nonce());
    settings.add_or_update_provider(
        &provider_id,
        "https://api.x.ai/v1",
        Some("grok-code-fast-1".to_string()),
        None,
    )?;
    settings.switch_active_provider(&provider_id)?;

    let expected_key = format!("test-key-{}", unique_nonce());
    let save_result = settings.update_user_api_key(&expected_key)?;
    if save_result != ApiKeySaveLocation::Keychain {
        eprintln!("Skipping keychain integration test: keychain backend unavailable.");
        return Ok(());
    }

    drop(settings);

    let mut loaded = SettingsManager::load_with_home(&cwd, &home)?;
    loaded.switch_active_provider(&provider_id)?;
    let Some(reloaded_key) = loaded.get_api_key() else {
        bail!("Expected API key to persist in keychain for provider {provider_id}");
    };
    assert_eq!(reloaded_key, expected_key);
    Ok(())
}

fn has_api_key_env_override() -> bool {
    ["GROK_API_KEY", "XAI_API_KEY", "OPENAI_API_KEY"]
        .iter()
        .any(|name| {
            std::env::var(name)
                .ok()
                .is_some_and(|value| !value.trim().is_empty())
        })
}

fn unique_nonce() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos()
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(prefix: &str) -> Self {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "grok-build-{}-{}-{}",
            prefix,
            std::process::id(),
            unique_nonce()
        ));
        std::fs::create_dir_all(&path).expect("create temp dir");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
