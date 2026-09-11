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
pnpm install
pnpm dev       # browser preview with fixture mail
pnpm test
pnpm test:e2e  # first run: pnpm exec playwright install chromium
pnpm build
pnpm tauri dev # native app with local SQLite
```

To connect Gmail, enable the Gmail API in a Google Cloud project, create an
OAuth client of type **Desktop app**, and launch the native client with its
client ID and client-secret value:

```sh
DISPATCH_GOOGLE_CLIENT_ID="1234.apps.googleusercontent.com" \
DISPATCH_GOOGLE_CLIENT_SECRET="value-from-downloaded-desktop-client-json" \
pnpm tauri dev
```

Google labels this value a client secret, but installed desktop applications
are public clients and cannot keep it confidential. It is used only as a
required token-endpoint parameter; OAuth tokens are stored in the
operating-system keychain. Never commit the credential value.

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
