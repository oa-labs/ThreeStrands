# IMAP/SMTP provider design

Status: accepted, with every open question resolved (2026-10-08).
Implementation hasn't started; phase 1 of [Phasing](#phasing) is next.

This is the design note `PLAN.MD` asks for before IMAP/SMTP work starts
("after its folder and threading behavior is designed"). IMAP now
comes before Microsoft 365. Microsoft 365 needs an Entra app registration, and
the primary user's mail is not on Microsoft 365. It is on Proton Mail, reached
through Proton Mail Bridge, which runs a local IMAP/SMTP server on the user's
machine. See [The primary account](#the-primary-account) for what that server
advertises and how it changes the design.

## Decisions already made

| Question | Decision |
| --- | --- |
| Which servers | Any standards-compliant IMAP server, whose extension support isn't known in advance. That includes ISP and hosting servers and local bridges such as Proton Mail Bridge, which the primary user runs. The design has to work on a plain IMAP4rev1 server and use newer extensions when a server advertises them. Nothing branches on the server's identity. |
| Authentication | Password or app password only in v1, stored in the OS keychain, TLS required. OAuth2 (XOAUTH2/OAUTHBEARER) can be added later inside the same credential envelope. |
| Labels | System states use folders. Archive, Trash, Spam and Sent map to special-use folders. The user's existing folders appear as read-only locations with a Move action. ThreeStrands user labels are stored one of two ways, set per account: as IMAP keywords when `PERMANENTFLAGS` includes `\*`, or as **label folders**, copies in children of a container mailbox the user picks at setup. The primary account uses label folders (decided 2026-10-08). See [User labels](#user-labels). |
| Calendar and contacts | Mail first. IMAP v1 covers mail and server search. CalDAV/CardDAV is a later step that needs a calendar provider interface. A Google Calendar connection still works alongside an IMAP mail account. |

## Guiding rules

1. **Act on capabilities, never on hostnames.** Behaviour is chosen from the
   server's `CAPABILITY` response, `PERMANENTFLAGS` and `LIST` attributes, never
   from the server's hostname or brand. This is the same rule as the
   email-rendering policy in `AGENTS.md`. Matching mailbox names such as
   "Sent Items" is allowed only as a fallback when `LIST` returns no
   special-use attributes, and the user can always override the result.
2. **The sync engine and UI stay provider-neutral.** The IMAP provider
   implements the existing `MailSync`/`MailFetch`/`MailMutate`/`MailSend`
   traits. It produces `RawMessage` envelopes with the same system label names
   the rest of the app already understands (`INBOX`, `UNREAD`, `STARRED`,
   `SENT`, `SPAM`, `TRASH`). Database, triage and rendering code do not branch
   on the provider.
3. **Never cause side effects by reading.** Every body fetch uses
   `BODY.PEEK[...]`, so opening or syncing a message never sets `\Seen` on the
   server.
4. **When unsure, do nothing destructive.** No `EXPUNGE` without `UIDPLUS`.
   No automatic resend after an uncertain SMTP result. Threads are never merged
   on subject alone.
5. **Same trust boundary.** IMAP mail goes through `mime.rs` normalization,
   the sanitizer and the remote-image proxy exactly like Gmail mail. No IMAP
   content takes any other path to the renderer.

## Recommended stack

| Concern | Recommendation | Why |
| --- | --- | --- |
| IMAP client | [`async-imap`](https://crates.io/crates/async-imap) 0.12 with `default-features = false, features = ["runtime-tokio"]`, behind an internal `ImapSession` trait | It is actively maintained (0.12.0 released 2026-09-30) and widely used: Delta Chat runs it against every kind of server. It has calls for everything the primary account offers: `idle()`, `uid_mv`, `uid_expunge`, `append`, `id()`, plus `select_condstore` for later. It wraps any `AsyncRead + AsyncWrite` stream, so STARTTLS and certificate pinning stay in our own rustls code. Its default runtime is async-std, so the feature flags matter. |
| IMAP fallback | [`imap-codec`](https://crates.io/crates/imap-codec) 1.0 / [`imap-next`](https://crates.io/crates/imap-next) | The most complete typed model of the protocol, including `CONDSTORE`/`QRESYNC`/`VANISHED`. `imap-next` is low-level, so we would write the command flow ourselves. It's the replacement if `async-imap` turns out to be inadequate. The `ImapSession` trait keeps that swap local. |
| SMTP | [`lettre`](https://crates.io/crates/lettre) 0.11 (tokio, rustls) | Supports implicit TLS and STARTTLS, `AUTH PLAIN/LOGIN`, and XOAUTH2 for later. It doesn't accept a custom certificate verifier. For a pinned certificate, use `CertificateStore::None` plus `add_root_certificate(pinned)`, which makes that one certificate the only trusted root. Never use `dangerous_accept_invalid_certs`. |
| MIME | `mail-parser` / `mail-builder` (already dependencies) | Reuse. Raw `BODY.PEEK[]` bytes are parsed into `RawMessage`. |
| TLS | rustls 0.23 with the `ring` provider (already a dependency), with certificates always verified | The same stack `reqwest` already uses. A certificate that doesn't chain to a trusted root is accepted only if it matches the certificate the user pinned at setup (see [Account setup](#account-setup)). On IMAP that's a custom `ServerCertVerifier` comparing the SHA-256 fingerprint. On SMTP it's lettre's root-store route above. Verification is never switched off. |
| DNS SRV | `hickory-resolver` | Only needed for RFC 6186 autodiscovery. |

Avoid the blocking [`imap`](https://crates.io/crates/imap) crate (2.4.1, last
released Feb 2025; 3.0 has stayed in alpha) and
[`imap-client`](https://crates.io/crates/imap-client) (0.3, a small user
base). `imap-client` also discards `COPYUID` on move and creates its own TLS
connection, which gets in the way of pinning.

**Known `async-imap` gaps, and the plan for each:**
- `uid_mv` and `append` return `()`, so the `COPYUID`/`APPENDUID` response
  codes are thrown away. Send those commands through `run_command` and read the
  tagged response code, which `imap-proto` already parses as `CopyUid`/`AppendUid`.
  If that turns out to be awkward, fall back to finding the message by its
  Message-ID on the next sync. That fallback already exists for servers without
  `UIDPLUS`, and it's cheap on a local server.
- `QRESYNC` and `VANISHED` aren't wrapped. The primary account doesn't offer
  them, so the "neither" sync path ships first. `QRESYNC` support can follow
  through raw commands without blocking v1.

**Spike before committing (about a day).** Run it against the primary account's
profile first (a Dovecot container restricted to
`IMAP4rev1 IDLE MOVE UIDPLUS UNSELECT ID`, a self-signed certificate, and no
custom keywords), then against Bridge itself. Check that:
- STARTTLS works on a non-standard port over a stream we upgrade ourselves,
  with a fingerprint-pinning `ServerCertVerifier`;
- lettre with `CertificateStore::None` plus the pinned certificate connects to
  a self-signed SMTP server whose certificate names the host only in its CN, or
  only in an IP SAN, for `127.0.0.1`. If it can't, that changes the SMTP pinning
  design;
- `IDLE` survives the 25-minute re-issue and reconnects cleanly after the
  server drops the connection;
- `UID MOVE` and `APPEND` sent through `run_command` return `COPYUID` and
  `APPENDUID`;
- `UID STORE` of a flag that isn't in `PERMANENTFLAGS` is detected rather than
  silently ignored.

Anything that fails decides between the raw-command route and moving to
`imap-next`.

## Server capabilities

Every extension below is optional. Each row says what the provider does when
the server advertises the extension and what it falls back to when it doesn't.

| Extension | When present | When absent |
| --- | --- | --- |
| `IDLE` (RFC 2177) | One connection idles on INBOX. IDLE is re-issued every 25 minutes. | Poll INBOX on the existing schedule. |
| `CONDSTORE` (RFC 7162) | Flag changes are fetched with `CHANGEDSINCE`. | Re-fetch `FLAGS` over the sync window each poll. |
| `QRESYNC` (RFC 7162) | Deletions arrive as `VANISHED`. Each mailbox resyncs in one round trip. | Find deletions with `UID SEARCH ALL` compared against local UIDs. This runs every poll for INBOX and less often for other folders. |
| `MOVE` (RFC 6851) | Moves are atomic. | `COPY` + `\Deleted` + `UID EXPUNGE`, which needs `UIDPLUS`. |
| `UIDPLUS` (RFC 4315) | New UIDs come back from `COPYUID`/`APPENDUID`. Targeted `UID EXPUNGE`. | No expunge. The source copy is only marked `\Deleted`, because a plain `EXPUNGE` would also remove other clients' deleted mail. The new location is found on the next sync. |
| `SPECIAL-USE` (RFC 6154) | `\Sent`, `\Archive`, `\Junk`, `\Trash`, `\Drafts`, `\All`, `\Flagged` come from `LIST`. | Special-use attributes are still read from a plain `LIST` when present; some servers return them without advertising the extension. Only when none come back are common mailbox names matched, and the user confirms the result at account setup. |
| `OBJECTID` (RFC 8474) | Message ID from `EMAILID`, thread ID from `THREADID` when the server returns it. | IDs are derived locally (next section). |
| `ESEARCH` (RFC 4731) | Search returns compact result sets. | Plain `SEARCH` results. |
| `LIST-STATUS` (RFC 5819) | One round trip for every mailbox's counters. | One `STATUS` command per mailbox. |
| `ENABLE`, `UTF8=ACCEPT`, `ID` | Turned on when offered. `ID` is always sent; some servers expect it. | Mailbox names are decoded from modified UTF-7. |

The **minimum requirement** is IMAP4rev1 over implicit TLS (usually port 993)
or STARTTLS (usually port 143, but any port the user configures, such as
Bridge's 1143). If a server set up for STARTTLS doesn't offer it, connecting
fails. There is no plaintext fallback, which closes off attacks that
strip STARTTLS.

## The primary account

The primary user's server, captured 2026-10-08 (Proton Mail Bridge, IMAP on
`127.0.0.1:1143` over STARTTLS):

```text
before login: AUTH=PLAIN ID IDLE IMAP4rev1 STARTTLS
after login:  AUTH=PLAIN ID IDLE IMAP4rev1 MOVE STARTTLS UIDPLUS UNSELECT
certificate:  self-signed, CN=127.0.0.1, O=Proton AG
```

`LIST "" "*"` (no folders or labels created yet):

```text
* LIST (\Marked \Noinferiors \Trash) "/" "Trash"
* LIST (\Noinferiors \Sent \Unmarked) "/" "Sent"
* LIST (\Drafts \Noinferiors \Unmarked) "/" "Drafts"
* LIST (\All \Marked \Noinferiors) "/" "All Mail"
* LIST (\Noselect \Unmarked) "/" "Folders"
* LIST (\Noselect \Unmarked) "/" "Labels"
* LIST (\Marked \Noinferiors) "/" "INBOX"
* LIST (\Flagged \Noinferiors \Unmarked) "/" "Starred"
* LIST (\Archive \Marked \Noinferiors) "/" "Archive"
* LIST (\Junk \Marked \Noinferiors) "/" "Spam"
```

`SELECT INBOX`:

```text
* FLAGS ($Forwarded Forwarded \Deleted \Flagged \Seen)
* OK [PERMANENTFLAGS ($Forwarded Forwarded \Deleted \Flagged \Seen)] Flags permitted
* OK [UIDNEXT 979] Predicted next UID
* OK [UIDVALIDITY 95479608] UIDs valid
a3 OK [READ-WRITE] SELECT
```

What this means for the design. Everything below follows from capabilities and
`LIST` attributes, so none of it needs code that checks for Bridge:

| Observation | Consequence |
| --- | --- |
| No `CONDSTORE` or `QRESYNC` | Every poll uses the "neither" sync path: re-fetch `FLAGS` over the sync window and compare `UID SEARCH ALL` with local UIDs to find deletions. This is the daily path, not a fallback, so the lowest capability tier gets the most test coverage. On a loopback server these round trips are cheap. |
| `IDLE` | New INBOX mail arrives without polling. |
| `MOVE` + `UIDPLUS` | Moves are atomic and return new UIDs through `COPYUID`. Archive, trash and spam don't wait for the next sync to learn where messages went. |
| `SPECIAL-USE` not advertised, but plain `LIST` returns `\Sent`, `\Drafts`, `\Trash`, `\Junk`, `\Archive`, `\All` and `\Flagged` | Always send a plain `LIST` (`RETURN (SPECIAL-USE)` needs `LIST-EXTENDED`, which isn't advertised) and trust the attributes it returns. Every system mailbox is mapped from attributes, so no name matching is needed and setup has nothing to confirm. An Archive mailbox already exists, so creating one isn't needed here. |
| `\Noselect` containers (`Folders`, `Labels`) | Mailboxes with `\Noselect` aren't locations and never become `folder:` labels. Only their selectable children do. |
| `\All` (`All Mail`) and `\Flagged` (`Starred`) | Aggregate mailboxes, skipped by sync (see [Labels](#labels-in-the-rawmessage-envelope)). |
| No `LIST-STATUS` | Counters come from one `STATUS` per synced mailbox. |
| No `OBJECTID` | Message and thread IDs are derived locally (below). |
| No `ENABLE` / `UTF8=ACCEPT` | Mailbox names are decoded from modified UTF-7. |
| Self-signed certificate for `127.0.0.1` | Strict verification fails. Setup needs the fingerprint-pinning path (see [Account setup](#account-setup)). |
| `PERMANENTFLAGS` has no `\*` | Custom keywords can't be stored, so ThreeStrands user labels can't be keywords on this account. The account uses label folders instead (see [User labels](#user-labels)). |
| `PERMANENTFLAGS` has no `\Answered`, `\Draft`, `$Junk` or `$NotJunk` | Only flags listed in `PERMANENTFLAGS` (or any keyword when `\*` is present) are ever stored. The spam action skips the `$Junk`/`$NotJunk` hint here, and moving to `\Junk` is enough. Replies don't set `\Answered`. |
| `$Forwarded` is permanent | Forwarding can set `$Forwarded`. It's a standard keyword and needs no custom-keyword support. |
| A local server on a non-standard port | Autodiscovery from the email domain finds Proton's public hosts, not the local server. Manual setup has to be a normal path, not a hidden one. |

User folders and labels will appear as children of the `Folders` and `Labels`
containers once some exist.
Proton labels are many-to-many, so a labelled message shows up both in its
folder and under `Labels/…`. Because of the multi-location model, that needs no
special case. ThreeStrands writes its own labels the same way, with `Labels`
as the account's label container (see [User labels](#user-labels)).

## Data model

### Message identity

IMAP UIDs are only unique within one mailbox while its `UIDVALIDITY` stays the
same, and they change whenever a message moves. ThreeStrands needs a message ID
that stays stable across moves. It must also be unique across accounts, because
`messages.id` is a primary key over all accounts.

- **Message ID:** `imap:<account>:<EMAILID>` when the server supports
  `OBJECTID`. Otherwise `imap:<account>:<hash>`, where the hash covers the
  normalized `Message-ID` header. Messages without a `Message-ID` header hash
  `Date`, `From`, `Subject` and `RFC822.SIZE` instead.
- **One message, several places:** if the same Message-ID appears in two
  mailboxes (for example a Bcc to yourself arrives in INBOX and is also stored
  in Sent), it is **one message with two locations**. This matches the "one
  message, several labels" model the app already uses.
- **Location table** (the provider's own state; see below):

```sql
CREATE TABLE imap_mailboxes (
    account_id TEXT NOT NULL,
    name TEXT NOT NULL,              -- decoded; delimiter kept separately
    delimiter TEXT,
    special_use TEXT,                -- \Sent, \Archive, … or NULL
    uidvalidity INTEGER NOT NULL,
    uidnext INTEGER NOT NULL,
    highestmodseq INTEGER,           -- NULL without CONDSTORE
    permanent_flags_json TEXT,       -- writable SELECT flags; NULL until known
    permanent_keywords INTEGER,     -- writable SELECT permits \*; NULL until known
    PRIMARY KEY (account_id, name)
);

CREATE TABLE imap_locations (
    account_id TEXT NOT NULL,
    mailbox TEXT NOT NULL,
    uidvalidity INTEGER NOT NULL,
    uid INTEGER NOT NULL,
    message_id TEXT NOT NULL,        -- the stable id above
    flags_json TEXT NOT NULL,
    modseq INTEGER,
    PRIMARY KEY (account_id, mailbox, uidvalidity, uid)
);
CREATE INDEX imap_locations_by_message ON imap_locations(account_id, message_id);
```

**Seam change:** the IMAP provider needs persistent state that can't fit in
the opaque `SyncCursor`: the UID-to-ID map and each mailbox's counters. It gets
a narrow `ImapStateStore` handle onto these tables when it is constructed. The
cursor itself stays small, holding only a sync generation number. The Gmail
provider is unaffected.

Mailbox write capabilities remain unknown until a writable `SELECT` supplies
`PERMANENTFLAGS`. Read-only discovery uses `EXAMINE` for counters and preserves
any known write capabilities; its read-only permissions never replace them.
Database migration 56 invalidates the unreliable EXAMINE-derived permissions
stored by earlier versions while preserving the catalog and message locations.
This provider-internal metadata is not part of settings transfer.

### Threading

Most ISP servers don't return `THREADID`, so local threading is the main path:

- Threads are grouped by message references. Each message's normalized
  `Message-ID`, `In-Reply-To` and `References` are combined per account, and
  messages that point at each other share a thread. This is the reference
  part of the JWZ threading algorithm, without its subject grouping.
- **There is no subject-based merging.** Merging on subject joins unrelated
  "Invoice" or "Re: hello" threads. A thread that is wrongly left split is
  harmless; a wrong merge is not.
- A thread's ID is fixed when the thread is created: `imap:<account>:t:<hash of
  the first message ID seen>`.
- **Merges:** a late message can link two existing threads. When that
  happens, the older thread's ID survives and the other thread's messages are
  re-pointed to it. A `thread_aliases(account_id, old_id, new_id)` row lets
  tasks, triage events and replicated-sync references that hold the old ID
  resolve to the new one. Both IDs are reported as changed so the sync engine
  re-ingests them.
- When the server returns `THREADID`, it is used and local merging is skipped.

`ProviderCapabilities` gains a flag `provided_threads: bool`, as already
anticipated in `provider.rs`. It is false for local threading.

### Labels in the `RawMessage` envelope

Labels are worked out from where a message is stored and its flags:

| IMAP state | `label_ids` entry |
| --- | --- |
| Message is in `INBOX` | `INBOX` |
| Message is in the `\Sent` mailbox | `SENT` |
| Message is in the `\Junk` mailbox | `SPAM` |
| Message is in the `\Trash` mailbox | `TRASH` |
| No `\Seen` flag | `UNREAD` |
| `\Flagged` | `STARRED` |
| Keyword `foo` (keyword mode) | `kw:foo`, a user label |
| Copy in `<container>/foo` (label-folder mode) | `lf:foo`, a user label |
| Message is in user folder `Clients/Acme` | `folder:Clients/Acme`, a new `kind: "folder"` label |

A message stored only in the Archive mailbox therefore has no `INBOX` label.
The existing rule "archived = no INBOX" in `db.rs` works without changes.

**Aggregate mailboxes** are mailboxes with the RFC 6154 `\All` or `\Flagged`
attribute (or a user-confirmed equivalent when `LIST` returns no attributes). They
are views of mail stored elsewhere, so they don't contribute `folder:` labels
and aren't synced as locations. Otherwise every message would get a
`folder:All Mail` label and be fetched twice. `\Flagged` state comes from the
`\Flagged` flag, not from membership in a starred mailbox.

**`\Noselect` mailboxes** are containers in the hierarchy. They hold no
messages, aren't synced and don't produce labels.

**A message in several user folders** is already one message with several
locations, so it gets several `folder:` labels. On servers that expose
many-to-many labels as mailboxes, this is how those labels show up, without any
name-based special case.

`list_labels` returns user labels (`kind: "user"`, from keywords or label
folders, depending on the account's mode) and folders (`kind: "folder"`). The
frontend shows folders as locations, not as labels that can be toggled.

### User labels

Each account has a `label_storage` setting:

- **`keywords`**: the default when INBOX's `PERMANENTFLAGS` includes `\*`.
- **`folders`**: the user picks a container mailbox, either a `\Noselect`
  container or a top-level folder (`Labels` on the primary account). Each child
  of the container is one label. Nothing is detected by name. The choice is
  shown at setup when keywords are unavailable, and it can be changed later in
  account settings.

When `PERMANENTFLAGS` lacks `\*` and the user hasn't picked a container, user
labels are unavailable and the UI hides label actions for that account. It
never pretends a label stuck.

In label-folder mode:

- **Reading:** a message's locations in the container's children become
  `lf:<name>` user labels, not `folder:` labels. Labels added in other clients
  (Proton's own apps, on the primary account) appear the same way. The
  container and its children are left out of the Move-to-folder list.
- **Adding a label:** `UID COPY` the thread's messages from one of their
  current locations into `<container>/<name>`. If the server answers
  `TRYCREATE`, create the mailbox and retry once. With `UIDPLUS`, the new
  location comes from `COPYUID`; otherwise the next sync finds it.
- **Removing a label:** in `<container>/<name>`, `UID STORE +FLAGS.SILENT
  (\Deleted)` the copies, then `UID EXPUNGE` those UIDs only. This needs
  `UIDPLUS`; without it, removing labels is unavailable, because a plain
  `EXPUNGE` could remove mail flagged `\Deleted` by other clients. Only
  locations inside the label mailbox are touched, never the message's other
  locations.
- **Creating, renaming and deleting labels:** `CREATE`, `RENAME` and `DELETE`
  on the container's children. Deleting a label deletes only that label
  mailbox.
- **System actions don't touch label copies.** Archive, trash, spam and
  move-to-folder act only on the message's locations outside the container.
  On servers where label mailboxes are views of one stored message, as on the
  primary account, the server keeps the views consistent. On servers where
  they're real copies, a trashed message keeps its labels, the same as Gmail.
- **Sync:** label mailboxes sync headers and flags like other user folders.
  Bodies are shared through the stable message ID, so a labelled message is
  never downloaded twice.

## Sync

### What gets synced

- **Mailboxes:** INBOX, Sent, Archive, Junk and Trash are synced continuously.
  Drafts are not, because ThreeStrands drafts are local. Aggregate mailboxes
  (`\All`, `\Flagged`) are not synced; see above. User folders are
  listed. Their headers sync inside the window below, and bodies are fetched
  when a message is opened.
- **Initial window** (decided 2026-10-08, matching Gmail):
  - **INBOX:** all of it. The inbox is a to-do list, and a cutoff would hide
    old mail that still needs handling. A count limit applies only to inboxes
    too large to sync in full.
  - **Sent:** the newest 5,000 messages, filled in the background after the
    first INBOX sync. This reuses the Gmail limit
    (`MAX_SENT_BACKFILL_THREADS` in `sync.rs`), so the address book, contact
    timelines and Keep in Touch see the same depth of history on both
    providers.
  - **Archive and user folders, including label folders:** headers for the
    newest 2,000 messages in each, with bodies fetched when a message is
    opened.
  - **Junk and Trash:** headers inside the same per-folder limit.
  - **Older mail** is reached through server search, as with Gmail today.

  There is no age cutoff. All of these limits go in one policy module,
  following the pattern of `emailRenderingPolicy.ts`, with tests just below, at
  and just above each limit.
- **Bodies:** message bodies never change in IMAP. Only flags and locations
  do. So `BODY.PEEK[]` is fetched once per message ID and cached. A flag change
  never re-downloads a thread. Messages above a size threshold fetch
  `BODYSTRUCTURE` and their text parts first. Attachments are fetched lazily by
  MIME section number, which becomes the `attachment_bytes` handle.

### Incremental sync for one mailbox

```text
SELECT/EXAMINE mailbox
  ├─ UIDVALIDITY changed? → drop that mailbox's locations, resync only it
  ├─ QRESYNC:   SELECT (QRESYNC (uidvalidity modseq)) → VANISHED + changed FLAGS
  ├─ CONDSTORE: UID FETCH 1:* (FLAGS) (CHANGEDSINCE modseq)
  │             UID FETCH uidnext:* (…)               new mail
  │             UID SEARCH ALL vs local               deletions (at a slower rate)
  └─ neither:   UID FETCH uidnext:* (…)               new mail
                UID FETCH <window> (FLAGS)            flag changes
                UID SEARCH ALL vs local               deletions
```

A `UIDVALIDITY` reset (a local bridge does this when it rebuilds its cache)
throws away locations, not bodies. The body cache is keyed by the stable message
ID, so a resync re-reads headers and flags but doesn't download every message
again.

New, changed and deleted locations are mapped to message IDs and then to
thread IDs. These go back to the existing engine as `SyncBatch.changed_threads`.
`fetch_thread` builds the thread from the location table and the body cache.

**What triggers a sync:** the `IDLE` connection for INBOX. For other
mailboxes, `STATUS` (or `LIST-STATUS`) on the existing polling schedule. A
mailbox is only re-selected when its `UIDNEXT`, `MESSAGES` or `HIGHESTMODSEQ`
changed.

**Connections:** at most two per account, one idling and one for commands,
with commands run one at a time. ISP servers often cap connections per user
across all of that user's devices. Dovecot's default is 10.

**Errors** are mapped onto `ProviderError` using RFC 5530 response codes:
- `AUTHENTICATIONFAILED` → `ReauthenticationRequired`.
- `UNAVAILABLE`, `INUSE`, a dropped connection or a timeout →
  `TransientTransport`.
- `OVERQUOTA` → `PermanentClientRejection`, with a message the user can read.
- `TRYCREATE` → create the target mailbox, then retry once.
- `BAD` → `InvalidOperation`.

Only an explicit authentication failure pauses the account. This matches the
Gmail rule that only `invalid_grant` forces a reconnect.

## Changes (archive, read, star, labels)

`MailMutate::modify_thread(id, add, remove)` remains the single point where
label changes are translated. For IMAP it applies them to each message's
current locations:

| Change | IMAP operation |
| --- | --- |
| Archive (remove `INBOX`) | Move the thread's INBOX messages to the Archive mailbox. Sent copies stay in Sent. |
| Move back to inbox (add `INBOX`) | Move the messages from Archive, Trash or Junk back to INBOX. |
| Trash / untrash | Move to `\Trash` / move back to INBOX. |
| Spam / not spam | Move to `\Junk` and set `$Junk`, clearing `$NotJunk`. The reverse moves to INBOX and sets `$NotJunk`. The keywords help server-side spam filters learn. They're set only when `PERMANENTFLAGS` allows them; otherwise the move alone counts. |
| Read / unread | `UID STORE ±FLAGS.SILENT (\Seen)` |
| Star | `±FLAGS.SILENT (\Flagged)` |
| Add or remove a user label | Keyword mode: `±FLAGS.SILENT (keyword)`. Label-folder mode: copy into, or expunge from, `<container>/<label>` (see [User labels](#user-labels)). With neither mode available, fail with `InvalidOperation` and a clear message. |
| Move to folder (new action) | Move to the chosen mailbox. |

- **Only permanent flags:** a `STORE` is limited to flags the mailbox's
  `PERMANENTFLAGS` allows. A label change the server can't keep fails with
  `InvalidOperation` instead of appearing to work until the next reselect.
- **Idempotent:** each change first checks where the message currently is, so
  retries from the outbox do no harm.
- **New locations:** the new UID is recorded from `COPYUID` when available.
  Otherwise the next sync finds the moved message by its Message-ID.
- **No Archive mailbox** (decided 2026-10-08): the Archive mailbox is chosen
  at setup, on the same mapping screen as the other system mailboxes.
  1. If `LIST` marks a mailbox `\Archive`, it is used without asking.
  2. Otherwise, a folder whose name matches a known archive name is
     preselected (the existing name-matching fallback), and the user confirms
     or changes it.
  3. If no folder matches, the default is "Create `Archive`", and the user can
     pick any existing folder instead.

  The choice is saved as a mailbox override and can be changed in account
  settings. Archive is never turned off for an account.

## Sending

1. **Choose the From address** from the account's identities (see
   [Identities](#identities)).
2. **Submit over SMTP** with `lettre`:
   - Implicit TLS on port 465 is preferred (RFC 8314). STARTTLS on port 587,
     or any port the user sets (Bridge's default is 1025), is accepted but
     required; there is no plaintext fallback.
   - The SMTP connection uses the same certificate rules, including a pinned
     fingerprint.
   - Same username and password as IMAP unless the user sets separate SMTP
     credentials.
3. **Save the sent copy** with `APPEND` to `\Sent`, flagged `\Seen`. With
   `UIDPLUS`, `APPENDUID` records the location immediately.
4. **Avoid duplicate sent copies:** some hosts save submitted mail to Sent
   themselves. After the first send on an account, look in Sent for the
   Message-ID. If it's already there, skip `APPEND` and remember
   `server_saves_sent` for that account. The user can override this in account
   settings. Bridge is expected to save sent mail itself, so on the primary
   account this detection runs on the first send.

How this differs from Gmail's delivery:

- **The receipt doesn't wait for `APPEND`.** The stable message ID comes from
  the Message-ID header, which we generate, so it is known as soon as SMTP
  accepts the message. A failed `APPEND` is queued as its own retry job and
  never causes a resend.
- **An uncertain SMTP result can't be checked against the server.** If the
  connection drops after the message data was sent but before the server's
  `250` reply, the Sent folder can't confirm delivery, because only we would
  have put a copy there. The send is shown as "may have been sent: check
  before resending" and is never resent automatically. Today
  `find_sent_copy`-based reconciliation assumes a server-side copy, so the
  delivery state machine needs an `Unverifiable` outcome for IMAP.
- **Threading:** replies already set `In-Reply-To` and `References`
  (`correspondence.rs`), so local threading places the sent copy in its
  thread.
- **Message-ID domain** (decided 2026-10-08): IMAP sends generate
  `<uuid@sender-domain>`, where the domain comes from the From address
  actually used, so it follows the identity choice. Today every send uses
  `@threestrands.local` (`correspondence.rs`). That domain is reserved and owned
  by no one, receiving spam filters can see it, and it reveals which client
  sent the message. Gmail sends can switch to the same rule later, since it's
  harmless there too. `find_sent_copy` compares the exact Message-ID value, so
  it needs no change. Content-IDs for inline attachments are internal to one
  message and keep `@threestrands.local`.

### Identities

The primary user sends from more than one Proton address over one Bridge login
(Bridge's combined-addresses mode). Each IMAP account therefore has an ordered
**identity list**: the login address first, then addresses the user adds in
account settings. Each identity has an address and an optional display name.
Nothing is discovered automatically, because IMAP has no standard way to list
a login's addresses.

- **Compose** shows a From picker only when the account has more than one
  identity. A new message starts with the first identity.
- **Replies and forwards** use whichever identity the original was sent to.
  The match is checked against `To`, then `Cc`, then `Delivered-To`/`X-Original-To`
  on the original message, comparing normalized addresses. If nothing
  matches, the account's first identity is used.
- **"Sent by me":** a message counts as yours for threading, the address book,
  Keep in Touch and `find_sent_copy` when its `From` is any identity on the
  account, not only the login address.
- **Rejection:** if SMTP rejects the sender (for example `553` or `550` on
  `MAIL FROM`), the error names the identity and suggests checking it in
  account settings. Rejection is a permanent failure and is never retried.
- The identity list is non-secret account settings, so it's included in
  settings transfer and replicated sync.

## Server search

Today the user's query is sent to Gmail as typed. IMAP needs the query parsed
first:

- **Parse once into a provider-neutral query** (`SearchQuery` AST): free-text
  terms, `from:`, `to:`, `subject:`, `before:`/`after:`, `is:unread`,
  `is:starred`, `label:`, `in:`, `has:attachment`. Gmail turns the AST back
  into Gmail syntax; IMAP turns it into `UID SEARCH CHARSET UTF-8 …`
  (`TEXT`/`FROM`/`TO`/`SUBJECT`/`SINCE`/`BEFORE`/`UNSEEN`/`FLAGGED`/`KEYWORD`).
- **Unsupported terms:** anything IMAP can't express, such as
  `has:attachment`, is applied to the local results only. The search reports
  the term as filtered locally instead of guessing on the server.
- **Mailboxes searched:** INBOX, Archive and Sent by default, plus user folders
  when an `in:` term asks for them. Results are capped and ingested through the
  existing `search_and_ingest_missing`.
- **Speed:** servers without a full-text index scan message bodies, so every
  search has a timeout and partial results are shown.

`server_search: true` for IMAP accounts.

## Account setup

1. The user enters an email address and password.
2. **Autodiscovery, in order, HTTPS sources only:**
   1. `autoconfig.<domain>`;
   2. `https://<domain>/.well-known/autoconfig/mail/config-v1.1.xml`;
   3. the Mozilla ISP database (also looked up via the domain's MX host);
   4. RFC 6186/8314 SRV records: `_imaps._tcp`, `_submissions._tcp`,
      `_submission._tcp`;
   5. as a last resort, guesses such as `imap.<domain>` and `mail.<domain>`.
      A guess is only accepted if its TLS certificate matches the host.
3. **Confirm before the password goes anywhere.** The user sees the discovered
   hosts, ports and security settings and confirms or edits them. Manual setup
   (host, port, implicit TLS or STARTTLS) is a first-class choice, not buried,
   because local bridges and self-hosted servers can't be discovered from the
   email domain.
4. **Untrusted certificate:** if the certificate doesn't verify, setup stops
   before `LOGIN`. It shows the certificate's subject, issuer and SHA-256
   fingerprint and offers "Trust this certificate for this server", with a
   prompt to compare the fingerprint against the server's own export, such as
   Bridge's "Export TLS certificates". The pin is stored per host and port with
   the server settings. If the certificate changes later, the account pauses
   until the user reviews the new certificate; it is never re-pinned silently.
   The pin covers one exact certificate. It never turns off hostname or
   validity checks for other hosts.
5. **Connection test:** IMAP `CAPABILITY`, `LOGIN`, `LIST`, and the
   special-use mapping. SMTP `EHLO` and `AUTH` without sending anything.
   Problems are reported in plain language: wrong password, app password
   required, certificate mismatch, port blocked.
6. **Save:**
   - The password goes in the keychain as a new
     `StoredCredential::ImapPassword { imap, smtp: Option<…> }` variant.
   - The non-secret server settings go in a new `imap_account_settings` table:
     hosts, ports, security mode, usernames, mailbox overrides,
     the Archive choice, `label_storage` and its container mailbox, the
     identity list, pinned
     certificate fingerprints and `server_saves_sent`.
   - The account row is created with `adopt_mail_account(email,
     MailProviderKind::Imap)`.

These hosts are entered by the user, not taken from message content, so the
`net_safety.rs` SSRF filter doesn't apply. Self-hosted servers on a LAN must
keep working.

### Changes to the step-1 seam

- **`AccountAuth`** gets an `Imap(ImapCredential)` variant.
  `AccountAuth::credential()` currently returns an `OAuthCredential`, so it
  becomes OAuth-only: an `Option`, or a match at its callers. IMAP accounts are
  set up with a "test and save" command, not the interactive browser
  sign-in.
- **`MailProviderKind`** gets an `Imap` variant.
- **`ProviderCapabilities`** gains `provided_threads` and a label-model flag
  (Gmail labels; IMAP folders plus keyword labels; IMAP folders plus
  label folders; IMAP folders with no user labels) so the frontend can choose
  its label actions.
- **`DeliveryReceipt`** and the delivery state machine get the `Unverifiable`
  outcome described above.

## Settings transfer and replicated sync

- **Export:** an IMAP account is exported with `provider: "imap"` and its
  non-secret server settings. Passwords are never exported. Importing an IMAP
  account on another device marks it `needs_reauth` until the password is
  entered.
- **Format version:** older builds reject `provider: "imap"`, so this bumps
  `transfer.rs` `VERSION` to 4. Versions 1–3 must still import, with
  regression tests for exports produced by version 3, as `AGENTS.md`
  requires.
- **Replicated sync:** the mail-account entity carries the same non-secret
  settings.

## Gmail assumptions to remove first

These were found during the step-1 survey and have to become provider-neutral
or capability-driven:

- **Interface text:** "Add a Gmail account", "Searching Gmail…", "Gmail search
  unavailable", "couldn't be applied in Gmail" (`SettingsPanel.tsx`,
  `App.tsx`).
- **`labels.ts`:** hides `Label_NN` IDs and assumes the `CATEGORY_*` labels
  exist. It needs a `folder` label kind and must respect each account's label
  model.
- **The split-inbox "label" rule** (`SettingsPanel.tsx`, `db.rs:2682`): it
  should offer keywords and folders for IMAP accounts.
- **The `connect_google` first-run path:** onboarding needs a provider choice.

## Testing

- **Unit tests** against a scripted fake `ImapSession`, run across a matrix of
  server capabilities: IMAP4rev1 only; plus `CONDSTORE`; plus `QRESYNC`; plus
  `MOVE`/`UIDPLUS`; without special-use attributes; without custom keywords.
  One profile copies the primary account exactly:
  `IMAP4rev1 IDLE MOVE UIDPLUS UNSELECT ID`, no `CONDSTORE`/`QRESYNC`,
  `SPECIAL-USE` not advertised but attributes returned by plain `LIST`, the
  `LIST` response above, `PERMANENTFLAGS ($Forwarded Forwarded \Deleted
  \Flagged \Seen)` (no custom keywords), and a self-signed certificate. Cases:
  - `UIDVALIDITY` changes;
  - another client moves, flags or expunges a message;
  - duplicate Message-IDs, and a missing Message-ID;
  - thread merges and alias resolution;
  - `TRYCREATE`, `AUTHENTICATIONFAILED` and `OVERQUOTA` responses;
  - the connection drops partway through `IDLE`;
  - a `UIDVALIDITY` reset keeps cached bodies;
  - a message in `INBOX`, an `\All` mailbox and two user folders becomes one
    message with two `folder:` labels and no `\All` label;
  - label-folder mode: adding a label copies into the container and handles
    `TRYCREATE`; removing one expunges only the copy's UIDs and leaves the
    INBOX location alone; a label added by another client is read as `lf:`;
    the container's children are left out of Move-to-folder; label removal is
    refused without `UIDPLUS`;
  - keyword mode is never chosen when `PERMANENTFLAGS` lacks `\*`;
  - `\Noselect` containers produce no labels, and their children do;
  - special-use attributes are honoured when the capability isn't advertised;
  - Archive choice at setup: `\Archive` used without asking, a name match
    preselected, and "Create `Archive`" as the default when nothing matches;
  - initial sync: all of INBOX; Sent stops at 5,000; other folders stop at the
    per-folder header limit (below, at and above each limit);
  - identities: reply identity chosen from `To`, `Cc`, then `Delivered-To`; a
    message from any identity counts as sent by you; a rejected sender is a
    permanent failure;
  - the generated Message-ID uses the domain of the chosen From address.
- **Integration tests:** Dovecot in Docker, with extensions removed through its
  `imap_capability` setting to cover the matrix, plus an SMTP sink such as
  Mailpit. They live in a separate cargo test target gated by
  `THREESTRANDS_IMAP_IT=1` and run in CI.
- **Security-negative tests:**
  - IMAP-ingested MIME goes through the same normalize, sanitize and proxy
    path;
  - passwords never appear in logs, errors or exports;
  - a missing STARTTLS fails;
  - a certificate mismatch fails;
  - an untrusted certificate fails until pinned, a pinned certificate
    connects, and a changed certificate on a pinned host fails;
  - autodiscovery never contacts a host over plain HTTP.
- **Settings transfer:** regression tests for importing a version 3 export,
  and a round trip of the identity list, Archive choice and `label_storage`.
- **End-to-end:** `pnpm test:e2e` covers triage on an IMAP-backed demo account:
  folder labels, keyword labels, label-folder labels, archive moving the
  message to the Archive mailbox, and a reply sent from a second identity.

## Phasing

1. **Finish the seam, with Gmail unchanged:**
   - an `AccountAuth` variant that isn't OAuth;
   - the new `ProviderCapabilities` flags;
   - the provider state store;
   - the `Unverifiable` delivery outcome;
   - the `SearchQuery` AST, with Gmail rendering it back to Gmail syntax.
2. **Read-only IMAP:** account setup, manual setup and autodiscovery,
   certificate pinning, the password credential, mailbox discovery,
   INBOX/Sent/Archive sync for all three capability tiers, local threading, and
   label construction. Also the `async-imap` spike.
3. **Changes:** flags, moves, keyword labels, label-folder labels (including
   the setup choice of container), move to folder, creating an Archive mailbox.
4. **Sending:** SMTP submission, identities and the From picker, Message-IDs
   on the sender's domain, `APPEND` to Sent, detecting `server_saves_sent`,
   uncertain-outcome handling.
5. **Server search:** turning the AST into IMAP `SEARCH`.
6. **Settings transfer and UI:** transfer format version 4, replicated sync,
   the provider picker, the folder label kind, provider-neutral text.
7. **Hardening:** the integration matrix in CI, connection limits, large
   mailboxes, then CalDAV as a separate design.

## Open questions

None right now. Decisions made while this note was being written:

1. **Server survey** (2026-10-08): capabilities, `LIST` and `PERMANENTFLAGS`
   are recorded in [The primary account](#the-primary-account).
2. **No Archive mailbox** (2026-10-08): an `\Archive` mailbox, else a
   confirmed name match, else "Create `Archive`" by default. See
   [Changes](#changes-archive-read-star-labels).
3. **Initial sync window** (2026-10-08): all of INBOX, the newest 5,000 Sent
   messages, and headers for the newest 2,000 messages in other folders, with
   no age cutoff. See [What gets synced](#what-gets-synced).
4. **Self-signed certificates** (2026-10-08): fingerprint pinning. See
   [Account setup](#account-setup).
5. **Aliases** (2026-10-08): the primary user sends from several Proton
   addresses, so each account has an identity list. See
   [Identities](#identities).
6. **Message-ID domain** (2026-10-08): the domain of the From address used.
   See [Sending](#sending).
7. **User labels without keywords** (2026-10-08): per-account
   `label_storage`, with label folders on the primary account. See
   [User labels](#user-labels).
