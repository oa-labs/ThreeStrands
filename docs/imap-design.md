# IMAP/SMTP provider design

Status: proposed. This is the design note `PLAN.MD` asks for before IMAP/SMTP
work starts ("after its folder and threading behavior is designed"). IMAP now
comes before Microsoft 365. Microsoft 365 needs an Entra app registration, and
the primary user's mail is on a generic ISP/hosting IMAP server, not Microsoft
365.

## Decisions already made

| Question | Decision |
| --- | --- |
| Which servers | Generic ISP and hosting IMAP servers, whose extension support isn't known in advance. The design has to work on a plain IMAP4rev1 server and use newer extensions when a server advertises them. |
| Authentication | Password or app password only in v1, stored in the OS keychain, TLS required. OAuth2 (XOAUTH2/OAUTHBEARER) can be added later inside the same credential envelope. |
| Labels | System states use folders, user labels use keywords. Archive, Trash, Spam and Sent map to special-use folders. The user's existing folders appear as read-only locations with a Move action. ThreeStrands labels become IMAP keywords where the server allows custom keywords. |
| Calendar and contacts | Mail first. IMAP v1 covers mail and server search. CalDAV/CardDAV is a later step that needs a calendar provider interface. A Google Calendar connection still works alongside an IMAP mail account. |

## Guiding rules

1. **Act on capabilities, never on hostnames.** Behaviour is chosen from the
   server's `CAPABILITY` response, `PERMANENTFLAGS` and `LIST` attributes, never
   from the server's hostname or brand. This is the same rule as the
   email-rendering policy in `AGENTS.md`. Matching mailbox names such as
   "Sent Items" is allowed only as a fallback when a server lacks
   `SPECIAL-USE`, and the user can always override the result.
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
| IMAP client | [`async-imap`](https://crates.io/crates/async-imap) on tokio, behind an internal `ImapSession` trait | It is maintained by the chatmail/Delta Chat team, whose product runs against exactly this mix of ISP servers. The trait boundary lets tests use a scripted fake and keeps a swap to `imap-next`/`imap-codec` cheap if a spike shows gaps in `QRESYNC`/`VANISHED` handling. |
| SMTP | [`lettre`](https://crates.io/crates/lettre) (tokio, rustls) | Supports implicit TLS and STARTTLS, `AUTH PLAIN/LOGIN`, and XOAUTH2 for later. |
| MIME | `mail-parser` / `mail-builder` (already dependencies) | Reuse. Raw `BODY.PEEK[]` bytes are parsed into `RawMessage`. |
| TLS | rustls, with certificates always verified | The same stack `reqwest` already uses. Self-signed certificates are refused in v1 with a clear error (see open questions). |
| DNS SRV | `hickory-resolver` | Only needed for RFC 6186 autodiscovery. |

**Spike before committing (about a day):** against a Dovecot container, check
that `async-imap` can:
- `SELECT … (QRESYNC (…))` and parse `VANISHED (EARLIER)`;
- run `UID FETCH … (CHANGEDSINCE n)`;
- issue `MOVE` and `UIDPLUS` commands and return the new UIDs from their
  `COPYUID`/`APPENDUID` responses;
- keep `IDLE` running across reconnects.

Anything missing goes through its raw-command escape hatch or decides the
library choice.

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
| `SPECIAL-USE` (RFC 6154) | `\Sent`, `\Archive`, `\Junk`, `\Trash`, `\Drafts` come from `LIST`. | Common mailbox names are matched as a fallback. The user confirms the result at account setup. |
| `OBJECTID` (RFC 8474) | Message ID from `EMAILID`, thread ID from `THREADID` when the server returns it. | IDs are derived locally (next section). |
| `ESEARCH` (RFC 4731) | Search returns compact result sets. | Plain `SEARCH` results. |
| `LIST-STATUS` (RFC 5819) | One round trip for every mailbox's counters. | One `STATUS` command per mailbox. |
| `ENABLE`, `UTF8=ACCEPT`, `ID` | Turned on when offered. `ID` is always sent; some servers expect it. | Mailbox names are decoded from modified UTF-7. |

The **minimum requirement** is IMAP4rev1 over implicit TLS on port 993 or
STARTTLS on port 143. If the server doesn't offer STARTTLS on port 143,
connecting fails. There is no plaintext fallback, which closes off attacks that
strip STARTTLS.

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
    permanent_keywords INTEGER NOT NULL, -- PERMANENTFLAGS contains \*
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
| Keyword `foo` | `kw:foo`, a user label |
| Message is in user folder `Clients/Acme` | `folder:Clients/Acme`, a new `kind: "folder"` label |

A message stored only in the Archive mailbox therefore has no `INBOX` label.
The existing rule "archived = no INBOX" in `db.rs` works without changes.

`list_labels` returns keywords (`kind: "user"`) and folders
(`kind: "folder"`). The frontend shows folders as locations, not as labels that
can be toggled.

## Sync

### What gets synced

- **Mailboxes:** INBOX, Sent, Archive, Junk and Trash are synced continuously.
  Drafts are not, because ThreeStrands drafts are local. User folders are
  listed. Their headers sync inside the window below, and bodies are fetched
  when a message is opened.
- **Initial window:** in INBOX and Sent, the newest messages up to a limit on
  count and age. In other folders, headers only. Older mail is reached through
  search backfill, as with Gmail today. All of these limits go in one policy
  module, following the pattern of `emailRenderingPolicy.ts`, with tests just
  below, at and just above each limit.
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
| Spam / not spam | Move to `\Junk` and set `$Junk`, clearing `$NotJunk`. The reverse moves to INBOX and sets `$NotJunk`. The keywords help server-side spam filters learn. |
| Read / unread | `UID STORE ±FLAGS.SILENT (\Seen)` |
| Star | `±FLAGS.SILENT (\Flagged)` |
| Add or remove a user label | `±FLAGS.SILENT (keyword)`. If `PERMANENTFLAGS` lacks `\*`, fail with `InvalidOperation` and a clear message. The UI hides keyword labels for that account. |
| Move to folder (new action) | Move to the chosen mailbox. |

- **Idempotent:** each change first checks where the message currently is, so
  retries from the outbox do no harm.
- **New locations:** the new UID is recorded from `COPYUID` when available.
  Otherwise the next sync finds the moved message by its Message-ID.
- **No Archive mailbox:** account setup offers to create one, `Archive`. See
  open questions.

## Sending

1. **Submit over SMTP** with `lettre`:
   - Implicit TLS on port 465 is preferred (RFC 8314). STARTTLS on port 587
     is accepted but required; there is no plaintext fallback.
   - Same username and password as IMAP unless the user sets separate SMTP
     credentials.
2. **Save the sent copy** with `APPEND` to `\Sent`, flagged `\Seen`. With
   `UIDPLUS`, `APPENDUID` records the location immediately.
3. **Avoid duplicate sent copies:** some hosts save submitted mail to Sent
   themselves. After the first send on an account, look in Sent for the
   Message-ID. If it's already there, skip `APPEND` and remember
   `server_saves_sent` for that account. The user can override this in account
   settings.

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
- **Message-ID domain:** generated IDs use `@threestrands.local`. On SMTP that
  domain is visible to receiving spam filters, so consider using the sender's
  domain for IMAP accounts (open question). `find_sent_copy` would need to
  match the change.

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
   hosts, ports and security settings and confirms or edits them. An
   "advanced" form allows fully manual setup.
4. **Connection test:** IMAP `CAPABILITY`, `LOGIN`, `LIST`, and the
   special-use mapping. SMTP `EHLO` and `AUTH` without sending anything.
   Problems are reported in plain language: wrong password, app password
   required, certificate mismatch, port blocked.
5. **Save:**
   - The password goes in the keychain as a new
     `StoredCredential::ImapPassword { imap, smtp: Option<…> }` variant.
   - The non-secret server settings go in a new `imap_account_settings` table:
     hosts, ports, security mode, usernames, mailbox overrides and
     `server_saves_sent`.
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
  (labels vs. folders plus keywords) so the frontend can choose its label
  actions.
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
  `MOVE`/`UIDPLUS`; without `SPECIAL-USE`; without custom keywords. Cases:
  - `UIDVALIDITY` changes;
  - another client moves, flags or expunges a message;
  - duplicate Message-IDs, and a missing Message-ID;
  - thread merges and alias resolution;
  - `TRYCREATE`, `AUTHENTICATIONFAILED` and `OVERQUOTA` responses;
  - the connection drops partway through `IDLE`.
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
  - autodiscovery never contacts a host over plain HTTP.
- **Settings transfer:** regression tests for importing a version 3 export.
- **End-to-end:** `pnpm test:e2e` covers triage on an IMAP-backed demo account:
  folder labels, keyword labels, and archive moving the message to the Archive
  mailbox.

## Phasing

1. **Finish the seam, with Gmail unchanged:**
   - an `AccountAuth` variant that isn't OAuth;
   - the new `ProviderCapabilities` flags;
   - the provider state store;
   - the `Unverifiable` delivery outcome;
   - the `SearchQuery` AST, with Gmail rendering it back to Gmail syntax.
2. **Read-only IMAP:** account setup and autodiscovery, the password
   credential, mailbox discovery, INBOX/Sent/Archive sync for all three
   capability tiers, local threading, and label construction. Also the
   `async-imap` spike.
3. **Changes:** flags, moves, keywords, move to folder, creating an Archive
   mailbox.
4. **Sending:** SMTP submission, `APPEND` to Sent, detecting
   `server_saves_sent`, uncertain-outcome handling.
5. **Server search:** turning the AST into IMAP `SEARCH`.
6. **Settings transfer and UI:** transfer format version 4, replicated sync,
   the provider picker, the folder label kind, provider-neutral text.
7. **Hardening:** the integration matrix in CI, connection limits, large
   mailboxes, then CalDAV as a separate design.

## Open questions

1. **What does your server advertise?** The design works without knowing, but
   your server's capabilities decide which code path you'll use every day. To
   see them (the password is typed into the TLS session, not into your shell
   history):

   ```sh
   openssl s_client -quiet -crlf -connect imap.example.com:993
   a1 CAPABILITY
   a2 LOGIN "you@example.com" "app-password"
   a3 CAPABILITY
   a4 LIST "" "*" RETURN (SPECIAL-USE)
   a5 LOGOUT
   ```

   The `CAPABILITY` list after login is the one that matters.
2. **No Archive mailbox:** create `Archive` at setup (recommended), or let the
   user pick an existing folder?
3. **Initial sync window:** proposed 90 days or 5,000 messages for INBOX and
   Sent, whichever is smaller. Is that enough history for correspondence
   features like contact timelines?
4. **Self-signed certificates:** refuse them in v1 (recommended), or offer
   "trust this certificate's fingerprint" for self-hosted servers?
5. **Aliases:** do you send from more than one address on one IMAP login?
   That affects identity selection in compose and the `From` check in
   `find_sent_copy`.
6. **Message-ID domain:** switch IMAP sends from `@threestrands.local` to the
   sender's domain?
