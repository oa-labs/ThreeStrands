# Repairing older Gmail conversations over IMAP

Gmail's IMAP extension supplies both the message ID and its conversation ID.
Older ThreeStrands caches grouped messages using headers. On normal sync,
ThreeStrands rechecks retained UID locations in small batches after the mailbox
walk. It requires the same UIDVALIDITY and compares the native message ID
against any previously verified binding. It never matches bodies, subjects, or
sender-authored headers to decide which conversation owns a message.

Each poll can repair one verified group. A split keeps an established native
binding when it has a matching member. For an unbound legacy group, the oldest
cached member whose Gmail conversation has no local binding can keep the old
thread ID. A whole merge into another conversation keeps an alias for the old
thread ID. An ambiguous split never gains a whole-thread alias.

The provider assignments and cached reader rows commit together. Message IDs
and attachment stars stay stable. Tasks and drafts with a verified source
message follow that message, and an unambiguous whole merge carries thread-only
references. Task changes use the existing replicated task write and leave the
task's other fields alone. Cached summaries are rebuilt from the corrected
grouping.

The repair leaves a group in place if some members have unavailable or
contradictory identities, any already-cached message is missing from the new
projection, or the change would widen the scope of a pending mail mutation, a
snooze, or an Outbox item. A split with several destinations and no surviving
source ID also waits when an older thread-only reference would become
unavailable. A bound ID with no verified matching member remains reserved for
its Gmail conversation. Grouping resumes with better evidence or when the
affected work settles. Ordinary IMAP accounts keep their existing threading.
