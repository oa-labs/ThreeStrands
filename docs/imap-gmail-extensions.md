# Gmail IMAP extensions, All Mail, and native labels

This document records how ThreeStrands uses Gmail's advertised `X-GM-EXT-1`
IMAP extensions (`X-GM-MSGID`, `X-GM-THRID`, `X-GM-LABELS`, `X-GM-RAW`), All
Mail acquisition, legacy thread repair, and label/folder management. Core
IMAP/SMTP behavior lives in [the IMAP design](imap-design.md); remaining parity
work is tracked in [the IMAP parity plan](imap-parity-plan.md).

Protocol references:
[Google's advertised IMAP extensions](https://developers.google.com/workspace/gmail/imap/imap-extensions),
[IMAP CREATE](https://www.rfc-editor.org/rfc/rfc3501.html#section-6.3.3), and
[IMAP mailbox commands](https://www.rfc-editor.org/rfc/rfc3501.html#section-6.3.4).

## Message and thread identity mappings

Schema migration 97 adds the device-local `imap_message_identities` table:

| Column | Meaning |
| --- | --- |
| `account_id` | Existing ThreeStrands account key |
| `namespace` | `gmail-message-id`, `objectid-emailid`, or `header-hash` |
| `external_id` | Canonical external token, or account-scoped header hash |
| `message_id` | Stable ThreeStrands message ID |

The primary key includes all four columns so contradictory observations can be
retained without silently overwriting evidence. An index supports reverse
ownership checks; all reads and writes are account-scoped.

- `gmail-message-id` canonicalizes the wire's unsigned decimal integer to
  lowercase hexadecimal without floating-point conversion, matching Gmail API
  spelling.
- `objectid-emailid` tokens remain case-sensitive and separate. Tokens are
  bounded by `MAX_EXTERNAL_ID_BYTES` (1024) in `src-tauri/src/provider/imap/policy.rs`
  and reject whitespace and control characters. (`async-imap` still does not
  expose live RFC 8474 `EMAILID`, so live `EMAILID` acquisition remains deferred.)
- `header-hash` entries are weaker evidence and never establish an authoritative
  cross-provider match.

### Resolution (`ImapStateStore::resolve_message_identity`)

Used transactionally (`src-tauri/src/db/imap/identity.rs`) by body acquisition,
index-only ingestion, and server search:

1. Look up the exact mailbox/UIDVALIDITY/UID location and all supplied server IDs.
2. If they identify different internal messages, retain conflicting candidates
   and return an error rather than guessing or mutating locations.
3. An existing exact location can learn additional identity namespaces; a second
   different ID in an already bound authoritative namespace is a conflict.
4. A known server mapping resolves a new folder location or new UIDVALIDITY to
   the same internal ID independently of header evidence.
5. For a new `gmail-message-id`:
   - On accounts converted from Gmail API (`gmail_imap_converted`), use the raw
     canonical Gmail hex ID directly (`db/imap/identity.rs`) so new acquisitions
     match existing Gmail API task, goal, and workflow IDs.
   - On non-converted IMAP accounts, create a deterministic, namespace-separated
     internal ID. Matching headers alone never attach a new server identity to
     legacy header-derived mail.
6. Without server IDs, plain IMAP's header-derived fallback runs, stopping if
   existing mappings show it is ambiguous or would impersonate a known
   authoritative message.

Reservations commit independently of a sync round (surviving failed body fetches
or restarts), never advance generations/cursors on their own, and commit
conflict evidence before returning an error.

### Protocol integration & `X-GM-THRID`

When `X-GM-EXT-1` is advertised, sessions (`src-tauri/src/provider/imap/session.rs`)
request `X-GM-MSGID` and `X-GM-LABELS` in identity and flag/label FETCH lists
(never from a plain IMAP server).

Migration 113 adds account-scoped `imap_gmail_threads` bindings. Typed FETCHes
read `X-GM-THRID` alongside `X-GM-MSGID` for the same UID and verify the message
ID before accepting the thread ID. New server conversations receive deterministic
internal IDs (`imap:gmail-thread:<SHA256>`, or raw Gmail hex IDs on converted
accounts via `db/imap/thread_repair.rs`); verified existing messages keep their
established internal conversation, and inconsistent bindings surface as sync
errors. Known thread bindings also let ThreeStrands match imported reply drafts
and look up snoozed conversations (`provider/imap/provider/snoozes.rs`).

## Legacy Gmail thread repair

Migration 115 adds `imap_gmail_message_threads` (`src-tauri/src/db/imap/thread_repair.rs`).
For older Gmail IMAP caches originally grouped by RFC headers, normal sync
rechecks retained UID locations in small batches after the mailbox walk
(`advance_gmail_thread_repair` in `provider/imap/provider/refresh.rs`). It
requires matching `UIDVALIDITY` and verifies the native `X-GM-MSGID` against any
established binding, never matching bodies, subjects, or sender-authored
headers.

- Each poll can repair one verified group. A split keeps an established native
  binding when a matching member exists; for an unbound legacy group, the oldest
  cached member whose Gmail conversation has no local binding can keep the old
  thread ID.
- A whole merge into another conversation keeps an alias for the old thread ID;
  an ambiguous split never gains a whole-thread alias.
- Provider assignments, cached reader rows, message IDs, and attachment stars
  commit together, with cached summaries rebuilt. Tasks and drafts with a
  verified source message follow that message (using replicated task writes that
  leave other fields alone); unambiguous whole merges carry thread-only
  references.
- Repair leaves a group untouched if members have unavailable or contradictory
  identities, a cached message is missing from the new projection, or the change
  would widen a pending mail mutation, snooze, or Outbox item, resuming when
  affected work settles. Ordinary IMAP accounts keep header-derived threading.

## All Mail, archive, and search completeness

### Aggregate acquisition and state

The catalog's `\All` (`All Mail`) role is a synced, label-free location bounded
by `FOLDER_SYNC_WINDOW` and `SENT_ROUND_MESSAGES`. Inbox runs first; All Mail
backfills in bounded rounds using the durable watermark, aging unpinned
locations out of the window while search pins and active snoozes keep needed
locations. Cached bodies and message identities are shared across copies.

Schema migration 98 adds device-local `imap_label_snapshots` (keyed by account
and stable message ID). Complete label membership—including empty sets—commits
atomically with location changes, thread assignments, generation updates, and
journals. Unchanged snapshots do not advance generations. Authoritative
`X-GM-LABELS` membership overrides stale physical folder copies for Inbox, Sent,
Trash, Spam, Draft, and user labels while retaining the thread-wide dynamic
label union.

Because label changes need not alter `EXISTS` or `UIDNEXT`, a negotiated All Mail
view sweeps its bounded window every poll while reusing cached bodies.

### Archive, undo, trash, spam, and snooze return

For messages with authoritative label identities, system actions use only
capability-gated additions/removals of Inbox, Trash, and Spam membership—never
replacing the complete label set, marking `\Deleted`, expunging, or moving to
a synthetic Archive folder.

Each native action locates the message by `X-GM-MSGID`, checks destination and
`UIDVALIDITY` guards (re-locating the destination after adding a label since the
source UID may vanish), and requires read-back confirmation preserving unrelated
labels before committing location changes and label state per message. Unarchive,
Undo, and snooze return add Inbox membership through this same path. Setup
discovery supplies a `labelArchive` runtime hint when `X-GM-EXT-1` and an `\All`
mailbox are present so the mapping screen omits the unused physical Archive
destination (verified server-side before accepting).

### Server search and partial coverage

Default IMAP search covers All Mail, Archive, Sent, user folders, and label
folders (Inbox first, Trash and Spam excluded unless `in:anywhere` or explicitly
targeted). When `X-GM-EXT-1` and an All Mail view are active, search issues
`X-GM-RAW` as a single escaped quoted string (`in:archive` maps to not-in-Inbox),
evaluating labels, boolean syntax, and attachments server-side while respecting
deadlines, acquisition budgets, and term/query limits. Excess mailbox targets
yield partial results with unsearched-folder counts rather than dropping hits;
All Mail search hits stay pinned across window sweeps and deduplicate by stable
message/conversation identity.

## Native labels, folder trees, and editing

Servers advertising `X-GM-EXT-1` expose user labels in **Manage Labels**
(`LabelModel::ImapNativeLabels`) with create, apply, remove, rename, and delete
actions (`src/labels.ts`, `src/LabelManager.tsx`,
`src-tauri/src/provider/imap/provider/{native_labels.rs,mutate.rs}`),
independently of configured keyword/folder storage modes.

- **Label & catalog model:** Existing native user folders become `user` labels
  keeping their exact `folder:` or `lf:` IDs so Split Inboxes and saved
  references remain intact. Display names decode modified UTF-7; protocol
  arguments retain wire encoding without double-escaping. System roles and
  advertised special-use views stay separate and excluded from user-label writes.
- **Capability state (migration 99):** `imap_server_capabilities` persists
  device-local negotiated extension support across restarts and is rechecked on
  every write. Once native semantics are observed, losing `X-GM-EXT-1` disables
  label writes rather than falling back to deleting folder copies.
- **Creation:** Accepts full label paths (including nested and non-ASCII names)
  without prepending a container. Reuses an exact selectable match if present or
  sends one quoted modified-UTF-7 `CREATE`, publishing only after a fresh `LIST`
  confirms a selectable user label.
- **Membership writes:** Mutations resolve an exact catalog label and message
  `X-GM-MSGID`, select an independent source (usually All Mail) under
  `UIDVALIDITY` guard when removing, send only `+X-GM-LABELS` / `-X-GM-LABELS`,
  and verify read-back before committing.
- **Quoted-name edge case & recency limit:** Native `LIST` parsing decodes
  quoted-string escapes once and preserves literals verbatim (correcting an
  earlier bug that retained escapes on labels containing quotes/backslashes).
  Separately, the UI label recency map enforces `MAX_LABEL_ID_LENGTH = 200`
  (`src/settings.ts`), which can discard recency history for very long encoded
  IMAP IDs without affecting identity or rules.

### Editing label hierarchies

Migration 114 adds `imap_label_bindings` and `imap_label_edits`
(`src-tauri/src/db/imap/label_edits.rs`, `label_replica.rs`), and settings
transfer v5 carries `TransferAccount::imap_label_bindings` and
`TransferSplitInbox::label_bindings` (`src-tauri/src/transfer.rs`).

- **Renaming a Gmail label or parent:** Sends one IMAP `RENAME`, records every
  visible selectable descendant, confirms every destination via `LIST`, and
  publishes verified paths in one transaction. Split Inboxes and pending label
  actions keep their logical label IDs; cached locations and search pins move
  when `UIDVALIDITY` is unchanged. Missing children block the edit, and lost
  acknowledgements/partial renames keep verified bindings while leaving vanished
  IDs available for explicit relinking.
- **Deleting a Gmail label or parent:** Sends one `DELETE` for the selected
  label, verifies the parent vanished as a selectable label while preserving
  messages and nested children in place with their original IDs, and retires the
  deleted ID so a new label at the same path gets a fresh ID while Split Inboxes
  stay preserved for relinking.
- **Plain IMAP folder trees:** Share the same journal and ID bindings to rename
  physical folder trees (carrying cached UIDs when `UIDVALIDITY` matches).
  Physical folder deletion stays limited to individual leaf folders and requires
  live proof of a surviving copy for every message and the folder's full UID set.

### Cross-device edits

IMAP structural edits use the
[account-scoped device authority contract](replicated-sync-design.md#device-authority-for-external-operations).

- ThreeStrands prefers an active device that introduced the account to
  Replicated Sync as its initial editing device; all active devices must
  acknowledge the assignment before rename or delete is permitted. Older clients
  must update and sync before structural edits unlock.
- On the current editor, open **Settings → Mail Accounts**, open the account's
  **More actions** menu, and choose **IMAP Editing Device…** (`src/AccountsSettings.tsx`,
  `src/ImapLabelEditorSettings.tsx`) to hand editing authority to another
  enrolled desktop device. Unfinished edits block handoff until reconciled;
  transfer editing before removing that device from Replicated Sync.
- Verified bindings and retired IDs replicate in the encrypted Mail Account
  record. Each IMAP peer verifies the live path and `UIDVALIDITY` on its own
  connection before attaching the ID, parking queued label actions while a
  binding is unverified and keeping provider-specific Split Inbox selections
  local when a peer uses another provider.

### Keyword edits

Keyword apply and remove (`provider/imap/provider/keyword_labels.rs`) operate on
individual selections only when the selected mailbox advertises permanent
keyword support in `PERMANENTFLAGS` and verify resulting flags by UID.
Account-wide keyword rename and removal remain unimplemented in the parity
backlog because they require a resumable scan across every applicable mailbox,
including mail outside the local sync window.
