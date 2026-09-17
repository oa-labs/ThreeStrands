# ThreeStrands

ThreeStrands is an open-source, keyboard-first, local-first email client inspired
by some of the best email clients available. It is designed for people who
spend significant time in email and want to process important conversations
quickly without losing follow-ups, context, or control of their data.

The core experience combines:

- instant keyboard navigation and a discoverable command palette;
- focused inbox triage, local full-text search, and optimistic actions;
- reliable archive, read, star, label, and eventual follow-up workflows;
- offline access backed by a local SQLite cache; and
- a privacy-conscious desktop architecture that connects directly to mail
  providers without requiring a ThreeStrands backend.

ThreeStrands currently targets Gmail through a standalone Tauri desktop
application. OAuth credentials are kept in the operating-system keychain,
message data and search indexes stay in local SQLite, and a durable mutation
queue reconciles local actions with Gmail.

The detailed product definition, architecture, and delivery roadmap are in
[`PLAN.MD`](PLAN.MD).

## Status

Read/triage and the Phase 2 correspondence workflow are implemented, with live-account validation still required. Compose, reply, forward, local drafts, attachments, and a durable undo-send outbox are available. See [correspondence status and shortcuts](docs/phase-2-status.md).

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

Linux release packages are built in the repository's reproducible Ubuntu
devcontainer. It produces x86-64 Debian, RPM, and AppImage artifacts whether
the host is Linux or Apple Silicon. See the [Linux build guide](docs/linux-builds.md).

To connect Gmail, enable the Gmail API in a Google Cloud project, create an
OAuth client of type **Desktop app**, and launch the native client with its
client ID and client-secret value:

```sh
THREESTRANDS_GOOGLE_CLIENT_ID="1234.apps.googleusercontent.com" \
THREESTRANDS_GOOGLE_CLIENT_SECRET="value-from-downloaded-desktop-client-json" \
pnpm tauri dev
```

Release builds require these same two variables and fail at build time when
either is missing, so an installed build cannot silently lose Gmail access.

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

## Independent implementation

ThreeStrands uses its own name, visual identity, product language, and
implementation. It learns from established interaction patterns across the
email category without copying proprietary code, assets, or trademarks.
