use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone)]
struct ToolContextState {
    project_root: PathBuf,
    current_dir: PathBuf,
}

static TOOL_CONTEXT: OnceLock<Mutex<ToolContextState>> = OnceLock::new();

fn context_mutex() -> &'static Mutex<ToolContextState> {
    TOOL_CONTEXT.get_or_init(|| {
        Mutex::new(ToolContextState {
            project_root: PathBuf::new(),
            current_dir: PathBuf::new(),
        })
    })
}

pub fn initialize(project_root: PathBuf) -> Result<()> {
    let normalized_root = normalize_existing_dir(&project_root)
        .with_context(|| format!("Failed to initialize tool context for {}", project_root.display()))?;
    let mut guard = context_mutex()
        .lock()
        .map_err(|_| anyhow::anyhow!("Tool context lock poisoned"))?;
    guard.project_root = normalized_root.clone();
    guard.current_dir = normalized_root;
    Ok(())
}

pub fn current_dir() -> Result<PathBuf> {
    let guard = context_mutex()
        .lock()
        .map_err(|_| anyhow::anyhow!("Tool context lock poisoned"))?;
    if guard.current_dir.as_os_str().is_empty() {
        bail!("Tool context not initialized");
    }
    Ok(guard.current_dir.clone())
}

pub fn set_current_dir(path: &str) -> Result<PathBuf> {
    let mut guard = context_mutex()
        .lock()
        .map_err(|_| anyhow::anyhow!("Tool context lock poisoned"))?;
    if guard.current_dir.as_os_str().is_empty() {
        bail!("Tool context not initialized");
    }

    let candidate = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        guard.current_dir.join(path)
    };

    let normalized = normalize_existing_dir(&candidate)
        .with_context(|| format!("Failed to change directory to '{path}'"))?;
    guard.current_dir = normalized.clone();
    Ok(normalized)
}

pub fn resolve_path(path: &str) -> Result<PathBuf> {
    if Path::new(path).is_absolute() {
        return Ok(PathBuf::from(path));
    }

    let base = current_dir()?;
    Ok(base.join(path))
}

fn normalize_existing_dir(path: &Path) -> Result<PathBuf> {
    let canonical = std::fs::canonicalize(path)
        .with_context(|| format!("Directory not found: {}", path.display()))?;
    if !canonical.is_dir() {
        bail!("Not a directory: {}", canonical.display());
    }
    Ok(canonical)
}
