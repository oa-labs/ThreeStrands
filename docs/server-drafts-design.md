# Server drafts design

Status: proposed (2026-10-20). Replaces the draft content sync that shipped in
0.147.0 through Replicated Sync.

## Why change

0.147.0 syncs compose drafts between ThreeStrands devices as Replicated Sync
entities (`EntityType::Draft`, [draft_sync.rs](../src-tauri/src/draft_sync.rs)).
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
  - **Draft saved** only once the server has confirmed the current revision.
  - **Saved on this device · Will sync when online** while a revision is waiting
    for the network or the account is offline.
  - **Saved on this device · Couldn't save to server** after a permanent error,
    with the reason available, and the draft kept locally.
- Drafts created or edited in another app appear in **Drafts** and can be
  opened, edited, sent, and discarded from ThreeStrands.
- Sending, scheduling, or discarding a draft removes it from the server.
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

Each local draft gains server-link fields stored natively, never in headers
that could leave the device:

| Field | Meaning |
| --- | --- |
| `serverId` | Gmail draft ID, or `null` before the first push. IMAP stores the mailbox UID of the current copy. |
| `serverMessageId` | Stable RFC `Message-ID` reused for every server copy of this draft. |
| `pushedRevision` | Latest local revision the server has confirmed. |
| `serverVersion` | What the server reported for the copy we last wrote or read: Gmail message ID, or IMAP UID plus `UIDVALIDITY`. Used to detect edits made elsewhere. |
| `pushState` | `idle`, `pending`, `pushing`, `blocked` (with typed error). |

A draft is "synced" when `pushedRevision == revision`.

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
- **Delete:** sending or discarding enqueues a server delete with the last
  known `serverId`/`serverMessageId`. The local row goes immediately, so the
  UI never waits for the network.

### Server message

Pushes need a variant of `build_mime` ([correspondence.rs](../src-tauri/src/correspondence.rs))
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

- **Push:** `drafts.create` on first push, then `drafts.update` with the stored
  draft ID. Pass `threadId` for replies so the draft sits inside its
  conversation. The existing `gmail.modify` scope covers both.
- **Delete:** `drafts.delete`. Sending keeps the current send path and then
  deletes the server draft. Using `drafts.send` would change send undo,
  scheduling, and Outbox behavior, so it stays out of scope.
- **Pull:** mail sync already sees `DRAFT`-labelled messages through history.
  Draft changes drive an import step (below) instead of showing up as thread
  messages. `drafts.list` repairs state on first run and after a history gap.

### IMAP

- **Push:** `APPEND` the new copy to `\Drafts` with the `\Draft` and `\Seen`
  flags. Only after it succeeds, remove the previous copy: `UID EXPUNGE` when
  `UIDPLUS` is present, otherwise mark it `\Deleted` and leave it, as the Sent
  copy path does. With `UIDPLUS`, `APPENDUID` gives the new UID. Without it,
  the next pull finds the copy by `serverMessageId`.
- **Order matters:** always append before removing, so a failure leaves a
  duplicate rather than nothing. The pull step collapses duplicates that share
  a `serverMessageId` by keeping the newest.
- **Size:** reuse the existing `APPEND` size limit in
  [policy.rs](../src-tauri/src/provider/imap/policy.rs). A draft over the
  limit stays local with the **Couldn't save to server** status.
- **Pull:** `\Drafts` moves from `SyncWindowClass::NotSynced` to a synced
  class, with headers fetched in a window and bodies fetched on open. Like
  other synced mailboxes, it contributes no label, and its messages never
  appear as conversation messages.
- **No `\Drafts` mailbox:** the account keeps local-only drafts, and
  **Settings → Mail Accounts** says so. ThreeStrands doesn't create one.

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
- On upgrade, delete this device's draft records from the sync log; local
  drafts are untouched and get pushed to the server like any other.
- 0.147.x devices that are still running keep exchanging draft records with
  each other. That's harmless, and it ends once they update.
- The settings export does not contain drafts, so the settings-transfer format
  is unchanged. Confirm this with a test before shipping.

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
