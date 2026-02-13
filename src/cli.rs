use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "grok-build",
    version,
    about = "A Rust-based Grok coding CLI with terminal-native streaming UI"
)]
pub struct Cli {
    #[arg(short = 'd', long = "directory", global = true)]
    pub directory: Option<PathBuf>,

    #[arg(short = 'k', long = "api-key", global = true)]
    pub api_key: Option<String>,

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
