# Native IMAP labels

Servers advertising `X-GM-EXT-1` now expose existing user labels in **Manage
Labels**, independently of the configured keyword/folder storage mode. Apply
and remove work, and new labels can be created and applied from the picker.
Rename and delete remain a separate parity stage.
Plain IMAP retains its existing label-folder behavior and keyword limitations.

The UI distinguishes toggling existing labels from changing the catalog. Native
accounts offer create and membership controls, with no rename or delete controls.
Ordinary IMAP folders stay read-only locations. Native user folders become
`user` labels while keeping their exact `folder:` or `lf:` IDs, preserving Split
Inbox and saved-label references. Display names decode modified UTF-7; protocol
arguments retain the catalog's wire encoding without double-encoding it.
System mailbox roles keep their dedicated actions and cannot be supplied as
arbitrary user-label writes. Advertised special-use views are excluded from
the picker. Native plans use attributes and explicit role choices instead of
name guesses, so ordinary user labels such as Archive or Projects/Sent stay
writable.

Schema 99 adds device-local negotiated capability state. It is not a setting,
credential, or settings-transfer field. Support is learned from authenticated
sessions, remembered across restart, and checked again on every write. Once
native semantics are observed, losing the extension disables label actions;
it never falls back to deleting folder copies. Pre-upgrade authoritative label
snapshots also retain this protection until capability negotiation succeeds.
Removing the account or replacing its server namespace clears this cache.

Creation accepts a full label path, including nested and non-ASCII names, without
prepending the configured label container. It validates controls, reserved names,
shared limits, and complete hierarchy segments using the live delimiter. A fresh
LIST reuses an exact existing selectable user label; otherwise one quoted,
modified-UTF-7 CREATE is sent. Another LIST must confirm a selectable user label
before its catalog row or return value is published. Failed/ignored creates,
missing verification, system views, and non-selectable containers cannot publish
success. Lost replies are safe to retry: the next listing can discover the
already-created label without issuing another CREATE. Parent creation is left
to the server's hierarchy semantics; no extra user labels are synthesized.

Native LIST parsing distinguishes quoted strings from literals before decoding
escapes. Correctly parsed IDs retain their existing format. See the
[focused label reference audit](imap-label-reference-audit.md) for the legacy
incorrectly escaped-name edge case and the required rename/delete foundation.
No schema, credential, or settings-transfer format changes are introduced by
creation. The existing plain IMAP folder-label creation path is retained.

Each mutation resolves an exact existing catalog label and each message's
unique server ID. It checks the label mailbox still exists, locates the message
by `X-GM-MSGID`, and selects under a UIDVALIDITY guard. The source used for a
removal must be independent of the label being removed, usually All Mail.
Only `+X-GM-LABELS` or `-X-GM-LABELS` changes membership. Names are quoted and
escaped as data; controls, reserved system names, and shared size limits are
validated before sending STORE. There is no COPY, MOVE, Deleted flag, EXPUNGE,
or whole-set replacement.

Read-back verifies the message identity, mailbox incarnation, requested
membership, and preservation of other labels. Verified membership, stale label
location removal, flags, and thread notifications commit together per message.
Lost responses and partial batches can be retried idempotently. A missing
independent location, ignored write, identity conflict, or changed UIDVALIDITY
cannot publish successful local state. Existing Inbox/All Mail membership
sweeps reconcile edits from other clients, and periodic LIST reconciles the
catalog; this does not introduce instant push updates for label changes.

Regression coverage includes ordinary and native accounts, both existing ID
forms, no-container settings, minimal and multipart/headerless mail, quoted and
nested names, modified UTF-7, repeated operations, external edits, lost replies,
ignored writes, capability loss, and UIDVALIDITY changes. UI tests cover
create/apply/remove and suppression of unsupported rename/delete actions. Database tests
cover capability isolation, restart persistence, cleanup, and upgrade from the
preceding schema without changing message identities or membership.

## Editing label hierarchies

Renaming a Gmail parent sends one IMAP RENAME. ThreeStrands records each
visible selectable descendant, confirms the destination of every renamed
label, and publishes all verified paths in one database transaction.
Split Inboxes and pending label changes keep their label IDs. Cached
message locations and search pins move only when the destination reports
the same UIDVALIDITY. A known child missing from LIST blocks the edit.
A lost rename acknowledgement leaves any vanished IDs available for
explicit relinking. A partial rename reports the incomplete result and
keeps verified bindings for labels that moved or stayed in place.

Deleting a Gmail parent sends one DELETE for the selected label. The app
checks that the parent has disappeared as a selectable label and that the
children are still present with their original IDs. The parent may remain
as a non-selectable container. Messages stay in Gmail, and a Split Inbox
that used the parent can be relinked. [IMAP mailbox commands](https://www.rfc-editor.org/rfc/rfc3501.html#section-6.3.4) · [Gmail IMAP extensions](https://developers.google.com/workspace/gmail/imap/imap-extensions#access_to_gmail_labels_x-gm-labels)

## Cross-device edits

ThreeStrands prefers an active device that introduced the account to
Replicated Sync as its initial editing device. Every active device
acknowledges the same assignment before ThreeStrands permits a rename or
delete. Older clients still read ordinary
synchronized account data. They need to update and sync before the editor
can start a structural edit. Conflicting acknowledgements leave structural editing unavailable.

Open **Settings → Mail Accounts → IMAP Editing Device** on the current
editor to hand the account to another enrolled desktop device. The old editor stops
issuing structural writes as soon as it saves the handoff. The new editor
learns its assignment through sync. An unfinished mailbox edit blocks
handoff until the current editor checks the server result. If the editor is
offline, it keeps its role. Transfer editing before removing that device
from Replicated Sync. Revocation without a handoff leaves structural editing
unavailable.

Verified paths and retired IDs reach peers in the encrypted Mail Account
record. Each IMAP peer checks the live path and UIDVALIDITY for its own
matching connection before attaching the old ID. A missing binding parks
queued label actions until the peer can verify a rename or learns that the
label was deleted. Other connection settings and provider-specific Split Inbox
selections stay local. External changes with insufficient evidence leave the
saved rule available for explicit relinking.

Plain IMAP uses the same journal for renaming physical folder trees. It
checks each selectable descendant and reports partial server results.
Deletion is available for individual leaf folders, subject to the existing
complete UID set and surviving-copy checks.

## Keyword edits

Keyword apply and remove support individual message selections. Account-wide
rename and removal need a resumable scan of every applicable mailbox, including
mail outside the local sync window. A rename must verify the replacement flag
before clearing the original flag on each message. Read-only mailboxes,
capability changes, and concurrent changes from other clients can leave work
incomplete. The current app leaves account-wide keyword edits unavailable.

Protocol references: [Google IMAP extensions](https://developers.google.com/workspace/gmail/imap/imap-extensions)
and [IMAP CREATE](https://www.rfc-editor.org/rfc/rfc3501.html#section-6.3.3).
