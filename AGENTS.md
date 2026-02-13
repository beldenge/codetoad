# AGENTS.md

## Purpose

This repository is a Rust implementation of `grok-cli` behavior, focused on:
- streaming terminal-native UX (`crossterm`)
- OpenAI-compatible chat completions
- agent tool loop for coding tasks

## Core Rules

- After finishing a set of changes, run:
  - `cargo clippy --all-targets --all-features -- -D warnings`
- Never use `#[allow(...)]` attributes to bypass clippy warnings; fix the underlying issue instead.
- Keep `README.md` up to date with real, current behavior.
- Keep this `AGENTS.md` up to date with real agent-facing guidance.
- Once clippy is clean and docs are updated, stage and commit the changes with a descriptive commit message, but do not push.

## Architecture Map

- `src/main.rs`
  - CLI entrypoint
  - inline interactive mode (default), headless prompt mode, and git subcommand routing
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
  - Enables xAI Agent Tools search (`web_search`, `x_search`) in auto search mode for Grok-4 models
  - Routes only search-enabled requests to `grok-4-latest` (or `GROK_SEARCH_MODEL`) when current model is not Grok-4
  - SSE stream parsing for both formats
- `src/protocol.rs`
  - serde request/response DTOs
- `src/git_ops.rs`
  - shared `commit-and-push` workflow used by headless and inline entrypoints
- `src/agent.rs`
  - ReAct-style loop and tool execution orchestration
  - session-scoped confirmation routing for file/bash operations
- `src/tools.rs`
  - `view_file`, `create_file`, `str_replace_editor`, `bash`, `search`
  - `create_todo_list`, `update_todo_list`
- `src/inline_ui.rs`
  - scrollback-native inline interaction loop
  - primary parity mode for terminal history behavior
  - vertical slash-command suggestion panel under prompt
  - markdown-aware streaming renderer with lightweight syntax coloring
  - tool lifecycle timeline (start/result + durations + response summary)
  - active generation cancellation via `Esc`/`Ctrl+C`
  - live status telemetry with approximate token counts and auto-edit mode indicator
  - interactive tool confirmation prompts (`y/a/n`) and auto-edit bypass

## UI Direction

- Runtime uses one inline UI mode for terminal-native behavior and stable scrollback.

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
