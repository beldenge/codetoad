use crate::agent::Agent;
use crate::settings::SettingsManager;
use std::path::{Path, PathBuf};
use std::sync::Arc;
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
}

impl AppContext {
    pub fn new(cwd: PathBuf, agent: Agent, settings: SettingsManager) -> Self {
        Self {
            cwd,
            agent: Arc::new(Mutex::new(agent)),
            settings: Arc::new(Mutex::new(settings)),
            runtime_flags: Arc::new(Mutex::new(RuntimeFlags::default())),
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
}
