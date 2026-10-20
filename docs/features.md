# Features and known limitations

This is the current reference for what the native app does, its keyboard
shortcuts, and the live-account checks still outstanding. It replaces the
Phase 1 and Phase 2 status trackers; the original Phase 2 plan is in
[the archive](archive/phase-2-compose.md).

Contacts, groups, and Keep in Touch are described in
[contact management](address-book.md). Sync, settings transfer, scheduled send,
and message rendering each have their own document in this folder.

## Accounts and providers

- Several mail accounts in one merged inbox, with a single-account focus, an
  account rail, per-account colors and order, and account shortcuts.
- **Gmail:** Google installed-app OAuth (Authorization Code with PKCE, a random
  loopback port, state validation, and browser handoff). Access and refresh
  tokens are stored only in the operating-system credential store
  (`app.threestrands.mail`), never in SQLite or the webview. Credential setup
  for builds is in [DEVELOPERS.md](../DEVELOPERS.md#gmail-oauth-credentials).
- **IMAP/SMTP:** password or app-password accounts, aimed first at Proton Mail
  Bridge. Reading, triage, labels, and sending with multiple identities work;
  server search and hardening are still in progress. See the
  [IMAP/SMTP design](imap-design.md) for status and behavior.

## Reading and triage

- Inbox, thread reader, and diagnostics, backed by a local SQLite cache with
  FTS5 search over thread metadata and cached body text.
- Optimistic archive, trash, read/unread, star, and label actions, with undo,
  including batch actions on a checked set of conversations.
- Durable mutation records with idempotent UUIDs. Identical pending changes are
  coalesced; interrupted `running` records return to `pending` on startup;
  acknowledged and rejected changes are recorded separately.
- Gmail sync: initial paginated import, then incremental History sync. An
  expired or invalid history cursor triggers a complete import; the cursor is
  captured before listing and history is replayed afterwards to close the
  snapshot race. Requests use bounded `Retry-After`/exponential backoff and
  recognize quota failures returned as HTTP 429 or 403.
- Label creation, rename, deletion, and per-thread application. Only
  user-created labels are toggled directly; system labels (`INBOX`, `TRASH`,
  `SPAM`, `UNREAD`, `STARRED`, `CATEGORY_*`, …) stay behind the dedicated
  archive, trash, star, and read actions.
- Adaptive background polling while connected, plus catch-up on startup and
  desktop resume.
- Sanitized HTML rendering in a sandboxed iframe; see the
  [email rendering policy](email-rendering-policy.md).
- Newsletter sender history with unsubscribe, archive, mute, and Split Inbox
  controls. Muted senders can follow you through encrypted sync.
- Local Trust checks explain suspicious sender names, lookalike domains,
  misleading links, and risky attachments. Show link reveals the link in the
  message; sender verification, linked sites, and tracking images are in Details.
- Inbox zero shows the account’s last-check status, open tasks, due Keep in Touch
  reminders, and today’s agenda when a calendar is connected.
- Opt-in crash reporting; see the [crash reporting policy](crash-reporting.md).

## Correspondence

- New message, reply, reply-all, and forward, with To/Cc/Bcc, subject,
  rich-text and plain-text bodies, and quoted source content.
- Formatted signatures with logos, a visual editor, Gmail import, and separate
  defaults for new messages and replies or forwards. The plain-text alternative
  can be edited independently. See the [writing guide](../site/docs/compose.md).
- Sent shows outgoing conversations across devices. Edit as New copies the
  subject, body, and attachments to a fresh message without recipients.
- Drafts and Outbox views. Outbox contains only mail that has not finished
  sending. Drafts follow you to your other devices through Replicated Sync
  (attachment files stay where they were added). Drafts on Gmail accounts are
  also pushed to Gmail Drafts after a 5-second pause, on composer close, and
  before quitting; drafts edited in Gmail are not imported yet.
- SQLite draft persistence with revision checks, 300 ms debounced autosave,
  visible save status, and save-before-close. Unsaved edits remain visible if a
  write fails. An abrupt crash can lose the unsaved interval.
- Sender identities are retrieved from the account and cached for offline
  composition. Gmail uses the primary address only (aliases are not yet
  supported); IMAP accounts have an identity list and a From picker.
- Reply metadata is cached during sync, with on-demand retrieval for older
  messages. Replies use Reply-To and preserve threading headers; changing a
  reply subject detaches the explicit Gmail thread ID.
- A durable send outbox with immutable MIME snapshots, a 10-second undo window,
  atomic claim/cancel transitions, persisted attempt metadata, and prevention
  of duplicate queue submissions.
- A native send worker independent of the polling interval. It verifies the
  account before delivery and issues a single send request per claim. Explicit
  provider rejection can restore a draft; uncertain outcomes are reconciled
  against Sent and are never blindly retried.
- Restart recovery preserves drafts and queued work, gives interrupted undo
  windows a fresh grace period, and marks interrupted in-flight delivery
  uncertain. Normal close flushes drafts, keeps undo available, and waits for an
  active send acknowledgement before exiting.
- Scheduled send with an explicit timezone, a fixed sending installation, and
  optional encrypted summary visibility on other devices. See the
  [scheduled-send design](scheduled-send-design.md).
- Native file selection and managed attachment copies, MIME multipart
  construction, and on-demand retrieval of forwarded attachments. Unavailable
  attachments block Send until downloaded or removed. Limits are 18 MB of files
  and 24 MB of encoded MIME.
- Unsubscribe from `List-Unsubscribe` headers, with confirmation, RFC 8058
  one-click POSTs, and safe mailto/web fallbacks. Attempts are recorded
  locally; arbitrary URLs are never accepted from the webview.

## Keyboard shortcuts

| Action | Shortcut |
| --- | --- |
| Next / previous conversation | `j` / `k` |
| Archive | `e` |
| Mark not done (return to inbox) | `Shift+e` |
| Toggle read | `u` |
| Star | `s` |
| Trash | `#` |
| Select for batch actions | `x` |
| Search | `/` |
| Compose | `c` |
| Reply | `r` |
| Reply all | `a` |
| Forward | `f` |
| Refresh mail | `Shift+r` |
| Send from composer | `Cmd/Ctrl+Enter` |
| Open Send Later picker | `Cmd/Ctrl+Shift+L` |
| Save and close composer | `Escape` |
| Command palette | `Cmd/Ctrl+K` |
| Unsubscribe (when advertised by the message) | `Cmd/Ctrl+U` |

This is a summary; the in-app shortcut help (`?`) lists every binding.
Buttons, shortcuts, and the command palette share one command registry. Send
Later, Attach files, Drafts, Outbox, and Undo Send are available in the palette.
Reply targets the latest displayed message.

## Known limitations

- Background polling runs only while the app is running. The app catches up on
  launch and resume; it does not deliver mail while suspended or closed.
- Gmail sign-in needs the system browser and a free loopback port. Credential
  storage needs a logged-in desktop keychain service.
- Composition is plain text. Gmail sending aliases and draft sync with Gmail
  Drafts are not supported.
- Completed outbox records and their attachment copies are kept as local
  history; there is no retention setting yet.
- The browser preview (`pnpm dev`) uses fixtures and simulated delivery. It
  never contacts a mail provider.

## Live-account release checks

Automated coverage includes native SQLite restart and migration tests, provider
contract suites, fault injection, MIME round trips, controlled send clocks and
transports, and Playwright keyboard, compose, and layout workflows. Checks
against real accounts are left to the developer and remain release gates until
recorded as done:

1. Connect a Google test account and confirm OAuth, initial import, and
   incremental sync.
2. Compose and send only to a designated test recipient; inspect received
   To/Cc/Bcc behavior and message text.
3. Reply and reply-all; confirm Gmail threading, including edited subjects.
4. Forward and compare attachment bytes and Unicode filenames at receipt.
5. Save offline, quit and relaunch, reconnect, send, and undo.
6. Exercise file-picker cancel, window close and app quit, token expiry, and
   account disconnect/reconnect while mail is queued.
7. Verify a scheduled send, sleep/wake and restart recovery, and read-only
   visibility on a second enrolled computer.

If a delivery outcome is unresolved, check the provider's Sent folder before
composing another message.

## Implementation notes

- Ordered SQLite migrations use `PRAGMA user_version`. Raw source metadata is
  cached in a side table so legacy message rows need no destructive rebuild.
- The native correspondence module owns draft routing metadata and attachment
  locators. JavaScript can edit message fields but cannot replace the sender,
  reply headers, or filesystem references. The native file picker runs from
  Rust; the webview has no arbitrary filesystem access.
- Gmail's existing `gmail.modify` scope covers sending; no additional OAuth
  scope is requested.
- MIME is built and parsed with `mail-builder` and `mail-parser`.


## Task, calendar, and file workspaces

- Tasks and Calendar use right-side details and context rails following the email
  panel’s typography, sections, rows, and keyboard conventions. Rails resize and
  collapse; narrow windows use an overlay. Task details save automatically.
- Calendar offers Day, 3-day, Workweek (default), and Week. Calendar navigation
  and selections are available from the Calendars control. Opening a weekend
  event temporarily uses Day when the saved preference is Workweek.
- Find Times searches beside the conversation, with optional AI writing help to
  interpret date requests. Select times to insert into a draft or start a reply.
  Working hours can have multiple blocks per day, and Drive Time Buffer reserves
  travel time around timed events with a location. Events support all-day dates
  and locations; briefs and chat can suggest each event announced in an email.
- Tasks and meetings surface related conversations and attachments from local
  mail without requiring AI. Direct sources come first; meeting suggestions
  require topic evidence and, when attendees are known, participant overlap.
- Press **5** to open Files. It includes attachments from synchronized received,
  sent, and archived mail, with All/Starred, filename search, sender, date, type,
  direction, and account filters. Inline resources, Spam, and Trash are excluded.
- Attachment stars are independent of email stars, persist on this device, and
  appear in Mail, Files, and contextual file rows. They are not synchronized or
  included in settings exports. Starring does not retain a separate offline copy.
- On macOS, Files previews safe documents and pictures. Risky attachments show
  warnings and are never previewed. Mark Spam removes the email’s files, with Undo.
- Files show known local availability and use the existing native open/download
  actions. Source-email navigation supports returning to the workspace selection
  and filters. Files cover mail synchronized on this device.

See [the implementation plan](workspace-context-and-files-plan.md) for scope,
acceptance criteria, and deferred work.
