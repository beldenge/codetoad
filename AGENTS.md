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
- `src/lib.rs`
  - shared library surface so integration tests can exercise agent/tool flows
- `src/app_context.rs`
  - shared app/runtime context (cwd, agent handle, settings handle, runtime flags)
  - central auto-edit flag synchronization between UI runtime state and agent execution state
- `src/cli.rs`
  - clap argument/subcommand definitions
- `src/slash_commands.rs`
  - canonical slash-command metadata, parsing, and suggestion/help helpers
- `src/inline_prompt.rs`
  - prompt input editor, key handling, slash suggestion panel, and model picker UI
- `src/inline_markdown.rs`
  - markdown-aware streaming renderer and lightweight code syntax highlighting
- `src/inline_feedback.rs`
  - shared inline output helpers (banner/tips, tool result rendering, confirmation prompt)
  - tool label/target formatting used by inline orchestration
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
- `src/model_client.rs`
  - `ModelClient` trait boundary used by `Agent` for provider abstraction
  - stream callback type alias shared by real and fake model clients
- `src/responses_adapter.rs`
  - xAI Responses API adapter and normalization logic into OpenAI-compatible structures
  - responses SSE event parsing and tool-call/content extraction
  - model capability helpers for server-side tools (`web_search`, `x_search`)
- `src/protocol.rs`
  - serde request/response DTOs
- `src/confirmation.rs`
  - shared confirmation operation enum (`File`, `Bash`) used across agent/UI/tool metadata
- `src/tool_catalog.rs`
  - canonical tool metadata (schemas, confirmation mapping, UI display labels)
- `src/tool_context.rs`
  - per-session tool context object (project root + session working directory)
  - central path resolution and `cd` state used by file/shell/search tools
  - enforces canonical project-root containment for file paths and working-directory changes
- `src/git_ops.rs`
  - shared `commit-and-push` workflow used by headless and inline entrypoints
- `src/agent.rs`
  - ReAct-style loop and tool execution orchestration
  - session-scoped confirmation routing for file/bash operations
- `src/agent_policy.rs`
  - agent policy/heuristics (system prompt, search-mode routing, token estimation)
- `src/agent_stream.rs`
  - streaming delta merge and partial tool-call assembly helpers
  - stream merge behavior tests for duplicate/snapshot/incremental chunk handling
- `src/tools/mod.rs`
  - tool dispatch + shared `ToolResult` shape
- `src/tools/file_ops.rs`
  - `view_file`, `create_file`, `str_replace_editor`
- `src/tools/bash_tool.rs`
  - `bash` execution + `cd` handling under project-root constraints
- `src/tools/todos.rs`
  - `create_todo_list`, `update_todo_list`
  - session-scoped in-memory todo store (no global static state)
- `src/tools/search_tool.rs`
  - `search` tool execution (text/file/both modes via ripgrep)
  - shared result ranking and formatting for search output
- `src/inline_ui.rs`
  - scrollback-native inline interaction loop orchestration
  - primary parity mode for terminal history behavior
  - tool lifecycle timeline (start/result + durations + response summary)
  - active generation cancellation via `Esc`/`Ctrl+C`
  - live status telemetry with approximate token counts and auto-edit mode indicator
  - auto-edit bypass + confirmation routing integration

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
