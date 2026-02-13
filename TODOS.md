# TODOs

Backlog for upcoming parity and platform enhancements.

## Planned

- [ ] Support image/screenshot input parity (drag/drop and paste)
  - Goal: accept image paths and pasted screenshots in interactive mode, then pass image content to the model using a supported multimodal payload shape.
  - UX target: similar to Codex/Claude-style CLI behavior, with visible attachment confirmation before submit.
  - Scope notes: include Windows, macOS, and Linux terminal behaviors.

- [ ] Add session persistence for saving and resuming tool loops across runs
  - Goal: allow users to pause and resume coding sessions, preserving context and partial tool executions.
  - UX target: simple commands like 'save' and 'load' with optional session names.
  - Scope notes: store state in a local file (e.g., JSON) and handle recovery on startup.

- [ ] Store API keys in secure OS keychain/credential store (cross-platform)
  - Goal: avoid plaintext-only API key storage by default.
  - Platforms:
    - Windows Credential Manager
    - macOS Keychain
    - Linux Secret Service (DBus/libsecret) with a clear fallback path when unavailable
  - UX target: seamless first-run save and subsequent retrieval, with explicit user opt-in/opt-out.

- [ ] Add provider/model compatibility beyond Grok-only usage
  - Goal: support additional provider-compatible models while preserving current coding-agent UX and tool loop behavior.
  - Scope notes: normalize streaming/tool responses across provider APIs and add per-model capability handling.

- [x] Enforce project-directory sandbox for tool operations
  - Goal: prevent agent-driven file and shell operations from affecting paths outside the active project directory.
  - Scope notes: canonicalize/resolve paths (including symlinks) and reject out-of-root access attempts for file tools and `cd`/bash execution context changes.

- [ ] Harden shell sandboxing beyond working-directory boundary checks
  - Goal: reduce risk from arbitrary shell commands that can still reference absolute/out-of-root paths while running inside an in-root cwd.
  - Scope notes: evaluate command policy enforcement and/or OS-level sandbox strategies for Windows/macOS/Linux.
