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
