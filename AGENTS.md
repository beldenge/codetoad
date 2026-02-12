# AGENTS.md

## Purpose

This repository is a Rust implementation of `grok-cli` behavior, focused on:
- streaming terminal UX (`ratatui` + `crossterm`)
- OpenAI-compatible chat completions
- agent tool loop for coding tasks

## Core Rules

- After finishing a set of changes, run:
  - `cargo clippy --all-targets --all-features -- -D warnings`
- Keep `README.md` up to date with real, current behavior.
- Keep this `AGENTS.md` up to date with real agent-facing guidance.
- Once clippy is clean and docs are updated, stage and commit the changes with a descriptive commit message, but do not push.

## Architecture Map

- `src/main.rs`
  - CLI entrypoint
  - interactive mode (`--ui inline|tui`), headless prompt mode, and git subcommand routing
- `src/cli.rs`
  - clap argument/subcommand definitions
- `src/settings.rs`
  - `~/.grok/user-settings.json` and `.grok/settings.json`
- `src/custom_instructions.rs`
  - loads `.grok/GROK.md` and `~/.grok/GROK.md`
- `src/grok_client.rs`
  - OpenAI-compatible HTTP client
  - Uses xAI Responses API for `api.x.ai` base URLs
  - Falls back to Chat Completions format for non-xAI compatible providers
  - SSE stream parsing for both formats
- `src/protocol.rs`
  - serde request/response DTOs
- `src/agent.rs`
  - ReAct-style loop and tool execution orchestration
- `src/tools.rs`
  - `view_file`, `create_file`, `str_replace_editor`, `bash`
- `src/tui.rs`
  - ratatui state, rendering, input handling, slash commands
- `src/inline_ui.rs`
  - scrollback-native inline interaction loop
  - default parity mode for terminal history behavior

## Slash Commands Implemented

- `/help`
- `/clear`
- `/models`
- `/models <name>`
- `/commit-and-push`
- `/exit`

## Development Notes

- Prefer adding behavior in existing modules instead of introducing parallel systems.
- Keep tool names and argument shapes compatible with OpenAI tool-calling conventions.
- Preserve current settings file schema unless migration logic is added in `src/settings.rs`.
- If UI behavior changes, update `README.md` and this file in the same change set.
