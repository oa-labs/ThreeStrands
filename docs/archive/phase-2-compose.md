# Phase 2 implementation plan: compose and correspondence

Status: implemented correspondence scope; see [verification status and remaining release checks](phase-2-status.md).

This delivers the four correspondence items from `PLAN.MD`: composer and local drafts; safe sending and undo; forwarding and attachments; keyboard integration. The first complete milestone is: open an email, write a reply offline, restart without losing saved work, reconnect, and send with an undo window.

## Scope and decisions

- One connected Gmail account; use its verified primary address as the sender. Sending aliases are a later extension.
- Start with an accessible plain-text editor and correctly quoted replies. Rich-text editing is a separate enhancement; incoming HTML rendering remains supported.
- SQLite is authoritative for local drafts. Draft synchronization with Gmail and editing Gmail-created drafts are deferred and must be identified as such in the UI/documentation.
- Default undo delay: 10 seconds. No provider send occurs during that delay.
- Preserve the existing read/triage workflow, themes, window sizing, and inbox resizing.
- Implement keyboard hooks alongside each feature; complete the full keyboard acceptance pass in item 4.
- Continue Phase 1 real-account sync validation and dogfooding while building these features.

## Current code and prerequisites

`src/domain.ts`, `src/data/client.ts`, and `src/data/demoClient.ts` define the UI-facing contract. `src-tauri/src/models.rs`, `db.rs`, `lib.rs`, and `gmail.rs` provide native models, persistence, commands, and provider access. Extend both native and demo implementations together.

The database currently initializes tables with `CREATE TABLE IF NOT EXISTS`; introduce ordered, transactional migrations before altering existing message tables. Test upgrades from an existing installation, preserving mail, FTS content, and queued mutations.

The message model currently drops reply headers and separates neither To nor Cc. Extend `mime.rs`, native/TypeScript models, database storage, and import paths to retain RFC Message-ID, Reply-To, To, Cc, and References. Refresh source metadata on demand for previously cached messages; offline replies with incomplete addressing/threading metadata must explain what is missing rather than guess.

## 1. Composer and durable local drafts

### Deliverables

- A composer with To, expandable Cc/Bcc, subject, body, sender identity, and visible save status.
- New Message, Reply, and Reply All actions, plus a Drafts view with resume and discard.
- Closing the composer saves it; discarding is a separate explicit action. Returning to a conversation reopens its existing active reply draft instead of accidentally creating duplicates.
- Reply uses Reply-To when present, otherwise From. Reply All deduplicates recipients, excludes the user's own identity, preserves To/Cc roles, and never guesses hidden Bcc recipients. Handle replies to the user's own sent messages.
- Quoted body with attribution; preserve Unicode and line breaks. Use an address parser rather than the current comma-splitting helper for display-name addresses.

### Implementation

Add `drafts` with account ID, mode, source message/thread IDs, structured recipient lists, subject, body, revision, and timestamps. Reserve attachment relationships and store provider-thread/RFC reply metadata separately from editable content.

Expose typed create/get/list/save/discard draft operations through `MailClient` and Tauri. Use revision checks and serialized saves so late responses cannot overwrite newer edits. Debounce autosave around 300 ms, flush on composer close and normal app exit, and show “Saved” only after SQLite acknowledges the write. Keep unsaved text and a retry action on write failure. Abrupt crashes may lose only the explicitly unsaved interval; do not claim otherwise.

Add focused components such as `Composer.tsx`, `RecipientInput.tsx`, and `DraftList.tsx`; keep lifecycle logic outside the growing `App.tsx`.

### Acceptance

Offline creation/editing works. Saved drafts survive native restart. Rapid edits and navigation do not lose or reorder saves. Recipient parsing handles quoted names, Unicode, duplicates, and invalid addresses. Both themes, small supported windows, focus management, and screen-reader save status work.

## 2. Safe sending, durable outbox, and undo

### Deliverables

- Send validates recipients, sender, draft revision, and attachment readiness in Rust as well as the UI.
- Clicking Send atomically freezes a draft snapshot into a durable outbox item and starts the 10-second cancellation window.
- An Outbox view shows waiting, offline, sending, sent, failed, and uncertain outcomes. Undo restores the editable draft. Failed messages remain recoverable.

### Implementation

Create a separate `outbox_messages` table: stable local operation ID, account ID, draft revision, immutable MIME snapshot/content reference, RFC Message-ID, provider message/thread IDs when available, undo deadline, attempt details, timestamps, and error status. A uniqueness constraint prevents repeated clicks from queuing the same draft revision twice.

Use explicit transitions:

`undo_pending -> ready -> sending -> sent`

`undo_pending/ready -> canceled`

`sending -> failed | uncertain`

Atomic compare-and-set operations decide whether cancel or send wins. After claiming `sending`, cancellation cannot promise recall. Definite pre-delivery failures may return to a retryable state; uncertain delivery never retries automatically.

Add a Rust send service with a single serialized worker per account, a timer independent of the current 15–300 second mailbox poll, and startup/resume recovery. Do not copy the current behavior that resets every interrupted running mutation to pending. Do not route send requests through a generic blind retry helper.

Generate MIME with a maintained Rust MIME builder, including injection-safe headers, a stable RFC Message-ID, and reply references. Send through the Gmail provider contract and persist the provider result before announcing success. Reconcile sent messages into the local reader/search without duplicates.

On timeout or a crash after dispatch, mark the operation uncertain and reconcile against sent mail using the persisted identity and message metadata. A local UUID or RFC Message-ID is not a provider idempotency guarantee. An empty search result is not proof that delivery failed; keep unresolved outcomes visible and require explicit user action before risking another send.

Normal quit flushes drafts and offers cancellation or briefly defers exit during the undo window, as required by `PLAN.MD`. Interrupted undo windows receive a fresh cancellable grace period on restart before dispatch; ready offline items resume when connected. Disconnect/account changes pause delivery and never send a previous account's queued mail through another identity.

### Acceptance

Fake-clock tests prove no send before the deadline, cancellation wins correctly, repeated Send queues once, and offline items survive restart. Fault injection covers termination before dispatch, after provider acceptance, and before local commit. Uncertain results never trigger automatic duplicate sends. Token expiry, rate limits, permanent rejection, and disk-write failure retain actionable state.

## 3. Forwarding and attachments

### Deliverables

- Forward opens a new-addressed draft with subject, attribution, and quoted content; it does not inherit reply-thread headers.
- A native file picker adds attachments with filename, size, preparation state, remove, and retry controls.
- Incoming attachment metadata and on-demand retrieval support forwarding original attachments. Show the included files explicitly; unavailable files block sending until retrieved or removed.

### Implementation

Extend MIME normalization to collect attachment metadata independently of body selection. Add attachment records with provider locator, app-owned content locator, size, MIME type, and cache state. Copy selected files into managed app storage before marking them ready, so moving the original file cannot break a saved draft.

Use native file operations with narrowly scoped capabilities and opaque attachment IDs at the UI boundary. Do not expose unrestricted file reads or put large file bytes into React state. Validate names and paths, account for MIME encoding overhead, verify Gmail's current size limits at implementation time, and reject oversized messages before queueing.

Build multipart MIME in Rust; preserve non-ASCII filenames and content bytes. Retain file references while any draft/outbox item needs them, then clean orphaned files safely. Discard/cancel must not delete content still used by another item. Remote images remain governed by the existing image policy.

### Acceptance

Compose and forward with multiple files; restart after file selection; delete/move the original file; remove/re-add files; test zero-byte, Unicode-named, unavailable, and oversized attachments. Verify MIME round trips and received attachment bytes with a controlled Gmail test account. Never send test mail to real correspondents.

## 4. Keyboard and command integration

### Deliverables

All buttons and palette entries invoke shared commands with enabled states and focus scopes:

| Action | Shortcut |
| --- | --- |
| New Message | `c` |
| Reply | `r` |
| Reply All | `a` |
| Forward | `f` |
| Refresh mail | `Shift+r` (moves from current `r`) |
| Send from composer | `Cmd/Ctrl+Enter` |
| Save and close composer | `Escape` |

Drafts, Outbox, Attach, and Undo Send are also discoverable through the palette. Update shortcut hints and help to reflect the refresh change.

Extend `commands.ts` to support modifier-aware matching and a Compose group. Suppress inbox navigation/triage for the entire active composer scope, including focused buttons and recipient chips, and while IME composition is active. Escape dismisses a nested popup before closing the composer. Restore focus to the invoking control on close. Keep an accessible route to Undo during its window.

### Acceptance

A keyboard-only test covers new draft, reply/all, forward, recipients, attachment selection, send, and undo. Typing shortcut letters never archives or switches conversations. `r` and `Shift+r` remain distinct; repeated send shortcuts are harmless. Test focus trapping/restoration, screen-reader announcements, and both themes.

## Delivery sequence and review gates

1. **Schema and contracts:** migrations, identity/reply metadata, draft APIs, demo support, and provider fixtures.
2. **Offline composer:** compose/reply/all, Drafts view, autosave, close/restart behavior, and initial commands. Usable without sending.
3. **Send service:** immutable outbox, MIME construction, undo timer, recovery and fault-injection tests. Keep real send disabled until these tests pass.
4. **Sending UI:** Outbox, failures/uncertainty, quit handling, and controlled native Gmail verification.
5. **Forward and attachments:** caching, MIME multipart, retrieval, cleanup, and controlled attachment delivery verification.
6. **Keyboard and release acceptance:** full command coverage, accessibility, themes/layout, regression suite, and updated Phase 2 status documentation.

Run relevant Rust unit/integration/provider tests, frontend tests, production build, and Playwright flows at each gate. Browser fixtures cannot validate native SQLite durability, keychain behavior, app shutdown, or actual Gmail threading: include explicit native restart/offline tests and controlled real-account verification before declaring correspondence ready.

## Provider references

- [Gmail sending guide](https://developers.google.com/workspace/gmail/api/guides/sending): MIME construction, base64URL payloads, and attachments.
- [Gmail threading guide](https://developers.google.com/workspace/gmail/api/guides/threads): provider thread ID and reply-header/subject requirements.

These references inform the provider integration. Verify endpoint permissions, upload limits, and reconciliation behavior during implementation; do not infer exactly-once sending from the existing mutation UUIDs.
