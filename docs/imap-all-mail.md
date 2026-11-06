# All Mail, archive, and search completeness

This implements stage 1 of [IMAP parity](imap-parity-plan.md), using the
[persistent message identity mappings](imap-identity-mappings.md). It does not
implement automatic account conversion, server-provided conversation grouping,
or general label management.

## Aggregate acquisition and state

The catalog's All Mail role is now a synced, label-free location. It uses the
existing folder acquisition window (`FOLDER_SYNC_WINDOW`) and the existing
chunk budget (`SENT_ROUND_MESSAGES`). Inbox runs first. All Mail fills over
bounded rounds using the same durable per-mailbox backfill watermark as Sent;
unlike Sent, its unpinned locations can age out of the folder window. Search
pins and active snoozes retain the locations they need under the existing
policies. Cached bodies and stable message identities are shared across copies.
Drafts and other aggregate views such as Flagged keep their separate handling.

When X-GM-EXT-1 is negotiated, both identity FETCH lists request X-GM-LABELS
alongside X-GM-MSGID. Flag sweeps also request label membership, without bodies
or headers. Standard IMAP quoted-string escapes are decoded, system names remain
reserved, and user-label names stay data. These attributes are never requested
from a plain IMAP server.

Schema migration 98 adds device-local `imap_label_snapshots`, keyed by account
and stable message ID. Complete membership, including an empty set, commits in
the same transaction as location changes, thread assignments, generation and
journal updates. A failed round cannot publish a false archive or lose its
label-change notification. Unchanged snapshots do not advance the generation.
Removing an account or replacing its server namespace clears snapshots with
the other IMAP caches; UIDVALIDITY resets and body eviction do not re-key mail.
No settings-export fields, encrypted transfer schemas, or credentials change.

Authoritative membership takes precedence over stale/unsynced folder copies.
It supplies Inbox/Sent/Trash/Spam/Draft state and user labels even when a
physical label folder has not been acquired. The existing thread-wide dynamic
label union remains intact. Inbox reconciliation also considers authoritative
membership learned through All Mail, rather than assuming every known Inbox
message has a local Inbox UID. It still requires a live message location.

Because label changes need not alter EXISTS or UIDNEXT, a negotiated All Mail
view sweeps its bounded window every poll. Cached bodies are reused. Generic
IMAP keeps its existing folder cadence. Incremental flag/label extensions and
performance benchmarking remain stage 4 work.

## Archive, undo, trash, spam and snooze return

For messages with authoritative label identities, the system-action path uses
only capability-gated additions/removals of Inbox, Trash and Spam membership.
It never replaces the complete label set, marks a copy Deleted, expunges mail,
or moves it to a synthetic Archive folder. Ordinary IMAP retains its physical
Archive/Trash/Junk move behavior and existing safety checks.

The native path addresses messages by their unique server ID, not by a sender
Message-ID or a cached UID. It checks the destination before writing, selects
under a UIDVALIDITY guard, and re-locates the destination after adding a label:
the source UID may have disappeared. Read-back must confirm the requested
state and preserve unrelated labels. Changed UIDVALIDITY, ambiguous identities,
missing metadata, or ignored writes stop the action. Confirmed location changes
and label state commit together, per message, so a lost reply can be retried
without duplicating or deleting content. Unarchive/Undo and snooze return add
Inbox membership through this same path.

Setup discovery supplies a runtime `labelArchive` hint only after capability
negotiation and an attributed All Mail mailbox are available. The mapping screen
then omits the unused Archive destination. Confirmation independently checks
the live capability and catalog before accepting an absent physical Archive
choice; a client hint alone cannot relax that validation. Generic accounts
still require their ordinary Archive mapping. This hint is not a setting or
transfer field, and older discovery responses default to the existing UI.

## Server search and partial coverage

Generic search now includes All Mail, Archive, Sent, user folders and label
folders, with Inbox first and default Trash/Spam exclusions. Location-only and
attachment-only searches use a bounded server scan; empty input is still
skipped. `in:anywhere` explicitly includes Trash and Spam.

If the session advertises X-GM-EXT-1 and has an All Mail view, the driver uses
X-GM-RAW. The full expression is one quoted IMAP string, with quotes and
backslashes escaped. Control characters and shared term/query limits are
checked before sending. The app's `in:archive` means not in Inbox, rather than
the name of a new label. Native search evaluates labels, boolean syntax and
attachments at the server. Trash and Spam are searched only when requested.

All paths preserve the existing deadline, acquisition budget and pin limits.
Excess mailbox targets now yield partial results with unsearched-folder counts
instead of dropping every server result. Search uses the background scheduler's
same mailbox eligibility cap, so it cannot pin a folder that will never be
maintained. Results deduplicate by stable message/conversation identity; old
All Mail hits acquired by search remain pinned across subsequent window sweeps.
Cached results continue to work offline.

## Coverage and remaining work

Scripted tests cover plain and extension-capable servers, archive-only mail,
overlapping copies with one body download, bounded import and Inbox progress,
old pinned search hits, inclusive mailbox/query limits, quoted labels and search
input, partial results, native system actions, snooze return, lost replies,
ignored writes, UIDVALIDITY changes, and both minimal and multipart/headerless
mail. Database tests cover atomic snapshot rollback, account isolation/cleanup,
and upgrades from the preceding schema. UI tests cover both setup modes; the
existing physical-folder mutation assertions remain in place.

Plain IMAP still relies on header-derived identity where no authoritative
identifier is available, with the limitations recorded in the identity audit.
An ordinary aggregate view is not treated as a safe destructive source mailbox.
General label creation/rename, native conversation grouping, conflict repair,
safe provider conversion, and broader incremental-sync optimizations remain
in the parity backlog.

Protocol reference: [Google's advertised IMAP extensions](https://developers.google.com/workspace/gmail/imap/imap-extensions).
