# TODOs

Backlog for upcoming parity and platform enhancements.

## Planned

- [ ] Support image/screenshot input parity (drag/drop and paste)
  - Goal: accept image paths and pasted screenshots in interactive mode, then pass image content to the model using a supported multimodal payload shape.
  - UX target: similar to Codex/Claude-style CLI behavior, with visible attachment confirmation before submit.
  - Scope notes: include Windows, macOS, and Linux terminal behaviors.

- [x] Add session persistence for saving and resuming tool loops across runs
  - Goal: allow users to pause and resume coding sessions, preserving context and partial tool executions.
  - Implemented via automatic session saves plus `/resume` interactive picker.
  - Scope notes: session files are stored in `.grok/sessions/*.json` and restore model, message history, session cwd, todo state, and auto-edit/confirmation flags.

- [x] Store API keys in secure OS keychain/credential store (cross-platform)
  - Goal: avoid plaintext-only API key storage by default.
  - Implemented secure keychain mode by default (`apiKeyStorage=keychain`) with plaintext fallback when keychain write is unavailable.
  - Platforms:
    - Windows Credential Manager
    - macOS Keychain
    - Linux Secret Service/libsecret
  - UX: explicit opt-in/opt-out via `--api-key-storage keychain|plaintext`.

- [ ] Add provider/model compatibility beyond Grok-only usage
  - Goal: support additional provider-compatible models while preserving current coding-agent UX and tool loop behavior.
  - Scope notes: normalize streaming/tool responses across provider APIs and add per-model capability handling.

- [x] Enforce project-directory sandbox for tool operations
  - Goal: prevent agent-driven file and shell operations from affecting paths outside the active project directory.
  - Scope notes: canonicalize/resolve paths (including symlinks) and reject out-of-root access attempts for file tools and `cd`/bash execution context changes.

- [x] Harden shell sandboxing beyond working-directory boundary checks
  - Goal: reduce risk from arbitrary shell commands that can still reference absolute/out-of-root paths while running inside an in-root cwd.
  - Implemented command preflight policy:
    - validates path-like command arguments and redirection targets against project-root boundaries
    - blocks dynamic path expansion patterns (`~`, `$VAR/path`, `%VAR%\\path`, `$(...)`, backticks)
    - preserves existing `cd` handling and reports explicit sandbox-policy errors
