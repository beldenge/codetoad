# grok-build (Rust)

`grok-build` is a Rust port of the core `grok-cli` coding-agent workflow.

It provides:
- xAI Responses API integration (non-deprecated path) with compatibility fallback for chat-completions-style providers
- ReAct-style tool loop (`view_file`, `create_file`, `str_replace_editor`, `bash`)
- Streaming terminal UI built with `ratatui` + `crossterm`
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
- Ink-specific visual parity details from the TypeScript UI

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
```
