# Phase 1 — read and triage

This document tracks implementation against the Phase 1 exit criteria in
[`PLAN.MD`](../PLAN.MD).

## Implemented

- Tauri 2 desktop shell and restricted window capabilities.
- React read-and-triage interface with inbox, thread reader, and diagnostics.
- SQLite schema for threads, messages, sync state, and durable mutations.
- SQLite FTS5 search over thread metadata and cached body text.
- Typed Tauri commands for list, detail, search, mutation, and sync status.
- Optimistic archive, trash, read/unread, and star actions, plus batch
  variants applied to a checked set of conversations.
- Durable mutation records with idempotent UUIDs.
- Shared command registry for buttons, keyboard shortcuts, and command palette.
- Keyboard navigation with `j`, `k`, `e`, `Shift+e`, `u`, `s`, `#`, `x`, `/`, and
  `Cmd/Ctrl+K`.
- Undo affordance for archive, trash, read/unread, star, and label changes,
  including multi-conversation batch actions.
- HTML allowlist sanitization with remote images, forms, scripts, inline styles,
  and SVG blocked.
- Browser development mode backed by deterministic fixtures.
- Sync diagnostics for cursor, latest success, pending mutations, and errors.
- Google installed-app OAuth Authorization Code flow with PKCE, a random
  loopback port, state validation, and browser handoff. The non-secret client
  ID and Google-required Desktop client-secret value are configured with
  `DISPATCH_GOOGLE_CLIENT_ID` and `DISPATCH_GOOGLE_CLIENT_SECRET`.
- OAuth access and refresh tokens stored only in the operating-system
  credential store (`app.dispatch.mail`), never SQLite or the webview.
- Gmail REST integration with bounded `Retry-After`/exponential backoff,
  recognition of quota failures returned as either HTTP 429 or 403, paced
  high-cost thread retrieval, base64url MIME decoding, nested multipart
  plain/HTML selection, attachment exclusion, and normalized
  Subject/From/To/Date fields.
- Initial paginated synchronization and incremental Gmail History
  synchronization. Expired/invalid history cursors trigger a complete import;
  the import cursor is captured before listing and history is replayed
  afterwards to close the snapshot race.
- Durable Gmail delivery for archive, trash, read/unread, star, and arbitrary
  label changes (trash/untrash move the thread across `INBOX`/`TRASH`
  together, matching Gmail's own trash semantics). Identical pending changes
  are coalesced; interrupted `running` records return to `pending` on
  startup; acknowledged and rejected changes are recorded separately.
- Typed commands for Google connection status/connect/disconnect, manual sync,
  label list/create/rename/delete, and label thread mutations. The label
  manager only exposes user-created labels for per-thread/per-batch toggling;
  Gmail's system labels (`INBOX`, `TRASH`, `SPAM`, `UNREAD`, `STARRED`,
  `CATEGORY_*`, …) stay behind the dedicated archive/trash/star/read actions.
- Adaptive background polling while connected, plus catch-up on Tauri startup
  and desktop resume events.
- Native tests for nested MIME normalization and malformed provider data,
  repeated mutation idempotence, and recovery of an interrupted mutation.
- Label creation, rename, deletion, and per-thread application UI.
- Opt-in crash reporting with local retention, address/URL redaction, optional
  build-time reporting endpoint, and a documented data/retention policy.
- Automated accessibility checks, a reusable provider contract suite, native
  fault-injection tests, and Playwright keyboard read/triage workflows.

## Remaining before Phase 1 exit

- Validate OAuth and synchronization against a Google test account. Automated
  repository tests use provider contracts and mocks so CI never needs mailbox
  credentials.
- Two weeks of dogfooding without a data-loss incident.

The browser preview intentionally continues to use fixtures. The Tauri build
reads from local SQLite and contacts Gmail only after explicit OAuth consent.
Create a Google OAuth client of type **Desktop app**, enable the Gmail API, and
provide its public client ID when launching/building:

```sh
DISPATCH_GOOGLE_CLIENT_ID="1234.apps.googleusercontent.com" \
DISPATCH_GOOGLE_CLIENT_SECRET="value-from-downloaded-desktop-client-json" \
pnpm tauri dev
```

Google calls the second value a client secret, but installed desktop
applications are public clients and cannot keep it confidential. It is used
only as a required token-endpoint parameter and must not be committed. OAuth
requires the system browser and an available loopback port. OS credential-store
availability depends on a logged-in desktop keychain service. Background
polling runs only while the process is alive; the app performs catch-up rather
than claiming OS-level background delivery while suspended or terminated.
