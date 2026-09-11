# Phase 1 — read and triage

This document tracks implementation against the Phase 1 exit criteria in
[`overview.md`](overview.md).

## Implemented

- Tauri 2 desktop shell and restricted window capabilities.
- React read-and-triage interface with inbox, thread reader, and diagnostics.
- SQLite schema for threads, messages, sync state, and durable mutations.
- SQLite FTS5 search over thread metadata and cached body text.
- Typed Tauri commands for list, detail, search, mutation, and sync status.
- Optimistic archive, read/unread, and star actions.
- Durable mutation records with idempotent UUIDs.
- Shared command registry for buttons, keyboard shortcuts, and command palette.
- Keyboard navigation with `j`, `k`, `e`, `u`, `s`, `/`, and `Cmd/Ctrl+K`.
- Undo affordance for archive.
- HTML allowlist sanitization with remote images, forms, scripts, inline styles,
  and SVG blocked.
- Browser development mode backed by deterministic fixtures.
- Sync diagnostics for cursor, latest success, pending mutations, and errors.

## Remaining before Phase 1 exit

- Google OAuth Authorization Code with PKCE.
- OS-keychain token persistence.
- Gmail REST adapter and MIME normalization.
- Initial Gmail synchronization and history cursor recovery.
- Adaptive polling, startup/resume catch-up, and provider quota handling.
- Durable mutation delivery and reconciliation against Gmail.
- Label mutations and label management.
- Crash reporting policy and opt-in implementation.
- Automated accessibility, end-to-end, provider contract, and fault-injection
  coverage.
- Two weeks of dogfooding without a data-loss incident.

The browser preview intentionally uses fixtures. The Tauri build reads from the
local SQLite database. The current `sync_account` command updates diagnostics
only; it does not claim to contact Gmail.
