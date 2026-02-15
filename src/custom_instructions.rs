use std::fs;
use std::path::Path;

pub fn load_custom_instructions(cwd: &Path) -> Option<String> {
    let project_path = cwd.join(".grok").join("GROK.md");
    if let Ok(content) = fs::read_to_string(project_path) {
        let trimmed = content.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }

    if let Some(home) = dirs::home_dir() {
        let global_path = home.join(".grok").join("GROK.md");
        if let Ok(content) = fs::read_to_string(global_path) {
            let trimmed = content.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::load_custom_instructions;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn prefers_project_instructions_and_trims_whitespace() {
        let temp = TempDir::new("custom-instructions-project");
        let project_grok_dir = temp.path().join(".grok");
        fs::create_dir_all(&project_grok_dir).expect("create project .grok dir");
        fs::write(
            project_grok_dir.join("GROK.md"),
            "\n\n  Keep responses short and actionable. \n",
        )
        .expect("write project GROK.md");

        let loaded = load_custom_instructions(temp.path());
        assert_eq!(
            loaded.as_deref(),
            Some("Keep responses short and actionable.")
        );
    }

    #[test]
    fn project_instructions_override_global_home_instructions() {
        let temp = TempDir::new("custom-instructions-override");
        let project_grok_dir = temp.path().join(".grok");
        fs::create_dir_all(&project_grok_dir).expect("create project .grok dir");
        fs::write(project_grok_dir.join("GROK.md"), "project instructions")
            .expect("write project GROK.md");

        let loaded = load_custom_instructions(temp.path());
        assert_eq!(loaded.as_deref(), Some("project instructions"));
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
            let path = std::env::temp_dir().join(format!("codetoad-{prefix}-{pid}-{nonce}"));
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
