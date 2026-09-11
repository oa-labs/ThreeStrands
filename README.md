# Open-source high-performance email client

This repository is the starting point for an open-source, keyboard-first,
local-first email client inspired by the workflow benefits of Superhuman.

The product definition and proposed system design are in
[`docs/overview.md`](docs/overview.md).

## Status

Phase 1 (read and triage) is in progress.

The browser development build runs against deterministic fixtures. The native
Tauri build uses local SQLite through the same typed client interface. See
[`docs/phase-1.md`](docs/phase-1.md) for the implemented and remaining scope.

## Development

Prerequisites:

- Node.js 22 or newer
- Rust stable installed through `rustup`
- the [Tauri 2 platform prerequisites](https://v2.tauri.app/start/prerequisites/)

```sh
npm install
npm run dev       # browser preview with fixture mail
npm test
npm run build
npm run tauri dev # native app with local SQLite
```

## Product principles

1. Every common action is available from the keyboard.
2. The interface responds immediately, including on unreliable connections.
3. Users can see and control what is synchronized or sent to optional services.
4. Provider-specific behavior is isolated behind a tested capability contract.
5. The desktop client connects directly to mail providers and requires no
   project-operated backend.

## Naming and clean-room implementation

“Superhuman” is used in the planning document only to describe the existing
product category and benchmark. This project should use its own name, visual
identity, copy, and implementation. It should reproduce useful workflows, not
proprietary code, assets, or trademarks.
