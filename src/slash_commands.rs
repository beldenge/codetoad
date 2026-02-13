#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CommandGroup {
    BuiltIn,
    Session,
    Git,
}

#[derive(Clone, Copy)]
pub struct SlashCommand {
    pub command: &'static str,
    pub description: &'static str,
    pub group: CommandGroup,
    pub show_in_suggestions: bool,
}

impl SlashCommand {
    const fn new(
        command: &'static str,
        description: &'static str,
        group: CommandGroup,
        show_in_suggestions: bool,
    ) -> Self {
        Self {
            command,
            description,
            group,
            show_in_suggestions,
        }
    }
}

pub const SLASH_COMMANDS: &[SlashCommand] = &[
    SlashCommand::new(
        "/help",
        "Show help information",
        CommandGroup::BuiltIn,
        true,
    ),
    SlashCommand::new(
        "/clear",
        "Clear chat history",
        CommandGroup::BuiltIn,
        true,
    ),
    SlashCommand::new(
        "/models",
        "Switch between available models",
        CommandGroup::BuiltIn,
        true,
    ),
    SlashCommand::new(
        "/models <name>",
        "Set model directly",
        CommandGroup::BuiltIn,
        false,
    ),
    SlashCommand::new("/exit", "Exit application", CommandGroup::BuiltIn, true),
    SlashCommand::new(
        "/save",
        "Save current session (auto name)",
        CommandGroup::Session,
        true,
    ),
    SlashCommand::new(
        "/save <name>",
        "Save session with explicit name",
        CommandGroup::Session,
        false,
    ),
    SlashCommand::new(
        "/load",
        "Load a saved session (requires name)",
        CommandGroup::Session,
        true,
    ),
    SlashCommand::new(
        "/load <name>",
        "Load a saved session by name",
        CommandGroup::Session,
        false,
    ),
    SlashCommand::new(
        "/sessions",
        "List saved sessions",
        CommandGroup::Session,
        true,
    ),
    SlashCommand::new(
        "/commit-and-push",
        "AI-generated commit and push",
        CommandGroup::Git,
        true,
    ),
];

pub enum ParsedSlashCommand {
    Help,
    Clear,
    Models,
    SetModel(String),
    Save(Option<String>),
    Load(String),
    Sessions,
    CommitAndPush,
    Exit,
}

pub fn parse_slash_command(input: &str) -> Option<ParsedSlashCommand> {
    let trimmed = input.trim();
    match trimmed {
        "/help" => Some(ParsedSlashCommand::Help),
        "/clear" => Some(ParsedSlashCommand::Clear),
        "/models" => Some(ParsedSlashCommand::Models),
        "/save" => Some(ParsedSlashCommand::Save(None)),
        "/sessions" => Some(ParsedSlashCommand::Sessions),
        "/load" => Some(ParsedSlashCommand::Load(String::new())),
        "/commit-and-push" => Some(ParsedSlashCommand::CommitAndPush),
        "/exit" => Some(ParsedSlashCommand::Exit),
        _ => {
            if let Some(model) = trimmed.strip_prefix("/models ").map(str::trim)
                && !model.is_empty()
            {
                return Some(ParsedSlashCommand::SetModel(model.to_string()));
            }
            if let Some(name) = trimmed.strip_prefix("/save ").map(str::trim)
                && !name.is_empty()
            {
                return Some(ParsedSlashCommand::Save(Some(name.to_string())));
            }
            if let Some(name) = trimmed.strip_prefix("/load ").map(str::trim)
                && !name.is_empty()
            {
                return Some(ParsedSlashCommand::Load(name.to_string()));
            }
            None
        }
    }
}

pub fn filtered_command_suggestions(input: &str) -> Vec<&'static SlashCommand> {
    if !input.starts_with('/') {
        return Vec::new();
    }

    let prefix = input.trim();
    SLASH_COMMANDS
        .iter()
        .filter(|entry| entry.show_in_suggestions)
        .filter(|entry| entry.command.starts_with(prefix))
        .collect()
}

pub fn append_help_section(output: &mut String, title: &str, group: CommandGroup) {
    output.push_str(title);
    output.push_str(":\n");
    for command in SLASH_COMMANDS.iter().filter(|entry| entry.group == group) {
        output.push_str(&format!(
            "  {:<18} {}\n",
            command.command, command.description
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::{ParsedSlashCommand, parse_slash_command};

    #[test]
    fn parses_models_set_command() {
        match parse_slash_command("/models grok-4-latest") {
            Some(ParsedSlashCommand::SetModel(model)) => {
                assert_eq!(model, "grok-4-latest");
            }
            _ => panic!("expected SetModel"),
        }
    }

    #[test]
    fn ignores_unknown_slash_commands() {
        assert!(parse_slash_command("/not-real").is_none());
    }

    #[test]
    fn parses_session_commands() {
        assert!(matches!(
            parse_slash_command("/save"),
            Some(ParsedSlashCommand::Save(None))
        ));
        assert!(matches!(
            parse_slash_command("/save my-session"),
            Some(ParsedSlashCommand::Save(Some(name))) if name == "my-session"
        ));
        assert!(matches!(
            parse_slash_command("/load resume-1"),
            Some(ParsedSlashCommand::Load(name)) if name == "resume-1"
        ));
        assert!(matches!(
            parse_slash_command("/sessions"),
            Some(ParsedSlashCommand::Sessions)
        ));
    }
}
