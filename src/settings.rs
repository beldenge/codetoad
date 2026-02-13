use anyhow::{Context, Result};
use dirs::home_dir;
use keyring::Entry;
use keyring::Error as KeyringError;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const SETTINGS_VERSION: u32 = 1;
const DEFAULT_BASE_URL: &str = "https://api.x.ai/v1";
const DEFAULT_MODEL: &str = "grok-code-fast-1";
const KEYRING_SERVICE: &str = "grok-build";
const KEYRING_ACCOUNT: &str = "xai_api_key";

fn default_models() -> Vec<String> {
    vec![
        "grok-4-1-fast-reasoning".to_string(),
        "grok-4-1-fast-non-reasoning".to_string(),
        "grok-4-fast-reasoning".to_string(),
        "grok-4-fast-non-reasoning".to_string(),
        "grok-4".to_string(),
        "grok-4-latest".to_string(),
        "grok-code-fast-1".to_string(),
        "grok-3".to_string(),
        "grok-3-latest".to_string(),
        "grok-3-fast".to_string(),
        "grok-3-mini".to_string(),
        "grok-3-mini-fast".to_string(),
    ]
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct UserSettings {
    #[serde(rename = "apiKey", alias = "api_key")]
    pub api_key: Option<String>,
    #[serde(rename = "baseURL", alias = "base_url")]
    pub base_url: Option<String>,
    #[serde(rename = "defaultModel", alias = "default_model")]
    pub default_model: Option<String>,
    pub models: Option<Vec<String>>,
    #[serde(rename = "apiKeyStorage", alias = "api_key_storage")]
    pub api_key_storage: Option<ApiKeyStorageMode>,
    #[serde(rename = "settingsVersion", alias = "settings_version")]
    pub settings_version: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ProjectSettings {
    pub model: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SettingsManager {
    user_settings_path: PathBuf,
    project_settings_path: PathBuf,
    user_settings: UserSettings,
    project_settings: ProjectSettings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApiKeyStorageMode {
    Keychain,
    Plaintext,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiKeySaveLocation {
    Keychain,
    PlaintextFallback,
    Plaintext,
}

impl ApiKeyStorageMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Keychain => "keychain",
            Self::Plaintext => "plaintext",
        }
    }
}

impl SettingsManager {
    pub fn load(cwd: &Path) -> Result<Self> {
        let home = home_dir().context("Failed to determine home directory")?;
        let user_settings_path = home.join(".grok").join("user-settings.json");
        let project_settings_path = cwd.join(".grok").join("settings.json");

        let mut user_settings = load_json_or_default::<UserSettings>(&user_settings_path)?;
        let project_settings = load_json_or_default::<ProjectSettings>(&project_settings_path)?;

        if user_settings.settings_version.unwrap_or(0) < SETTINGS_VERSION {
            migrate_user_settings(&mut user_settings);
            ensure_parent_dir(&user_settings_path)?;
            write_json(&user_settings_path, &user_settings)?;
        }

        let mut manager = Self {
            user_settings_path,
            project_settings_path,
            user_settings,
            project_settings,
        };

        manager.ensure_default_files()?;
        manager.maybe_migrate_plaintext_api_key_to_keychain()?;
        Ok(manager)
    }

    fn ensure_default_files(&mut self) -> Result<()> {
        if !self.user_settings_path.exists() {
            self.user_settings.settings_version = Some(SETTINGS_VERSION);
            if self.user_settings.base_url.is_none() {
                self.user_settings.base_url = Some(DEFAULT_BASE_URL.to_string());
            }
            if self.user_settings.default_model.is_none() {
                self.user_settings.default_model = Some(DEFAULT_MODEL.to_string());
            }
            if self.user_settings.models.is_none() {
                self.user_settings.models = Some(default_models());
            }
            if self.user_settings.api_key_storage.is_none() {
                self.user_settings.api_key_storage = Some(ApiKeyStorageMode::Keychain);
            }
            self.save_user()?;
        }

        if !self.project_settings_path.exists() {
            if self.project_settings.model.is_none() {
                self.project_settings.model = Some(DEFAULT_MODEL.to_string());
            }
            self.save_project()?;
        }

        Ok(())
    }

    pub fn save_user(&self) -> Result<()> {
        ensure_parent_dir(&self.user_settings_path)?;
        write_json(&self.user_settings_path, &self.user_settings)
    }

    pub fn save_project(&self) -> Result<()> {
        ensure_parent_dir(&self.project_settings_path)?;
        write_json(&self.project_settings_path, &self.project_settings)
    }

    pub fn get_api_key(&self) -> Option<String> {
        if let Ok(from_env) = std::env::var("GROK_API_KEY")
            && !from_env.trim().is_empty()
        {
            return Some(from_env);
        }

        if self.get_api_key_storage_mode() == ApiKeyStorageMode::Keychain
            && let Ok(Some(from_keychain)) = load_api_key_from_keychain()
            && !from_keychain.trim().is_empty()
        {
            return Some(from_keychain);
        }

        self.user_settings.api_key.clone()
    }

    pub fn get_base_url(&self) -> String {
        std::env::var("GROK_BASE_URL")
            .ok()
            .or_else(|| self.user_settings.base_url.clone())
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
    }

    pub fn get_current_model(&self) -> String {
        self.project_settings
            .model
            .clone()
            .or_else(|| std::env::var("GROK_MODEL").ok())
            .or_else(|| self.user_settings.default_model.clone())
            .unwrap_or_else(|| DEFAULT_MODEL.to_string())
    }

    pub fn get_available_models(&self) -> Vec<String> {
        self.user_settings
            .models
            .clone()
            .unwrap_or_else(default_models)
            .into_iter()
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty())
            .collect()
    }

    pub fn update_project_model(&mut self, model: &str) -> Result<()> {
        self.project_settings.model = Some(model.to_string());
        self.save_project()
    }

    pub fn get_api_key_storage_mode(&self) -> ApiKeyStorageMode {
        self.user_settings
            .api_key_storage
            .unwrap_or(ApiKeyStorageMode::Keychain)
    }

    pub fn update_api_key_storage_mode(&mut self, mode: ApiKeyStorageMode) -> Result<()> {
        self.user_settings.api_key_storage = Some(mode);
        match mode {
            ApiKeyStorageMode::Keychain => {
                self.maybe_migrate_plaintext_api_key_to_keychain()?;
                self.save_user()
            }
            ApiKeyStorageMode::Plaintext => {
                if self.user_settings.api_key.is_none()
                    && let Ok(Some(key)) = load_api_key_from_keychain()
                {
                    self.user_settings.api_key = Some(key);
                }
                self.save_user()
            }
        }
    }

    pub fn update_user_api_key(&mut self, api_key: &str) -> Result<ApiKeySaveLocation> {
        match self.get_api_key_storage_mode() {
            ApiKeyStorageMode::Keychain => {
                if store_api_key_in_keychain(api_key).is_ok() {
                    self.user_settings.api_key = None;
                    self.save_user()?;
                    Ok(ApiKeySaveLocation::Keychain)
                } else {
                    self.user_settings.api_key = Some(api_key.to_string());
                    self.save_user()?;
                    Ok(ApiKeySaveLocation::PlaintextFallback)
                }
            }
            ApiKeyStorageMode::Plaintext => {
                self.user_settings.api_key = Some(api_key.to_string());
                self.save_user()?;
                Ok(ApiKeySaveLocation::Plaintext)
            }
        }
    }

    pub fn update_user_base_url(&mut self, base_url: &str) -> Result<()> {
        self.user_settings.base_url = Some(base_url.to_string());
        self.save_user()
    }
}

fn migrate_user_settings(settings: &mut UserSettings) {
    if settings.base_url.is_none() {
        settings.base_url = Some(DEFAULT_BASE_URL.to_string());
    }
    if settings.default_model.is_none() {
        settings.default_model = Some(DEFAULT_MODEL.to_string());
    }
    if settings.models.is_none() {
        settings.models = Some(default_models());
    }
    if settings.api_key_storage.is_none() {
        settings.api_key_storage = Some(ApiKeyStorageMode::Keychain);
    }
    settings.settings_version = Some(SETTINGS_VERSION);
}

impl SettingsManager {
    fn maybe_migrate_plaintext_api_key_to_keychain(&mut self) -> Result<()> {
        if self.get_api_key_storage_mode() != ApiKeyStorageMode::Keychain {
            return Ok(());
        }
        let Some(api_key) = self.user_settings.api_key.clone() else {
            return Ok(());
        };

        if store_api_key_in_keychain(&api_key).is_ok() {
            self.user_settings.api_key = None;
            self.save_user()?;
        }
        Ok(())
    }
}

fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed creating directory {}", parent.display()))?;
    }
    Ok(())
}

fn load_json_or_default<T>(path: &Path) -> Result<T>
where
    T: Default + for<'de> Deserialize<'de>,
{
    if !path.exists() {
        return Ok(T::default());
    }
    let raw =
        fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))?;
    let parsed = serde_json::from_str::<T>(&raw)
        .with_context(|| format!("Failed to parse {}", path.display()))?;
    Ok(parsed)
}

fn write_json<T>(path: &Path, value: &T) -> Result<()>
where
    T: Serialize,
{
    let serialized =
        serde_json::to_string_pretty(value).context("Failed serializing settings JSON")?;
    fs::write(path, serialized).with_context(|| format!("Failed writing {}", path.display()))?;
    Ok(())
}

fn keyring_entry() -> std::result::Result<Entry, KeyringError> {
    Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT)
}

fn store_api_key_in_keychain(api_key: &str) -> Result<()> {
    let entry = keyring_entry().context("Failed opening keychain entry")?;
    entry
        .set_password(api_key)
        .context("Failed storing API key in keychain")
}

fn load_api_key_from_keychain() -> Result<Option<String>> {
    let entry = keyring_entry().context("Failed opening keychain entry")?;
    match entry.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(KeyringError::NoEntry) => Ok(None),
        Err(err) => Err(anyhow::anyhow!(err).context("Failed loading API key from keychain")),
    }
}
