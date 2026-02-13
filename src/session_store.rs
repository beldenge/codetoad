use crate::agent::AgentSessionSnapshot;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const SESSIONS_DIR: &str = ".grok/sessions";
const SESSION_FILE_EXTENSION: &str = "json";
const SESSION_FILE_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionFile {
    version: u32,
    saved_at_epoch_ms: u128,
    snapshot: AgentSessionSnapshot,
}

pub(crate) fn save_session(
    cwd: &Path,
    requested_name: Option<&str>,
    snapshot: AgentSessionSnapshot,
) -> Result<String> {
    let sessions_dir = ensure_sessions_dir(cwd)?;
    let session_name = normalize_session_name(requested_name)
        .unwrap_or_else(|| format!("session-{}", epoch_millis()));
    let session_file = SessionFile {
        version: SESSION_FILE_VERSION,
        saved_at_epoch_ms: epoch_millis(),
        snapshot,
    };

    let path = session_path(&sessions_dir, &session_name);
    let payload = serde_json::to_string_pretty(&session_file).context("Failed encoding session")?;
    std::fs::write(&path, payload)
        .with_context(|| format!("Failed writing session file {}", path.display()))?;
    Ok(session_name)
}

pub(crate) fn load_session(cwd: &Path, requested_name: &str) -> Result<AgentSessionSnapshot> {
    let sessions_dir = ensure_sessions_dir(cwd)?;
    let session_name = normalize_session_name(Some(requested_name))
        .ok_or_else(|| anyhow::anyhow!("Session name cannot be empty"))?;
    let path = session_path(&sessions_dir, &session_name);
    if !path.exists() {
        bail!("Session not found: {session_name}");
    }

    let payload = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed reading session file {}", path.display()))?;
    let session_file: SessionFile =
        serde_json::from_str(&payload).with_context(|| format!("Invalid session file {}", path.display()))?;
    if session_file.version != SESSION_FILE_VERSION {
        bail!(
            "Unsupported session file version {} for {}",
            session_file.version,
            session_name
        );
    }

    Ok(session_file.snapshot)
}

pub(crate) fn list_sessions(cwd: &Path) -> Result<Vec<String>> {
    let sessions_dir = ensure_sessions_dir(cwd)?;
    let mut names = Vec::new();
    for entry in std::fs::read_dir(&sessions_dir)
        .with_context(|| format!("Failed listing {}", sessions_dir.display()))?
    {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let path = entry.path();
        let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
            continue;
        };
        if ext != SESSION_FILE_EXTENSION {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        names.push(stem.to_string());
    }
    names.sort();
    Ok(names)
}

fn ensure_sessions_dir(cwd: &Path) -> Result<PathBuf> {
    let directory = cwd.join(SESSIONS_DIR);
    std::fs::create_dir_all(&directory)
        .with_context(|| format!("Failed creating sessions directory {}", directory.display()))?;
    Ok(directory)
}

fn session_path(sessions_dir: &Path, session_name: &str) -> PathBuf {
    sessions_dir.join(format!("{session_name}.{SESSION_FILE_EXTENSION}"))
}

fn normalize_session_name(requested_name: Option<&str>) -> Option<String> {
    let raw = requested_name?.trim();
    if raw.is_empty() {
        return None;
    }

    let normalized = raw
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
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

fn epoch_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
