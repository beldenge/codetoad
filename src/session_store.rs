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
    let session_file: SessionFile = serde_json::from_str(&payload)
        .with_context(|| format!("Invalid session file {}", path.display()))?;
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
    let mut entries: Vec<(String, std::time::SystemTime)> = Vec::new();
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
        let modified = entry
            .metadata()
            .ok()
            .and_then(|meta| meta.modified().ok())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        entries.push((stem.to_string(), modified));
    }
    entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let names = entries.into_iter().map(|(name, _)| name).collect();
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

#[cfg(test)]
mod tests {
    use super::{
        SESSION_FILE_EXTENSION, SESSIONS_DIR, SessionFile, list_sessions, load_session,
        normalize_session_name, save_session,
    };
    use crate::agent::AgentSessionSnapshot;
    use serde_json::json;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::thread;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    #[test]
    fn normalize_session_name_sanitizes_and_rejects_empty_values() {
        assert_eq!(
            normalize_session_name(Some("  my/session:name  ")),
            Some("my-session-name".to_string())
        );
        assert_eq!(
            normalize_session_name(Some("keep_me-123")),
            Some("keep_me-123".to_string())
        );
        assert_eq!(normalize_session_name(Some("   ")), None);
        assert_eq!(normalize_session_name(Some("---")), None);
        assert_eq!(normalize_session_name(None), None);
    }

    #[test]
    fn save_and_load_session_round_trip() {
        let temp = TempDir::new("session-store-roundtrip");
        let snapshot = sample_snapshot();

        let saved_name =
            save_session(temp.path(), Some(" Sprint / Alpha "), snapshot.clone()).expect("save");
        assert_eq!(saved_name, "Sprint---Alpha");

        let loaded = load_session(temp.path(), "Sprint / Alpha").expect("load");
        assert_eq!(
            serde_json::to_value(loaded).expect("serialize loaded"),
            serde_json::to_value(snapshot).expect("serialize snapshot")
        );
    }

    #[test]
    fn load_session_fails_for_missing_session() {
        let temp = TempDir::new("session-store-missing");
        let err = load_session(temp.path(), "does-not-exist").expect_err("must fail");
        assert!(err.to_string().contains("Session not found"));
    }

    #[test]
    fn load_session_rejects_unsupported_version() {
        let temp = TempDir::new("session-store-version");
        let sessions_dir = temp.path().join(SESSIONS_DIR);
        fs::create_dir_all(&sessions_dir).expect("create sessions dir");

        let invalid = SessionFile {
            version: 99,
            saved_at_epoch_ms: 0,
            snapshot: sample_snapshot(),
        };
        let path = sessions_dir.join(format!("bad.{SESSION_FILE_EXTENSION}"));
        fs::write(
            &path,
            serde_json::to_string_pretty(&invalid).expect("encode invalid"),
        )
        .expect("write invalid session");

        let err = load_session(temp.path(), "bad").expect_err("must fail");
        assert!(err.to_string().contains("Unsupported session file version"));
    }

    #[test]
    fn list_sessions_returns_json_files_sorted_by_recent_mtime() {
        let temp = TempDir::new("session-store-list");
        let first = sample_snapshot();
        let second = sample_snapshot();

        save_session(temp.path(), Some("first"), first).expect("save first");
        thread::sleep(Duration::from_millis(20));
        save_session(temp.path(), Some("second"), second).expect("save second");

        let sessions_dir = temp.path().join(SESSIONS_DIR);
        fs::create_dir_all(sessions_dir.join("nested")).expect("create nested dir");
        fs::write(sessions_dir.join("ignored.txt"), "ignore").expect("write ignored");

        let names = list_sessions(temp.path()).expect("list sessions");
        assert_eq!(names, vec!["second".to_string(), "first".to_string()]);
    }

    fn sample_snapshot() -> AgentSessionSnapshot {
        serde_json::from_value(json!({
            "model": "grok-code-fast-1",
            "messages": [
                {
                    "role": "system",
                    "content": "system prompt"
                },
                {
                    "role": "user",
                    "content": "hello"
                }
            ],
            "tool_session": {
                "current_dir": ".",
                "todos": {
                    "todos": []
                }
            },
            "auto_edit_enabled": false,
            "session_allow_file_ops": false,
            "session_allow_bash_ops": false
        }))
        .expect("decode snapshot")
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
