# grok-build (Rust)

`grok-build` is a Rust port of the core `grok-cli` coding-agent workflow.

It provides:
- xAI Responses API integration (non-deprecated path) with compatibility fallback for chat-completions-style providers
- ReAct-style tool loop (`view_file`, `create_file`, `str_replace_editor`, `bash`)
- Streaming terminal-native UI built with `crossterm`
- Slash commands compatible with the TypeScript app:
  - `/help`
  - `/clear`
  - `/models`
  - `/models <name>`
  - `/commit-and-push`
  - `/exit`
- Headless prompt mode and `git commit-and-push` subcommand

## Current Status

Implemented now:
- Interactive streaming UI
- Inline-first mode with native terminal scrollback parity
- Inline mode colored output (user prompt, assistant/tool prefixes, error/result tinting)
- Inline mode shows a `thinking...` status when waiting for first streamed tokens
- Inline mode shows a live spinner + elapsed seconds during thinking/tool execution and a completion duration summary
- Inline prompt supports rich key controls (history, cursor movement, word/line deletion)
- Inline prompt supports slash-command suggestions with descriptions while typing `/...`
- `Up/Down` navigates command suggestions and `Tab/Enter` accepts the selected command
- `/models` opens an interactive model picker (arrow keys + Enter/Tab)
- Inline assistant output applies markdown-aware rendering (headings, lists, inline code, fenced code blocks) with lightweight syntax coloring
- Inline tool execution shows lifecycle timeline entries with per-tool durations and end-of-response tool summary
- Ctrl+C in prompt clears input first; pressing Ctrl+C again on empty input exits
- Native terminal scrollback remains visible after exit/Ctrl+C
- Tool-calling agent loop with max tool rounds
- Responses API request/response conversion:
  - Converts chat-style message history to Responses `input` items
  - Flattens tool schema format for Responses API
  - Parses Responses API output + streaming events back into chat/tool abstractions
- Settings loading/saving:
  - `~/.grok/user-settings.json`
  - `.grok/settings.json`
- Custom instruction loading:
  - `.grok/GROK.md` (project)
  - `~/.grok/GROK.md` (global fallback)
- Direct shell command passthrough in UI (`ls`, `pwd`, `cd`, `cat`, `mkdir`, `touch`, `echo`, `grep`, `find`, `cp`, `mv`, `rm`)

Not yet implemented:
- MCP server integration
- Morph fast-apply tool
- xAI Agent Tools web-search integration (legacy live-search parameters are intentionally disabled)
- Full TypeScript Ink UI parity details (command suggestion popup, rich markdown rendering)

## Build

```bash
cargo build
```

## Run

Interactive:

```bash
cargo run -- --api-key <KEY>
```

Choose UI mode explicitly:

```bash
cargo run -- --api-key <KEY> --ui inline
```

Note: `--ui tui` is currently deprecated and falls back to inline mode.

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
--ui <inline|tui> (tui currently deprecated)
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
```
