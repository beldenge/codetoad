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
