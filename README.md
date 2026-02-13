# grok-build (Rust)

`grok-build` is a Rust port of the core `grok-cli` coding-agent workflow.

Planned enhancements are tracked in `TODOS.md`.

It provides:
- xAI Responses API integration (non-deprecated path) with compatibility fallback for chat-completions-style providers
- ReAct-style tool loop (`view_file`, `create_file`, `str_replace_editor`, `bash`, `search`, `create_todo_list`, `update_todo_list`)
- Streaming terminal-native UI built with `crossterm`
- Slash commands compatible with the TypeScript app:
  - `/help`
  - `/clear`
  - `/models`
  - `/models <name>`
  - `/resume`
  - `/commit-and-push`
  - `/exit`
- Headless prompt mode and `git commit-and-push` subcommand

## Current Status

Implemented now:
- Interactive streaming UI
- Inline-first mode with native terminal scrollback parity
- Inline startup shows active project cwd
- Inline mode colored output (user prompt, assistant/tool prefixes, error/result tinting)
- Inline mode shows a `thinking...` status when waiting for first streamed tokens
- Inline mode shows a live spinner + elapsed seconds + approximate token count during thinking/tool execution and in completion summary
- Inline prompt supports rich key controls (history, cursor movement, word/line deletion)
- `Shift+Tab` toggles auto-edit mode, shown in the inline prompt status row
- Inline prompt supports slash-command suggestions with descriptions while typing `/...`
- Slash suggestions render as a vertical list under the prompt (no horizontal scrolling)
- `Up/Down` navigates command suggestions, `Tab` autocompletes, and `Enter` runs exact slash commands
- Sessions are auto-saved to `.grok/sessions/*.json` during interactive usage
- `/resume` opens an inline picker (same navigation style as model picker) to reload a saved session
- Resume restores model/history/cwd/todo state and auto-edit/confirmation session flags
- `/models` opens an interactive model picker (arrow keys + Enter/Tab)
- File-edit and bash operations (including direct commands) require confirmation (`y` once, `a` remember for session, `n`/`Esc` reject)
- File tools and shell working-directory changes are constrained to the active project root (canonical path boundary checks with symlink-aware ancestor resolution)
- Bash command execution includes sandbox preflight checks:
  - blocks out-of-root absolute/path-like arguments and redirection targets
  - blocks dynamic path expansion patterns (`~`, `$VAR/path`, `%VAR%\\path`, `$(...)`, backticks)
- Auto-edit mode bypasses confirmations for the current session
- Inline assistant output applies markdown-aware rendering (headings, lists, inline code, fenced code blocks) with lightweight syntax coloring
- Inline tool execution shows lifecycle timeline entries with per-tool durations and end-of-response tool summary
- Active generation can be cancelled with `Esc` or `Ctrl+C` without exiting the app
- Ctrl+C in prompt clears input first; pressing Ctrl+C again on empty input exits
- Native terminal scrollback remains visible after exit/Ctrl+C
- Tool-calling agent loop with max tool rounds
- Agent runtime now targets a provider trait boundary (`ModelClient`) to support fake/in-process clients in tests and future multi-provider backends
- Tool implementations are split by domain (`file_ops`, `bash_tool`, `search_tool`, `todos`) for cleaner extension paths
- Tool path/cwd state and todo state are session-scoped in the agent runtime (global statics removed)
- Responses API request/response conversion:
  - Converts chat-style message history to Responses `input` items
  - Flattens tool schema format for Responses API
  - Adds xAI Agent Tools search (`web_search`, `x_search`) when search mode is auto on Grok-4 models
  - If current model is not Grok-4, search-enabled requests are routed to `grok-4-latest` (or `GROK_SEARCH_MODEL`) while non-search requests keep the selected model
  - Parses Responses API output + streaming events back into chat/tool abstractions
- Settings loading/saving:
  - `~/.grok/user-settings.json`
  - `.grok/settings.json`
- Custom instruction loading:
  - `.grok/GROK.md` (project)
  - `~/.grok/GROK.md` (global fallback)
- Direct shell command passthrough in UI (`ls`, `pwd`, `cd`, `cat`, `mkdir`, `touch`, `echo`, `grep`, `find`, `cp`, `mv`, `rm`)
- `view_file` default preview is aligned to `grok-cli` (10 lines)
- Integration tests cover slash-command/help consistency and streamed tool-confirmation event ordering (`tests/command_flow.rs`)

Not yet implemented:
- MCP server integration
- Morph fast-apply tool
- Full TypeScript Ink UI parity details (command suggestion popup, rich markdown rendering)
- Full OS-level shell sandboxing for arbitrary bash commands beyond working-directory boundary enforcement

## Build

```bash
cargo build
```

## Run

Interactive:

```bash
cargo run -- --api-key <KEY>
```

Headless prompt:

```bash
cargo run -- --api-key <KEY> --prompt "show me all Rust files"
```

Git helper:

```bash
cargo run -- --api-key <KEY> git commit-and-push
```

With custom directory/base-url/model:

```bash
cargo run -- --directory D:\\dev\\gb\\grok-build --base-url https://api.x.ai/v1 --model grok-code-fast-1
```

## CLI Options

```text
--directory, -d
--api-key, -k
--base-url, -u
--model, -m
--prompt, -p
--max-tool-rounds
```

## Settings

User settings are stored in `~/.grok/user-settings.json` and include:
- `apiKey`
- `baseURL`
- `defaultModel`
- `models`

Project settings are stored in `.grok/settings.json` and include:
- `model`

## Quality Gate

After changes:

```bash
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```
