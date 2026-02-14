use crate::agent::Agent;
use crate::session_store::save_session;
use crate::settings::SettingsManager;
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

#[derive(Debug, Clone, Copy, Default)]
pub struct RuntimeFlags {
    pub auto_edit: bool,
}

#[derive(Clone)]
pub struct AppContext {
    cwd: PathBuf,
    agent: Arc<Mutex<Agent>>,
    settings: Arc<Mutex<SettingsManager>>,
    runtime_flags: Arc<Mutex<RuntimeFlags>>,
    active_session_name: Arc<Mutex<Option<String>>>,
}

impl AppContext {
    pub fn new(cwd: PathBuf, agent: Agent, settings: SettingsManager) -> Self {
        Self {
            cwd,
            agent: Arc::new(Mutex::new(agent)),
            settings: Arc::new(Mutex::new(settings)),
            runtime_flags: Arc::new(Mutex::new(RuntimeFlags::default())),
            active_session_name: Arc::new(Mutex::new(None)),
        }
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub fn agent(&self) -> Arc<Mutex<Agent>> {
        self.agent.clone()
    }

    pub fn settings(&self) -> Arc<Mutex<SettingsManager>> {
        self.settings.clone()
    }

    pub async fn auto_edit_enabled(&self) -> bool {
        self.runtime_flags.lock().await.auto_edit
    }

    pub async fn set_auto_edit_enabled(&self, enabled: bool) {
        {
            let mut flags = self.runtime_flags.lock().await;
            flags.auto_edit = enabled;
        }
        self.agent.lock().await.set_auto_edit_enabled(enabled);
    }

    pub async fn sync_auto_edit_from_agent(&self) {
        let enabled = self.agent.lock().await.auto_edit_enabled();
        let mut flags = self.runtime_flags.lock().await;
        flags.auto_edit = enabled;
    }

    pub async fn active_session_name(&self) -> Option<String> {
        self.active_session_name.lock().await.clone()
    }

    pub async fn set_active_session_name(&self, name: String) {
        let mut guard = self.active_session_name.lock().await;
        *guard = Some(name);
    }

    pub async fn autosave_session(&self) -> Result<String> {
        let snapshot = self.agent.lock().await.session_snapshot()?;
        let session_name = {
            let mut guard = self.active_session_name.lock().await;
            if guard.is_none() {
                *guard = Some(default_session_name());
            }
            guard.clone().unwrap_or_default()
        };
        let saved = save_session(&self.cwd, Some(&session_name), snapshot)?;
        Ok(saved)
    }
}

fn default_session_name() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("auto-{millis}")
}

#[cfg(test)]
mod tests {
    use super::{AppContext, default_session_name};
    use crate::agent::Agent;
    use crate::settings::SettingsManager;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn default_session_name_has_expected_prefix() {
        let name = default_session_name();
        assert!(name.starts_with("auto-"));
        assert!(name["auto-".len()..].chars().all(|ch| ch.is_ascii_digit()));
    }

    #[tokio::test]
    async fn set_auto_edit_enabled_updates_runtime_and_agent_flags() {
        let temp = TempDir::new("app-context-auto-edit");
        let settings = SettingsManager::load_with_home(temp.path(), temp.path()).expect("settings");
        let agent = Agent::new(
            "test-key".to_string(),
            "https://api.x.ai/v1".to_string(),
            "grok-code-fast-1".to_string(),
            2,
            temp.path(),
        )
        .expect("agent");
        let app = AppContext::new(temp.path().to_path_buf(), agent, settings);

        app.set_auto_edit_enabled(true).await;
        assert!(app.auto_edit_enabled().await);
        assert!(app.agent().lock().await.auto_edit_enabled());

        app.set_auto_edit_enabled(false).await;
        assert!(!app.auto_edit_enabled().await);
        assert!(!app.agent().lock().await.auto_edit_enabled());
    }

    #[tokio::test]
    async fn autosave_session_reuses_active_session_name() {
        let temp = TempDir::new("app-context-autosave");
        let settings = SettingsManager::load_with_home(temp.path(), temp.path()).expect("settings");
        let agent = Agent::new(
            "test-key".to_string(),
            "https://api.x.ai/v1".to_string(),
            "grok-code-fast-1".to_string(),
            2,
            temp.path(),
        )
        .expect("agent");
        let app = AppContext::new(temp.path().to_path_buf(), agent, settings);

        let first = app.autosave_session().await.expect("first autosave");
        let second = app.autosave_session().await.expect("second autosave");
        assert_eq!(first, second);

        let file = temp
            .path()
            .join(".grok")
            .join("sessions")
            .join(format!("{first}.json"));
        assert!(file.exists(), "expected session file at {}", file.display());
    }

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(prefix: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock should be after epoch")
                .as_nanos();
            let pid = std::process::id();
            let path = std::env::temp_dir().join(format!("grok-build-{prefix}-{pid}-{nonce}"));
            fs::create_dir_all(&path).expect("create temp dir");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
