use anyhow::Result;
use codetoad::settings::{ApiKeyStorageMode, SettingsManager};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn provider_keys_persist_across_reloads_in_plaintext_mode() -> Result<()> {
    let temp = TempDir::new("settings-persist");
    let home = temp.path().join("home");
    let cwd = temp.path().join("project");
    std::fs::create_dir_all(&home)?;
    std::fs::create_dir_all(&cwd)?;

    let mut settings = SettingsManager::load_with_home(&cwd, &home)?;
    settings.update_api_key_storage_mode(ApiKeyStorageMode::Plaintext)?;

    settings.add_or_update_provider(
        "xai",
        "https://api.x.ai/v1",
        Some("grok-code-fast-1".to_string()),
        None,
    )?;
    settings.switch_active_provider("xai")?;
    settings.update_user_api_key("xai-test-key")?;

    settings.add_or_update_provider(
        "openai",
        "https://api.openai.com/v1",
        Some("gpt-4.1".to_string()),
        None,
    )?;
    settings.switch_active_provider("openai")?;
    settings.update_user_api_key("openai-test-key")?;
    settings.switch_active_provider("xai")?;
    assert_eq!(settings.get_api_key().as_deref(), Some("xai-test-key"));

    drop(settings);

    let mut loaded = SettingsManager::load_with_home(&cwd, &home)?;
    assert_eq!(loaded.active_provider_id(), "xai");
    assert_eq!(loaded.get_api_key().as_deref(), Some("xai-test-key"));

    loaded.switch_active_provider("openai")?;
    assert_eq!(loaded.get_api_key().as_deref(), Some("openai-test-key"));
    Ok(())
}

#[test]
fn active_provider_default_model_persists_to_project_settings() -> Result<()> {
    let temp = TempDir::new("settings-model");
    let home = temp.path().join("home");
    let cwd = temp.path().join("project");
    std::fs::create_dir_all(&home)?;
    std::fs::create_dir_all(&cwd)?;

    let mut settings = SettingsManager::load_with_home(&cwd, &home)?;
    settings.update_api_key_storage_mode(ApiKeyStorageMode::Plaintext)?;
    settings.add_or_update_provider(
        "openai",
        "https://api.openai.com/v1",
        Some("gpt-4.1".to_string()),
        None,
    )?;
    settings.switch_active_provider("openai")?;

    drop(settings);

    let project_settings_path = cwd.join(".grok").join("settings.json");
    let raw = std::fs::read_to_string(project_settings_path)?;
    let parsed: serde_json::Value = serde_json::from_str(&raw)?;
    assert_eq!(
        parsed.get("model").and_then(|value| value.as_str()),
        Some("gpt-4.1")
    );
    Ok(())
}

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(prefix: &str) -> Self {
        let mut path = std::env::temp_dir();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        path.push(format!(
            "codetoad-{}-{}-{}",
            prefix,
            std::process::id(),
            nanos
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
