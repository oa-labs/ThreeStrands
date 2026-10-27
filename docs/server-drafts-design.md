# Server drafts design

Status: accepted (2026-10-20). Phases 1–3 (Gmail push, Gmail pull, and
retiring Replicated Sync draft content) are implemented in 0.149.0, and
phase 4 (IMAP) in 0.150.0. Only the optional metadata entity (phase 5)
remains. Replaces the
draft content sync that shipped in 0.147.0 through Replicated Sync.

## Why change

0.147.0 syncs compose drafts between ThreeStrands devices as Replicated Sync
entities (`EntityType::Draft`, in a `draft_sync.rs` module since removed).
That works only between ThreeStrands devices and has real limits:

- Attachment files stay on the device that added them. Only their metadata
  syncs, so a draft opened elsewhere must have its files attached again.
- Drafts are capped at 512 KB each and 8 MB in total
  (`MAX_SYNCED_DRAFT_BYTES`, `MAX_SYNCED_DRAFTS_TOTAL_BYTES`).
- Drafts never appear in other mail apps, such as Gmail on a phone or Apple
  Mail reading the same IMAP account.
- ThreeStrands maintains its own content addressing, conflict handling, and
  delete-everywhere rules for data that email already knows how to store.

A draft is an email, and every mail account already has a place for drafts
that syncs everywhere. Replicated Sync stays the right home for data only
ThreeStrands understands: tasks, snippets, settings, and draft metadata that
email can't hold.

## Product contract

- A draft saved in ThreeStrands appears in the account's drafts on the mail
  server: Gmail's **Drafts**, or the IMAP `\Drafts` mailbox.
- Typing stays instant and works offline. Every edit is saved locally first and
  reaches the server after a short pause, when the composer closes, and before
  the app quits.
- The save status tells the truth:
  - **Saving…** while the revision waits for the push delay.
  - **Draft saved** only once the server has confirmed the current revision.
  - **Draft saved · Will sync when online** while a revision is waiting
    for the network or the account is offline.
  - **Draft saved · Couldn't sync** after a permanent error,
    with the reason available, and the draft kept locally.
- Drafts created or edited in another app appear in **Drafts** and can be
  opened, edited, sent, and discarded from ThreeStrands.
- Sending or discarding a draft removes it from the server. A message waiting
  to send or scheduled keeps its server copy until it has been sent.
- A draft for an account that is not set up on this device does not appear on
  this device. Drafts follow accounts, not sync groups.

## Architecture

```
composer ──300 ms──▶ local drafts table ──idle / close / quit──▶ draft push queue ──▶ provider
                          ▲                                                            │
                          └──────────── draft pull (mail sync) ◀──────────────────────┘
```

### Local buffer

The existing `drafts` table and `useDraftAutosave` stay as they are. SQLite
remains the composer's source of truth for the open draft. Its job is to hold
work that has not reached the server yet, not to sync it.

Each local draft's server link lives in the device-local
`draft_server_links` table, never in the draft payload and never in headers
that could leave the device:

| Column | Meaning |
| --- | --- |
| `server_id` | Gmail draft ID, or `NULL` before the first push. IMAP will store the mailbox UID of the current copy. |
| `pushed_revision` | Latest local revision the server has confirmed. |
| `pending_since`, `due_at` | When the oldest unpushed edit was saved, and when the next push or retry runs. |
| `state`, `error`, `attempts` | `pending`, `retrying`, `failed`, or `saved`, with the reason and the backoff count. |
| `server_version` | The server copy's version this device last wrote or read (Gmail's message ID, which changes on every edit). A different version means an edit made elsewhere. |
| `conflict` | The server's version, held as a draft, when both sides changed. Pushes stop until the user picks one. |

The `Message-ID` isn't stored: it is `draft-<draft id>@<domain>`, the same on
every device, so a pulled copy carrying it maps back to the same local draft.
Server copies waiting to be deleted are rows in `draft_server_deletions`.

A draft is "synced" when `pushed_revision == revision`.

### Push queue

A native queue, modelled on the Outbox and the label mutation queue, pushes
local revisions to the provider:

- **Triggers:** 5 seconds after the last save with no further edits, composer
  close, app quit (best effort, bounded wait), and reconnect. Repeated saves
  coalesce; only the newest revision is pushed.
- **One push per draft at a time.** A push carries the revision it was built
  from. Its result never overwrites newer local edits, matching the autosave
  rule.
- **Retries** use the shared `backoff.rs` schedule. Authentication errors block
  the account's pushes until it is reconnected; they don't spin.
- **Delete:** discarding deletes the server copy after the 10-second undo
  window (`DISCARD_SERVER_DELAY_MS`). The discarded draft and its attachment
  files are held for an hour, so Undo restores it: within the window the
  server copy is untouched, after it the draft is saved to the server again. Sending deletes it only once
  the send has completed (`sent` or `unverifiable`); while a message waits in
  the undo window, is scheduled, is sending, or has failed, the server copy
  stays, so **Undo** and **Restore** bring back the same draft. The local row
  goes immediately, so the UI never waits for the network.

### Server message

Pushes need a variant of `build_mime` ([correspondence/mime.rs](../src-tauri/src/correspondence/mime.rs))
that builds a draft rather than a send:

- No recipients is valid. `build_mime` currently refuses that.
- No Bcc stripping: the draft keeps Bcc so it survives a round trip. Sending
  still strips it as it does today.
- `Message-ID` is `serverMessageId`, stable across pushes. The send path keeps
  its own Message-ID rules.
- `In-Reply-To` and `References` come from the draft's `replyId` and
  `references`, so other apps thread it correctly.
- The signature and forwarded content are rendered into the body exactly as
  they would be sent.
- Attachments are included in full, so the server copy is complete.

### Gmail

- **Conversations:** mail sync drops `DRAFT`-labelled messages from every
  conversation it stores, and a conversation holding only drafts isn't
  stored. Drafts reach the app only through pull, and each one shows at the
  end of its conversation as a draft with **Resume Draft**.
- **Push:** `drafts.create` on first push, then `drafts.update` with the stored
  draft ID. Pass `threadId` for replies so the draft sits inside its
  conversation. The existing `gmail.modify` scope covers both.
- **Delete:** `drafts.delete`. Sending keeps the current send path and then
  deletes the server draft. Using `drafts.send` would change send undo,
  scheduling, and Outbox behavior, so it stays out of scope.
- **Pull:** `drafts.list` every minute per account, and right away when the
  window comes to the front. Only drafts whose version changed are fetched,
  with `format=raw`. Before each push, a `format=minimal` read checks the
  version, so an edit made since the last pull is never overwritten.

### IMAP

- **Copy ids:** a copy is `UIDVALIDITY:UID` in the mapped `\Drafts` mailbox,
  and that id is also its version, since a stored message never changes.
- **Push:** `APPEND` the new copy with `\Seen`, plus `\Draft` when the
  mailbox's `PERMANENTFLAGS` keeps it (Proton Mail Bridge's doesn't). The new
  UID comes from a Message-ID search after the append, so it works without
  `UIDPLUS`. A copy stored but not yet visible to SEARCH is a retryable
  error; the retry finds it by `Message-ID` rather than appending again.
- **Order matters:** only after the new copy is stored is the old one marked
  `\Deleted`, then removed with `UID EXPUNGE` when `UIDPLUS` is present.
  Without it the old copy stays marked, as the Sent copy path does, because a
  plain `EXPUNGE` would also remove other clients' deleted mail.
- **Duplicates:** listing reports only the newest copy of each `Message-ID`.
  Stale copies of this app's own drafts (`draft-…` Message-IDs), left by a
  save interrupted between storing and removing, are removed; other apps'
  duplicates are left alone.
- **Replaced elsewhere:** another device saving creates a new copy and removes
  the one this device knew. Before pushing, a vanished copy is looked up by
  `Message-ID`; a replacement found there is a conflict, not a deletion. A
  pull maps a replacement back to the draft through the same `Message-ID`.
- **Size:** fetches reuse the `APPEND` size limit in
  [policy.rs](../src-tauri/src/provider/imap/policy.rs).
- **Mail sync:** `\Drafts` stays `SyncWindowClass::NotSynced`. Drafts reach
  the app only through pull, never as conversation messages.
- **Threads:** IMAP conversations are threaded locally, so an imported reply
  has no conversation id. It opens as a reply with correct headers, but it
  doesn't show as a draft in its conversation and **Reply** starts a new one.
- **No `\Drafts` mailbox:** the account keeps local-only drafts, the
  composer says **Draft saved**, and the mailbox mapping step at setup says
  drafts stay on this computer. ThreeStrands doesn't create one.

### Pull and import

When a server draft changes and the change didn't come from our own push
(its version differs from `serverVersion`):

- **Known draft, no unsaved local edits:** replace the local content with the
  server copy. If it is open, the composer follows it, as it does for remote
  versions today.
- **Known draft, unsaved local edits:** keep both. The composer asks which to
  keep, reusing the existing conflict prompt. Neither Gmail nor IMAP checks
  versions on update, so this is the only conflict protection we have. It runs
  before every push: if the server copy changed since `serverVersion`, the
  push stops and asks first.
- **Unknown draft** (created in another app): import it as a new local draft
  linked to that server copy.
- **Server draft gone:** if the local draft has no unpushed edits, remove it.
  If it has, keep it and push it as a new server draft. A draft deleted
  elsewhere while you were still typing comes back rather than disappearing.

### Fields email can't carry

| Draft field | Server copy | Imported from another app |
| --- | --- | --- |
| `threadId`, `replyId`, `references`, `sourceId`, `mode` | `In-Reply-To`, `References`, and Gmail `threadId` | Rebuilt by finding the `In-Reply-To` message in local mail. If it isn't there, the draft opens as a new message whose reply headers are still correct. |
| `from` | `From` header | Matched against the account's sending identities, otherwise the account address. |
| `signature` | Rendered into the body | Treated as body text with no structured signature, so it is never added twice. ThreeStrands does not try to detect and strip signatures. |
| `forwardedContent` | Rendered into the body | Treated as body text. |
| `followUpTaskId` | Not carried | Local only. |
| `attachments` | Full MIME parts | Imported as attachments of the local draft. |

Edits made only in ThreeStrands keep every field locally. The loss happens only
when another app edits the draft.

A later option, not part of v1: a small Replicated Sync **draft metadata**
entity keyed by `serverMessageId`, holding signature choice, follow-up task,
and reply source. Other ThreeStrands devices would then reopen a draft with
full fidelity. Without it, they get the "imported" column above.

## Retiring Replicated Sync draft content

- Stop writing `EntityType::Draft` records and stop materializing them.
- Keep the entity type and its payload validation, so records from 0.147.x
  devices are accepted and ignored instead of reported as errors.
- Existing draft records in this device's sync log are left alone, and no
  deletions are published: a deletion would make a 0.147.x device drop drafts
  it has saved nowhere else. Local drafts are untouched and reach the server
  on their next edit.
- 0.147.x devices that are still running keep exchanging draft records with
  each other. That's harmless, and it ends once they update.
- The settings export does not contain drafts, so the settings-transfer format
  is unchanged; a transfer test guards it.

## Privacy

Every pushed revision reaches the mail provider, including drafts that are
later abandoned. That is how drafts work in every mail app, and the sent
message reaches the provider anyway. [features.md](features.md) and the site
docs must say so in place of the current "not synchronized with Gmail Drafts".
No ThreeStrands-specific data goes into server headers.

## Phasing

1. **Gmail push.** Push queue, server-link fields, draft MIME variant, honest
   save status, delete on send and discard. Drafts appear in Gmail.
2. **Gmail pull.** Import new, changed, and deleted drafts, conflict prompt on
   push, repair from `drafts.list`, and drafts shown in their conversations.
3. **Retire draft content sync.** Can ship with phase 2, so there is never a
   window where neither path syncs drafts between devices.
4. **IMAP push and pull,** including Proton Mail Bridge. Verify `APPEND` and
   expunge behavior against the primary account.
5. **Optional:** draft metadata entity.

Each phase bumps the minor version and gets a changelog entry.

## Testing

- Push coalescing: many saves produce one push of the newest revision; a stale
  push result never replaces newer edits.
- Offline: edits queue, status reads **Will sync when online**, and reconnect
  pushes once.
- Draft MIME: no recipients allowed, Bcc kept, stable `Message-ID`, reply
  headers present, header injection still blocked.
- Gmail mock: create then update reuses the draft ID; send and discard delete
  it; reply drafts carry `threadId`.
- IMAP mock with and without `UIDPLUS`: append before remove, duplicate
  collapse by `Message-ID`, size limit refusal.
- Import: known draft follows; unsaved edits trigger the conflict prompt;
  unknown drafts import; deleted-elsewhere drafts with local edits survive.
- Replicated Sync: 0.147.x draft records are accepted and ignored; nothing new
  is written.
- Conversations: a server draft in a thread appears once, as a draft, and
  **Reply** reopens it.
- Account removal: this-device removal pushes pending revisions first, warns
  with a count when it can't, and never calls a server delete.
- E2E: compose save status through push, offline, and failure.

## Decisions

Decided 2026-10-20.

- **Drafts inside conversations.** A server draft that belongs to a
  conversation, whether written in ThreeStrands or another app, appears in that
  conversation as the user's draft reply. It is shown as a draft, not as a
  received or sent message: opening it resumes it in the composer, and
  **Reply** reopens it instead of starting a second draft, as it does for local
  reply drafts today. Gmail's `threads.get` returns `DRAFT`-labelled messages
  with the thread. Phase 2 must check how those messages are stored and shown
  today, so a draft is never shown twice: once as a message and once as a
  draft.
- **Removing an account from this device only** never deletes its drafts from
  the server. Before the account's local data is removed, ThreeStrands pushes
  any revisions that haven't reached the server yet, waiting a bounded time. If
  some can't be pushed, for example because the device is offline, the removal
  confirmation says how many unsent draft edits will be lost and lets the user
  cancel. **Remove on all devices** follows the same rule: removing an account
  from ThreeStrands never deletes mail or drafts on the server.
- **The push delay is a fixed 5 seconds,** not a setting. It is a named
  constant next to `AUTOSAVE_DEBOUNCE_MS`, with tests below, at, and above the
  delay. It is in the range common mail apps use. Composer close and app quit
  push right away, so the delay only matters while typing.
