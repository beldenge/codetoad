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
