use crate::provider::{
    ProviderKind, XAI_DEFAULT_BASE_URL, XAI_DEFAULT_MODEL, api_key_env_candidates,
    default_model_for, default_models_for, detect_provider,
};
use anyhow::{Context, Result, bail};
use dirs::home_dir;
use keyring::Entry;
use keyring::Error as KeyringError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const SETTINGS_VERSION: u32 = 2;
const DEFAULT_BASE_URL: &str = XAI_DEFAULT_BASE_URL;
const DEFAULT_MODEL: &str = XAI_DEFAULT_MODEL;
const KEYRING_SERVICE: &str = "grok-build";
const LEGACY_KEYRING_ACCOUNT: &str = "xai_api_key";
const KEYRING_ACCOUNT_PREFIX: &str = "provider";

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
    pub providers: Option<BTreeMap<String, ProviderProfile>>,
    #[serde(rename = "activeProvider", alias = "active_provider")]
    pub active_provider: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ProviderProfile {
    #[serde(rename = "displayName", alias = "display_name")]
    pub display_name: Option<String>,
    #[serde(rename = "baseURL", alias = "base_url")]
    pub base_url: String,
    #[serde(rename = "defaultModel", alias = "default_model")]
    pub default_model: Option<String>,
    pub models: Option<Vec<String>>,
    #[serde(rename = "apiKey", alias = "api_key")]
    pub api_key: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ProviderSummary {
    pub id: String,
    pub base_url: String,
    pub default_model: String,
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
        self.ensure_provider_catalog();

        if !self.user_settings_path.exists() {
            self.user_settings.settings_version = Some(SETTINGS_VERSION);
            if self.user_settings.api_key_storage.is_none() {
                self.user_settings.api_key_storage = Some(ApiKeyStorageMode::Keychain);
            }
            self.sync_legacy_fields_from_active();
            self.save_user()?;
        }

        if !self.project_settings_path.exists() {
            if self.project_settings.model.is_none() {
                self.project_settings.model = Some(self.active_default_model());
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
        for key in api_key_env_candidates(self.current_provider()) {
            if let Ok(from_env) = std::env::var(key)
                && !from_env.trim().is_empty()
            {
                return Some(from_env);
            }
        }

        let active_provider_id = self.active_provider_id();
        if self.get_api_key_storage_mode() == ApiKeyStorageMode::Keychain {
            if let Ok(Some(from_keychain)) = load_api_key_from_keychain(&active_provider_id)
                && !from_keychain.trim().is_empty()
            {
                return Some(from_keychain);
            }
            if let Ok(Some(legacy)) = load_legacy_api_key_from_keychain()
                && !legacy.trim().is_empty()
            {
                return Some(legacy);
            }
        }

        self.active_provider_profile()
            .and_then(|profile| profile.api_key.clone())
            .or_else(|| self.user_settings.api_key.clone())
    }

    pub fn get_base_url(&self) -> String {
        std::env::var("GROK_BASE_URL")
            .ok()
            .or_else(|| std::env::var("OPENAI_BASE_URL").ok())
            .or_else(|| {
                self.active_provider_profile()
                    .map(|profile| profile.base_url.clone())
            })
            .or_else(|| self.user_settings.base_url.clone())
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
    }

    pub fn get_current_model(&self) -> String {
        self.project_settings
            .model
            .clone()
            .or_else(|| std::env::var("GROK_MODEL").ok())
            .or_else(|| {
                self.active_provider_profile()
                    .and_then(|profile| profile.default_model.clone())
            })
            .or_else(|| self.user_settings.default_model.clone())
            .unwrap_or_else(|| DEFAULT_MODEL.to_string())
    }

    pub fn get_available_models(&self) -> Vec<String> {
        let models = self
            .active_provider_profile()
            .and_then(|profile| profile.models.clone())
            .or_else(|| self.user_settings.models.clone())
            .unwrap_or_else(|| default_models_for(self.current_provider()));

        models
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
                if self
                    .active_provider_profile()
                    .and_then(|p| p.api_key.as_ref())
                    .is_none()
                    && let Ok(Some(key)) = load_api_key_from_keychain(&self.active_provider_id())
                    && let Some(profile) = self.active_provider_profile_mut()
                {
                    profile.api_key = Some(key);
                }
                self.sync_legacy_fields_from_active();
                self.save_user()
            }
        }
    }

    pub fn update_user_api_key(&mut self, api_key: &str) -> Result<ApiKeySaveLocation> {
        let active_provider_id = self.active_provider_id();
        match self.get_api_key_storage_mode() {
            ApiKeyStorageMode::Keychain => {
                if store_api_key_in_keychain(&active_provider_id, api_key).is_ok() {
                    if let Some(profile) = self.active_provider_profile_mut() {
                        profile.api_key = None;
                    }
                    self.user_settings.api_key = None;
                    self.save_user()?;
                    Ok(ApiKeySaveLocation::Keychain)
                } else {
                    if let Some(profile) = self.active_provider_profile_mut() {
                        profile.api_key = Some(api_key.to_string());
                    }
                    self.sync_legacy_fields_from_active();
                    self.save_user()?;
                    Ok(ApiKeySaveLocation::PlaintextFallback)
                }
            }
            ApiKeyStorageMode::Plaintext => {
                if let Some(profile) = self.active_provider_profile_mut() {
                    profile.api_key = Some(api_key.to_string());
                }
                self.sync_legacy_fields_from_active();
                self.save_user()?;
                Ok(ApiKeySaveLocation::Plaintext)
            }
        }
    }

    pub fn update_user_base_url(&mut self, base_url: &str) -> Result<()> {
        if let Some(profile) = self.active_provider_profile_mut() {
            let previous_provider = detect_provider(&profile.base_url);
            profile.base_url = base_url.to_string();
            let next_provider = detect_provider(base_url);
            maybe_update_profile_defaults_for_provider_change(
                profile,
                previous_provider,
                next_provider,
            );
        } else {
            let provider = detect_provider(base_url);
            self.add_or_update_provider(
                default_provider_id_for(detect_provider(base_url)),
                base_url,
                Some(default_model_for(provider).to_string()),
                Some(default_models_for(provider)),
            )?;
        }

        self.sync_legacy_fields_from_active();
        self.save_user()
    }

    pub fn active_provider_id(&self) -> String {
        if let Some(active) = self.user_settings.active_provider.as_ref()
            && self
                .user_settings
                .providers
                .as_ref()
                .is_some_and(|providers| providers.contains_key(active))
        {
            return active.clone();
        }

        if let Some(first) = self
            .user_settings
            .providers
            .as_ref()
            .and_then(|providers| providers.keys().next())
        {
            return first.clone();
        }

        default_provider_id_for(self.current_provider()).to_string()
    }

    pub fn list_provider_summaries(&self) -> Vec<ProviderSummary> {
        self.user_settings
            .providers
            .as_ref()
            .map(|providers| {
                providers
                    .iter()
                    .map(|(id, profile)| {
                        let provider = detect_provider(&profile.base_url);
                        ProviderSummary {
                            id: id.to_string(),
                            base_url: profile.base_url.clone(),
                            default_model: profile
                                .default_model
                                .clone()
                                .unwrap_or_else(|| default_model_for(provider).to_string()),
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    }

    pub fn add_or_update_provider(
        &mut self,
        provider_id: &str,
        base_url: &str,
        default_model: Option<String>,
        models: Option<Vec<String>>,
    ) -> Result<()> {
        let provider_id = normalize_provider_id(provider_id)
            .ok_or_else(|| anyhow::anyhow!("Provider id cannot be empty"))?;
        let provider_kind = detect_provider(base_url);

        let profile = self
            .user_settings
            .providers
            .get_or_insert_with(BTreeMap::new)
            .entry(provider_id.clone())
            .or_default();
        profile.base_url = base_url.to_string();
        profile.default_model =
            default_model.or_else(|| Some(default_model_for(provider_kind).to_string()));
        profile.models = models.or_else(|| Some(default_models_for(provider_kind)));

        if self.user_settings.active_provider.is_none() {
            self.user_settings.active_provider = Some(provider_id);
        }

        self.sync_legacy_fields_from_active();
        self.save_user()
    }

    pub fn switch_active_provider(&mut self, provider_id: &str) -> Result<()> {
        if !self
            .user_settings
            .providers
            .as_ref()
            .is_some_and(|providers| providers.contains_key(provider_id))
        {
            bail!("Unknown provider: {provider_id}");
        }

        self.user_settings.active_provider = Some(provider_id.to_string());
        self.project_settings.model = Some(self.active_default_model());
        self.sync_legacy_fields_from_active();
        self.save_user()?;
        self.save_project()?;
        Ok(())
    }

    fn current_provider(&self) -> ProviderKind {
        detect_provider(&self.get_base_url())
    }

    fn active_default_model(&self) -> String {
        self.active_provider_profile()
            .and_then(|profile| profile.default_model.clone())
            .unwrap_or_else(|| default_model_for(self.current_provider()).to_string())
    }

    fn active_provider_profile(&self) -> Option<&ProviderProfile> {
        let providers = self.user_settings.providers.as_ref()?;
        providers.get(&self.active_provider_id())
    }

    fn active_provider_profile_mut(&mut self) -> Option<&mut ProviderProfile> {
        let id = self.active_provider_id();
        self.user_settings
            .providers
            .as_mut()
            .and_then(|providers| providers.get_mut(&id))
    }

    fn ensure_provider_catalog(&mut self) {
        if self
            .user_settings
            .providers
            .as_ref()
            .is_none_or(|providers| providers.is_empty())
        {
            let base_url = self
                .user_settings
                .base_url
                .clone()
                .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
            let provider_kind = detect_provider(&base_url);
            let provider_id = default_provider_id_for(provider_kind).to_string();

            let mut providers = BTreeMap::new();
            providers.insert(
                provider_id.clone(),
                ProviderProfile {
                    display_name: None,
                    base_url,
                    default_model: self
                        .user_settings
                        .default_model
                        .clone()
                        .or_else(|| Some(default_model_for(provider_kind).to_string())),
                    models: self
                        .user_settings
                        .models
                        .clone()
                        .or_else(|| Some(default_models_for(provider_kind))),
                    api_key: self.user_settings.api_key.clone(),
                },
            );
            self.user_settings.providers = Some(providers);
            self.user_settings.active_provider = Some(provider_id);
        }

        let active_missing = self.user_settings.active_provider.is_none()
            || !self
                .user_settings
                .providers
                .as_ref()
                .is_some_and(|providers| {
                    self.user_settings
                        .active_provider
                        .as_ref()
                        .is_some_and(|id| providers.contains_key(id))
                });
        if active_missing
            && let Some(first) = self
                .user_settings
                .providers
                .as_ref()
                .and_then(|providers| providers.keys().next())
                .cloned()
        {
            self.user_settings.active_provider = Some(first);
        }

        if let Some(providers) = self.user_settings.providers.as_mut() {
            for profile in providers.values_mut() {
                let provider_kind = detect_provider(&profile.base_url);
                if profile.default_model.is_none() {
                    profile.default_model = Some(default_model_for(provider_kind).to_string());
                }
                if profile.models.is_none() {
                    profile.models = Some(default_models_for(provider_kind));
                }
            }
        }

        self.sync_legacy_fields_from_active();
    }

    fn sync_legacy_fields_from_active(&mut self) {
        if let Some((base_url, default_model, models, api_key)) = self
            .active_provider_profile()
            .map(|profile| {
                (
                    profile.base_url.clone(),
                    profile.default_model.clone(),
                    profile.models.clone(),
                    profile.api_key.clone(),
                )
            })
        {
            self.user_settings.base_url = Some(base_url);
            self.user_settings.default_model = default_model;
            self.user_settings.models = models;
            self.user_settings.api_key = api_key;
        }
        self.user_settings.settings_version = Some(SETTINGS_VERSION);
    }
}

fn migrate_user_settings(settings: &mut UserSettings) {
    if settings.api_key_storage.is_none() {
        settings.api_key_storage = Some(ApiKeyStorageMode::Keychain);
    }

    if settings
        .providers
        .as_ref()
        .is_none_or(|providers| providers.is_empty())
    {
        let base_url = settings
            .base_url
            .clone()
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        let provider_kind = detect_provider(&base_url);
        let provider_id = default_provider_id_for(provider_kind).to_string();

        let mut providers = BTreeMap::new();
        providers.insert(
            provider_id.clone(),
            ProviderProfile {
                display_name: None,
                base_url: base_url.clone(),
                default_model: settings
                    .default_model
                    .clone()
                    .or_else(|| Some(default_model_for(provider_kind).to_string())),
                models: settings
                    .models
                    .clone()
                    .or_else(|| Some(default_models_for(provider_kind))),
                api_key: settings.api_key.clone(),
            },
        );
        settings.providers = Some(providers);
        settings.active_provider = Some(provider_id);
    }

    if settings.active_provider.is_none()
        && let Some(first) = settings
            .providers
            .as_ref()
            .and_then(|providers| providers.keys().next())
            .cloned()
    {
        settings.active_provider = Some(first);
    }

    settings.settings_version = Some(SETTINGS_VERSION);
}

fn maybe_update_profile_defaults_for_provider_change(
    profile: &mut ProviderProfile,
    previous_provider: ProviderKind,
    next_provider: ProviderKind,
) {
    let previous_default_model = default_model_for(previous_provider);
    let next_default_model = default_model_for(next_provider);

    if profile
        .default_model
        .as_deref()
        .map(str::trim)
        .is_none_or(|model| model == previous_default_model)
    {
        profile.default_model = Some(next_default_model.to_string());
    }

    let should_replace_models = match profile.models.as_ref() {
        None => true,
        Some(existing) => models_match(existing, &default_models_for(previous_provider)),
    };
    if should_replace_models {
        profile.models = Some(default_models_for(next_provider));
    }
}

fn default_provider_id_for(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Xai => "xai",
        ProviderKind::OpenAi => "openai",
        ProviderKind::Compatible => "default",
    }
}

fn normalize_provider_id(requested_id: &str) -> Option<String> {
    let trimmed = requested_id.trim();
    if trimmed.is_empty() {
        return None;
    }

    let normalized = trimmed
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();

    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

fn models_match(current: &[String], defaults: &[String]) -> bool {
    if current.len() != defaults.len() {
        return false;
    }
    current
        .iter()
        .zip(defaults)
        .all(|(lhs, rhs)| lhs.trim().eq_ignore_ascii_case(rhs.trim()))
}

impl SettingsManager {
    fn maybe_migrate_plaintext_api_key_to_keychain(&mut self) -> Result<()> {
        if self.get_api_key_storage_mode() != ApiKeyStorageMode::Keychain {
            return Ok(());
        }

        let provider_id = self.active_provider_id();
        let Some(api_key) = self
            .active_provider_profile()
            .and_then(|profile| profile.api_key.clone())
        else {
            return Ok(());
        };

        if store_api_key_in_keychain(&provider_id, &api_key).is_ok() {
            if let Some(profile) = self.active_provider_profile_mut() {
                profile.api_key = None;
            }
            self.sync_legacy_fields_from_active();
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

fn keyring_entry(account: &str) -> std::result::Result<Entry, KeyringError> {
    Entry::new(KEYRING_SERVICE, account)
}

fn keyring_account_for(provider_id: &str) -> String {
    let id = normalize_provider_id(provider_id).unwrap_or_else(|| "default".to_string());
    format!("{KEYRING_ACCOUNT_PREFIX}_{id}")
}

fn store_api_key_in_keychain(provider_id: &str, api_key: &str) -> Result<()> {
    let account = keyring_account_for(provider_id);
    let entry = keyring_entry(&account).context("Failed opening keychain entry")?;
    entry
        .set_password(api_key)
        .context("Failed storing API key in keychain")
}

fn load_api_key_from_keychain(provider_id: &str) -> Result<Option<String>> {
    let account = keyring_account_for(provider_id);
    let entry = keyring_entry(&account).context("Failed opening keychain entry")?;
    match entry.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(KeyringError::NoEntry) => Ok(None),
        Err(err) => Err(anyhow::anyhow!(err).context("Failed loading API key from keychain")),
    }
}

fn load_legacy_api_key_from_keychain() -> Result<Option<String>> {
    let entry = keyring_entry(LEGACY_KEYRING_ACCOUNT).context("Failed opening keychain entry")?;
    match entry.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(KeyringError::NoEntry) => Ok(None),
        Err(err) => Err(anyhow::anyhow!(err).context("Failed loading API key from keychain")),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        UserSettings, default_provider_id_for, migrate_user_settings, models_match,
        normalize_provider_id,
    };
    use crate::provider::ProviderKind;

    #[test]
    fn models_match_ignores_case_and_whitespace() {
        let current = vec![" GPT-4.1 ".to_string(), "o4-mini".to_string()];
        let defaults = vec!["gpt-4.1".to_string(), "o4-mini".to_string()];
        assert!(models_match(&current, &defaults));
    }

    #[test]
    fn migration_builds_provider_catalog() {
        let mut settings = UserSettings {
            base_url: Some("https://api.openai.com/v1".to_string()),
            ..UserSettings::default()
        };

        migrate_user_settings(&mut settings);

        assert_eq!(settings.active_provider.as_deref(), Some("openai"));
        assert!(
            settings
                .providers
                .as_ref()
                .is_some_and(|providers| providers.contains_key("openai"))
        );
    }

    #[test]
    fn provider_id_normalization_is_stable() {
        assert_eq!(
            normalize_provider_id(" XAI Prod "),
            Some("xai-prod".to_string())
        );
        assert_eq!(normalize_provider_id(""), None);
    }

    #[test]
    fn default_provider_ids_match_kind() {
        assert_eq!(default_provider_id_for(ProviderKind::Xai), "xai");
        assert_eq!(default_provider_id_for(ProviderKind::OpenAi), "openai");
        assert_eq!(default_provider_id_for(ProviderKind::Compatible), "default");
    }
}
