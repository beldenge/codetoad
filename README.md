# grok-build (Rust)

`grok-build` is a Rust port of the core `grok-cli` coding-agent workflow.

It provides:
- Provider-aware model client behavior:
  - xAI base URLs (`api.x.ai`) use the Responses API (non-deprecated path)
  - non-xAI OpenAI-compatible base URLs use Chat Completions payloads
- ReAct-style tool loop (`view_file`, `create_file`, `str_replace_editor`, `bash`, `search`, `create_todo_list`, `update_todo_list`)
- Streaming terminal-native UI built with `crossterm`
- Multimodal image input support from file paths (drag/drop paths, markdown image links, and `file://` paths)
  - absolute image paths with spaces are supported (including files outside the current project directory)
  - image attachment parsing is separate from tool/file sandboxing; normal tools remain project-root constrained
- Slash commands compatible with the TypeScript app:
  - `/help`
  - `/clear`
  - `/models`
  - `/models <name>`
  - `/resume`
  - `/providers`
  - `/providers add`
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
- Inline input auto-detects image attachments from dropped paths/markdown image links and prints attachment confirmation before submit
- Headless `--prompt` mode also detects image attachments from file paths in prompt text
- Sessions are auto-saved to `.grok/sessions/*.json` during interactive usage
- `/resume` opens an inline picker (same navigation style as model picker) to reload a saved session
- Resume restores model/history/cwd/todo state and auto-edit/confirmation session flags
- `/models` opens an interactive model picker (arrow keys + Enter/Tab)
- `/providers` opens a provider picker and switches active provider in-session
- `/providers add` runs an inline wizard to add/update provider profiles
  - Provider ids entered in setup are normalized for stability (trimmed, lowercased, spaces/special chars -> `-`)
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
  - Maps user image attachments to Responses `input_image` parts
  - Flattens tool schema format for Responses API
  - Adds xAI Agent Tools search (`web_search`, `x_search`) when search mode is auto on Grok-4 models
  - If current model is not Grok-4, search-enabled requests are routed to `grok-4-latest` (or `GROK_SEARCH_MODEL`) while non-search requests keep the selected model
  - If image attachments are present and the selected xAI model is not image-capable, requests auto-route to `grok-4-latest` (or `GROK_IMAGE_MODEL`)
  - Parses Responses API output + streaming events back into chat/tool abstractions
- Settings loading/saving:
  - `~/.grok/user-settings.json`
  - `.grok/settings.json`
  - API key storage mode defaults to secure keychain; plaintext storage is opt-in only
  - Provider-aware default models based on configured base URL (`api.x.ai` vs OpenAI-compatible URLs)
  - Provider profiles + active provider selection persisted in user settings
- Custom instruction loading:
  - `.grok/GROK.md` (project)
  - `~/.grok/GROK.md` (global fallback)
- Direct shell command passthrough in UI (`ls`, `pwd`, `cd`, `cat`, `mkdir`, `touch`, `echo`, `grep`, `find`, `cp`, `mv`, `rm`)
- `view_file` default preview is aligned to `grok-cli` (10 lines)
- Integration tests cover slash-command/help consistency and streamed tool-confirmation event ordering (`tests/command_flow.rs`)

Not yet implemented:
- MCP server integration
- Morph fast-apply tool
- Clipboard screenshot paste to image attachment flow
- Full TypeScript Ink UI parity details (command suggestion popup, rich markdown rendering)
- Full OS-level shell sandboxing for arbitrary bash commands beyond working-directory boundary enforcement

## Build

```bash
cargo build
```

## Run

Interactive:

```bash
cargo run --
```

If no API key exists, the CLI starts a first-run setup wizard, prompts for an API key, and stores it in the configured storage mode (keychain by default).

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
--api-key-storage [keychain|plaintext]
--base-url, -u
--model, -m
--prompt, -p
--max-tool-rounds
```

## Provider Setup

Use this matrix for the fastest correct setup:

| Provider target | Base URL | Recommended key env var | Model example |
|---|---|---|---|
| xAI | `https://api.x.ai/v1` | `XAI_API_KEY` (or `GROK_API_KEY`) | `grok-code-fast-1` |
| OpenAI-compatible | provider endpoint | `OPENAI_API_KEY` (or `GROK_API_KEY`) | `gpt-4.1` |

Example (xAI):

```bash
export XAI_API_KEY=...
cargo run -- --base-url https://api.x.ai/v1 --model grok-code-fast-1
```

Optional xAI overrides:
- `GROK_SEARCH_MODEL`: model used for server-side search when current model is not Grok-4 (default `grok-4-latest`)
- `GROK_IMAGE_MODEL`: model used for image-attached prompts when current model is not image-capable (default `grok-4-latest`)

Example (OpenAI-compatible):

```bash
export OPENAI_API_KEY=...
export OPENAI_BASE_URL=https://api.openai.com/v1
cargo run -- --model gpt-4.1
```

## Settings

User settings are stored in `~/.grok/user-settings.json` and include:
- `apiKey`
- `apiKeyStorage` (`keychain` or `plaintext`)
- `baseURL`
- `defaultModel`
- `models`
- `providers`
- `activeProvider`

API key behavior:
- Environment variable lookup order is provider-aware and based on active base URL:
  - xAI: `GROK_API_KEY`, `XAI_API_KEY`, `OPENAI_API_KEY`
  - OpenAI-compatible: `GROK_API_KEY`, `OPENAI_API_KEY`, `XAI_API_KEY`
- Default mode is `keychain`, which stores/retrieves API keys from the OS credential store:
  - Windows Credential Manager
  - macOS Keychain
  - Linux Secret Service/libsecret
- In `keychain` mode, the CLI never writes API keys to plaintext settings.
- If keychain write/readback is unavailable, the entered key is kept only in-memory for the current process and you will be prompted again next launch.
- Set explicit mode with `--api-key-storage keychain` or `--api-key-storage plaintext`.
- Keychain storage is provider-scoped using separate credential accounts per provider id.

Credential precedence for runtime requests:
1. `--api-key`
2. Provider-aware environment variables (order above)
3. In-memory session key (when keychain persist failed in current run)
4. Keychain value (when `apiKeyStorage=keychain`)
5. Plaintext `apiKey` in `~/.grok/user-settings.json` (only when `apiKeyStorage=plaintext`)

Base URL behavior:
- `--base-url` (if passed) is used and saved
- else `GROK_BASE_URL`
- else `OPENAI_BASE_URL`
- else active provider `baseURL` from `~/.grok/user-settings.json`
- else default `https://api.x.ai/v1`

## Keychain Operations By OS

Set key into secure storage using the CLI:

```bash
cargo run -- --api-key-storage keychain --api-key <KEY>
```

Switch to plaintext mode (not recommended):

```bash
cargo run -- --api-key-storage plaintext --api-key <KEY>
```

Inspect/remove in OS store (advanced):
- Windows: Credential Manager -> Windows Credentials -> Generic Credentials (search for `grok-build` and account `provider_<id>`).
- macOS:
  - read: `security find-generic-password -s grok-build -a provider_xai -w`
  - delete: `security delete-generic-password -s grok-build -a provider_xai`
- Linux (Secret Service/libsecret):
  - read: `secret-tool lookup service grok-build username provider_xai`
  - clear: use your keyring UI (for example Seahorse) and remove the `grok-build` entry.

Project settings are stored in `.grok/settings.json` and include:
- `model`

## Quality Gate

After changes:

```bash
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

Optional secure-storage integration test (writes/reads a temporary provider key via OS keychain):

```bash
cargo test --features keychain-integration-tests --test settings_keychain
```
