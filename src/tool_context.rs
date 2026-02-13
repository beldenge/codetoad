use anyhow::{Context, Result, bail};
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
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

    let normalized = resolve_and_validate_locked(&guard, path)
        .with_context(|| format!("Failed to change directory to '{path}'"))?;
    if !normalized.is_dir() {
        bail!("Not a directory: {}", normalized.display());
    }
    guard.current_dir = normalized.clone();
    Ok(normalized)
}

pub fn resolve_path(path: &str) -> Result<PathBuf> {
    let guard = context_mutex()
        .lock()
        .map_err(|_| anyhow::anyhow!("Tool context lock poisoned"))?;
    if guard.current_dir.as_os_str().is_empty() {
        bail!("Tool context not initialized");
    }

    resolve_and_validate_locked(&guard, path)
}

fn normalize_existing_dir(path: &Path) -> Result<PathBuf> {
    let canonical = std::fs::canonicalize(path)
        .with_context(|| format!("Directory not found: {}", path.display()))?;
    if !canonical.is_dir() {
        bail!("Not a directory: {}", canonical.display());
    }
    Ok(canonical)
}

fn resolve_and_validate_locked(state: &ToolContextState, raw_path: &str) -> Result<PathBuf> {
    let candidate = if Path::new(raw_path).is_absolute() {
        PathBuf::from(raw_path)
    } else {
        state.current_dir.join(raw_path)
    };
    let normalized = lexical_normalize(&candidate);
    let resolved = resolve_with_existing_ancestor(&normalized)?;

    ensure_inside_project(&resolved, &state.project_root)?;
    Ok(resolved)
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => {
                let root = std::path::MAIN_SEPARATOR.to_string();
                normalized.push(root);
            }
            Component::CurDir => {}
            Component::ParentDir => {
                let _ = normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
}

fn resolve_with_existing_ancestor(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return std::fs::canonicalize(path)
            .with_context(|| format!("Failed to resolve path {}", path.display()));
    }

    let mut ancestor = path.to_path_buf();
    let mut suffix: Vec<OsString> = Vec::new();
    while !ancestor.exists() {
        let Some(name) = ancestor.file_name() else {
            bail!("Path has no existing ancestor: {}", path.display());
        };
        suffix.push(name.to_os_string());
        let Some(parent) = ancestor.parent() else {
            bail!("Path has no existing ancestor: {}", path.display());
        };
        ancestor = parent.to_path_buf();
    }

    let mut resolved = std::fs::canonicalize(&ancestor)
        .with_context(|| format!("Failed to resolve ancestor {}", ancestor.display()))?;
    if !suffix.is_empty() && !resolved.is_dir() {
        bail!("Cannot resolve nested path under file {}", resolved.display());
    }
    for segment in suffix.iter().rev() {
        resolved.push(segment);
    }
    Ok(resolved)
}

fn ensure_inside_project(path: &Path, project_root: &Path) -> Result<()> {
    if path.starts_with(project_root) {
        return Ok(());
    }
    bail!(
        "Path escapes project root: {} (root: {})",
        path.display(),
        project_root.display()
    );
}

#[cfg(test)]
mod tests {
    use super::lexical_normalize;
    use std::path::Path;

    #[test]
    fn lexical_normalize_collapses_parent_segments() {
        let normalized = lexical_normalize(Path::new("a/b/../c/./file.txt"));
        assert_eq!(normalized, Path::new("a/c/file.txt"));
    }
}
