# Existing native IMAP labels

Servers advertising `X-GM-EXT-1` now expose existing user labels in **Manage
Labels**, independently of the configured keyword/folder storage mode. Apply
and remove work; create, rename, and delete remain a separate parity stage.
Plain IMAP retains its existing label-folder behavior and keyword limitations.

The UI distinguishes toggling existing labels from changing the catalog. Native
accounts offer a find-only picker, with no create, rename, or delete controls.
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
apply/remove and suppression of unsupported catalog actions. Database tests
cover capability isolation, restart persistence, cleanup, and upgrade from the
preceding schema without changing message identities or membership.

Protocol reference: [Google IMAP extensions](https://developers.google.com/workspace/gmail/imap/imap-extensions).
