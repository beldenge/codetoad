use crate::custom_instructions::load_custom_instructions;
use crate::grok_client::SearchMode;
use crate::protocol::ChatMessage;
use std::path::Path;

pub(crate) fn build_system_prompt(cwd: &Path) -> String {
    let custom = load_custom_instructions(cwd)
        .map(|instructions| {
            format!(
                "\n\nCUSTOM INSTRUCTIONS:\n{}\n\nFollow the custom instructions above while respecting the tool safety constraints below.\n",
                instructions
            )
        })
        .unwrap_or_default();

    format!(
        "You are Grok CLI, an AI coding assistant in a terminal environment.{custom}
You can use these tools:
- view_file: Read file contents or list directories.
- create_file: Create a new file.
- str_replace_editor: Replace text in an existing file.
- bash: Run shell commands.
- search: Find text and files.
- create_todo_list: Create a todo checklist.
- update_todo_list: Update todo checklist items.

Important behavior:
- Use view_file before editing when practical.
- Use str_replace_editor for existing files instead of create_file.
- Keep responses concise and directly tied to the task.
- Use bash for file discovery and command execution when useful.
- Use search for broad text or file discovery across the workspace.

Current working directory: {}",
        cwd.display()
    )
}

pub(crate) fn search_mode_for(message: &str) -> SearchMode {
    if should_use_search_for(message) {
        SearchMode::Auto
    } else {
        SearchMode::Off
    }
}

fn should_use_search_for(message: &str) -> bool {
    let lowered = message.to_lowercase();
    let keywords = [
        "today",
        "latest",
        "news",
        "trending",
        "current",
        "recent",
        "price",
        "release notes",
        "changelog",
    ];
    keywords.iter().any(|keyword| lowered.contains(keyword))
}

pub(crate) fn estimate_messages_tokens(messages: &[ChatMessage]) -> usize {
    let mut chars = 0usize;
    for message in messages {
        chars += message.role.chars().count();
        if let Some(content) = &message.content {
            chars += content.chars().count();
        }
        if let Some(attachments) = &message.attachments {
            for attachment in attachments {
                chars += attachment.filename.chars().count();
                chars += attachment.mime_type.chars().count();
                // Approximate per-image token impact without counting full base64 payload.
                chars += 512;
            }
        }
        if let Some(tool_id) = &message.tool_call_id {
            chars += tool_id.chars().count();
        }
        if let Some(tool_calls) = &message.tool_calls {
            for call in tool_calls {
                chars += call.id.chars().count();
                chars += call.function.name.chars().count();
                chars += call.function.arguments.chars().count();
            }
        }
    }
    estimate_chars_tokens(chars)
}

pub(crate) fn estimate_text_tokens(text: &str) -> usize {
    estimate_chars_tokens(text.chars().count())
}

fn estimate_chars_tokens(char_count: usize) -> usize {
    if char_count == 0 {
        0
    } else {
        // Rough token approximation for streaming UX feedback.
        char_count.div_ceil(4)
    }
}

#[cfg(test)]
mod tests {
    use super::search_mode_for;
    use crate::grok_client::SearchMode;

    #[test]
    fn search_mode_is_auto_for_recency_keywords() {
        assert!(matches!(
            search_mode_for("what is the latest grok release"),
            SearchMode::Auto
        ));
        assert!(matches!(
            search_mode_for("check current ai news"),
            SearchMode::Auto
        ));
    }

    #[test]
    fn search_mode_is_off_for_regular_code_prompt() {
        assert!(matches!(
            search_mode_for("refactor src/agent.rs"),
            SearchMode::Off
        ));
    }
}
