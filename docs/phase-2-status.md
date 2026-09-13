# Phase 2: correspondence implementation status

The correspondence portion of [the Phase 2 plan](phase-2-compose.md) is implemented. Broader Phase 2 features (snooze, reminders, snippets, Split Inbox) remain separate work.

## Available

- New-message composer, reply, reply-all, and forward, with To/Cc/Bcc, subject, plain-text body, and quoted source content.
- Drafts and Outbox views in the left navigation; saved drafts are local to this installation, not synchronized with Gmail Drafts.
- Native SQLite draft persistence with revision checks, 300 ms debounced autosave, visible save status, and save-before-close. Unsaved edits remain visible if a write fails. An abrupt crash can lose the explicitly unsaved interval.
- Sender identity retrieved from the connected Gmail account and cached for offline composition. The primary identity is supported; sending aliases are not yet supported.
- Reply metadata cached during synchronization, with on-demand retrieval for older cached messages. Replies use Reply-To and preserve threading headers; changing a reply subject detaches the explicit Gmail thread ID.
- A separate durable send outbox with immutable MIME snapshots, a 10-second undo window, atomic claim/cancel transitions, persisted attempt metadata, and prevention of duplicate queue submissions.
- A native send worker independent of the mailbox polling interval. It verifies the connected account before delivery and issues a single send request per claim. Explicit provider rejection can restore a draft; uncertain outcomes are reconciled against Gmail Sent and are never blindly retried.
- Restart recovery preserves drafts and queued work, gives interrupted undo windows a fresh grace period, and marks interrupted in-flight delivery uncertain. Normal close flushes drafts, keeps undo available, and waits for an active send acknowledgement before exiting.
- Native file selection and managed attachment copies, MIME multipart construction, removal, and on-demand retrieval of forwarded attachments. Unavailable attachments block Send until downloaded or removed. Copies persist independently of their original files. Limits are conservatively capped at 18 MB of files and 24 MB of encoded MIME.
- Shared command actions, modifier-aware shortcuts, compose focus handling, IME suppression, and accessible save/error announcements. Themes and the minimum supported window size are covered by browser checks.
- Unsubscribe metadata extraction from synchronized message headers, with confirmation, RFC 8058 one-click POSTs, and safe mailto/web fallbacks. Attempts are recorded locally; arbitrary URLs are never accepted from the webview.

| Action | Shortcut |
| --- | --- |
| Compose | `c` |
| Reply | `r` |
| Reply all | `a` |
| Forward | `f` |
| Refresh mail | `Shift+r` |
| Send from composer | `Cmd/Ctrl+Enter` |
| Save and close composer | `Escape` |
| Command palette | `Cmd/Ctrl+K` |
| Unsubscribe (when advertised by the message) | `Cmd/Ctrl+U` |

Attach files, Drafts, Outbox, and Undo Send are available in the command palette. Reply targets the latest displayed message. The native file picker is used from Rust; the webview does not receive arbitrary filesystem access.

## Verification and remaining release checks

Automated coverage includes native SQLite restart/migration tests, stale revision protection, quoted-name address parsing, reply-all recipients, forwarding headers, MIME attachment byte/name round trips, missing attachments, a controlled send clock and transport, cancel/delivery races, explicit rejection, transport uncertainty, and provider success followed by local acknowledgement failure. Browser tests cover offline composition, reload, sending/undo, keyboard isolation, forwarding, attachment controls, palette commands, and light/dark layout at 900×600.

The browser preview uses local fixture storage and simulated delivery/attachment metadata. It does not contact Gmail or deliver mail.

Before declaring the correspondence milestone ready for regular use, verify on a controlled Gmail account:

1. Compose and send only to an explicitly designated test recipient; inspect received To/Cc/Bcc behavior and message text.
2. Reply/reply-all and confirm Gmail threading, including edited subjects.
3. Forward and compare attachment bytes and Unicode filenames at receipt.
4. Save offline, quit/relaunch the native app, reconnect, send, and undo.
5. Exercise native file-picker cancel, normal window close and app quit, token expiry, and account disconnect/reconnect while queued.

These live/native UI checks have not been performed by the implementation agent; no real email was sent. The automatic tests use fixtures, temporary databases, and fake transport outcomes. Unresolved delivery should be checked in Gmail Sent before the user explicitly composes another message.

## Implementation notes

- Ordered SQLite migrations use `PRAGMA user_version`; existing mail and mutation tables remain intact. Raw source metadata is cached in a side table so legacy message rows do not need destructive rebuilding.
- The native correspondence module owns draft routing metadata and attachment locators; JavaScript can edit message fields but cannot replace sender, reply headers, or filesystem references.
- Completed outbox records and their attachment references are retained as local history. A user-configurable retention policy is part of later hardening.
- Gmail's existing `gmail.modify` permission supports the sending API; no additional OAuth scope is introduced.
- The MIME implementation uses `mail-builder` and `mail-parser`. See [Gmail's sending API](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.messages/send) and [threading guide](https://developers.google.com/workspace/gmail/api/guides/threads).
