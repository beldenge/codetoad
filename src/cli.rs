use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "codetoad",
    version,
    about = "A friendly code toad CLI with terminal-native streaming UI"
)]
pub struct Cli {
    #[arg(short = 'd', long = "directory", global = true)]
    pub directory: Option<PathBuf>,

    #[arg(short = 'k', long = "api-key", global = true)]
    pub api_key: Option<String>,

    #[arg(long = "api-key-storage", value_enum, global = true)]
    pub api_key_storage: Option<ApiKeyStorageArg>,

    #[arg(short = 'u', long = "base-url", global = true)]
    pub base_url: Option<String>,

    #[arg(short = 'm', long = "model", global = true)]
    pub model: Option<String>,

    #[arg(short = 'p', long = "prompt", global = true)]
    pub prompt: Option<String>,

    #[arg(long = "max-tool-rounds", default_value_t = 400, global = true)]
    pub max_tool_rounds: usize,

    #[command(subcommand)]
    pub command: Option<Commands>,

    #[arg()]
    pub message: Vec<String>,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    Git {
        #[command(subcommand)]
        command: GitCommands,
    },
}

#[derive(Debug, Subcommand)]
pub enum GitCommands {
    CommitAndPush,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ApiKeyStorageArg {
    Keychain,
    Plaintext,
}

#[cfg(test)]
mod tests {
    use super::{ApiKeyStorageArg, Cli, Commands, GitCommands};
    use clap::Parser;

    #[test]
    fn parses_global_flags_and_message_arguments() {
        let cli = Cli::parse_from([
            "codetoad",
            "--directory",
            "repo",
            "--api-key",
            "key-123",
            "--base-url",
            "https://api.openai.com/v1",
            "--model",
            "gpt-4.1",
            "--prompt",
            "hello",
            "--max-tool-rounds",
            "12",
            "write",
            "tests",
        ]);

        assert_eq!(cli.directory.as_deref(), Some(std::path::Path::new("repo")));
        assert_eq!(cli.api_key.as_deref(), Some("key-123"));
        assert_eq!(cli.base_url.as_deref(), Some("https://api.openai.com/v1"));
        assert_eq!(cli.model.as_deref(), Some("gpt-4.1"));
        assert_eq!(cli.prompt.as_deref(), Some("hello"));
        assert_eq!(cli.max_tool_rounds, 12);
        assert_eq!(cli.message, vec!["write".to_string(), "tests".to_string()]);
    }

    #[test]
    fn parses_git_commit_and_push_subcommand() {
        let cli = Cli::parse_from(["codetoad", "git", "commit-and-push"]);

        match cli.command {
            Some(Commands::Git { command }) => {
                assert!(matches!(command, GitCommands::CommitAndPush));
            }
            _ => panic!("expected git commit-and-push command"),
        }
    }

    #[test]
    fn parses_api_key_storage_value_enum() {
        let keychain = Cli::parse_from(["codetoad", "--api-key-storage", "keychain"]);
        assert!(matches!(
            keychain.api_key_storage,
            Some(ApiKeyStorageArg::Keychain)
        ));

        let plaintext = Cli::parse_from(["codetoad", "--api-key-storage", "plaintext"]);
        assert!(matches!(
            plaintext.api_key_storage,
            Some(ApiKeyStorageArg::Plaintext)
        ));
    }
}
