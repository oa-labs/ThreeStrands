# Persistent IMAP message identities

Implemented after the [identity audit](imap-identity-audit.md). This is the
message-identity foundation for [IMAP parity](imap-parity-plan.md), not account
conversion, All Mail support, or server-provided conversation grouping.

## Storage and authority

Schema migration 97 adds the device-local `imap_message_identities` table:

| Column | Meaning |
| --- | --- |
| `account_id` | The existing ThreeStrands account key |
| `namespace` | `gmail-message-id`, `objectid-emailid`, or `header-hash` |
| `external_id` | Canonical external token, or the existing account-scoped header hash |
| `message_id` | The stable ThreeStrands message ID |

The primary key includes all four columns. Several candidates can be retained
for the same external key, so contradictory observations are recorded instead
of silently overwriting evidence. An index supports reverse ownership checks.
All reads and writes are account-scoped.

`gmail-message-id` canonicalizes the wire's unsigned decimal integer to lowercase
hexadecimal without a floating-point conversion, matching Gmail API spelling.
Opaque EMAILID tokens remain case-sensitive and separate from Gmail IDs. Tokens
are bounded by `MAX_EXTERNAL_ID_BYTES` in the IMAP policy and reject whitespace
and control characters. Header hashes are weaker evidence and may have multiple
candidates; they do not establish an authoritative cross-provider match.

## Resolution

`ImapStateStore::resolve_message_identity` is the single transactional resolver
used by body acquisition and index-only ingestion. Server search acquires through
the same body-ingest routine.

1. Look up the exact mailbox/UIDVALIDITY/UID location and all supplied server IDs.
2. If these identify different internal messages, retain the conflicting
   candidates and return an error. Never return a guessed ID or change locations.
3. An exact existing location can learn additional identity namespaces. A second
   different ID in an already bound authoritative namespace is a conflict.
4. A known server mapping resolves a new folder location or a new UIDVALIDITY
   to the same internal ID, independently of header evidence.
5. For a new server identity, create a deterministic, namespace-separated internal
   ID. Two fresh devices assign the same message IDs even if duplicate sender
   headers arrive in opposite orders. Matching headers alone never attach the
   new server identity to legacy mail.
6. Without server IDs, retain ordinary IMAP's header-derived fallback, but stop
   when existing mappings show it is ambiguous or would impersonate a known
   authoritative message at a new location.

Reservations commit independently of a sync round, like reusable cached bodies.
They create no locations, advance no generation/cursor, and survive failed body
fetches or a process restart. SQL failures roll the whole reservation transaction
back. A conflict result commits its evidence before returning an error.
Body caching is consulted only after successful identity resolution.

## Protocol integration

Command sessions discover post-login capabilities. When `X-GM-EXT-1` is advertised,
the session adds `X-GM-MSGID` to both identity FETCH item lists. It does not add it
to body or flag requests and never requests it from a plain IMAP server.
Both header parsers retain the numeric ID returned by async-imap.

The current library still does not expose RFC 8474 EMAILID. The resolver supports
that namespace and tests learning it alongside Gmail IDs, but requesting/parsing
it on the live wire is deferred. X-GM-THRID and server conversation mappings are
also deferred; current header-based threading and alias migration remain in use.

## Existing installations and cleanup

The migration is additive and does not erase accounts, mail, tasks, or schedules.
Mappings are learned when identities are fetched; there is no eager legacy
backfill or automatic Gmail API-to-IMAP conversion. Existing exact locations keep
their IDs. If a new authoritative ID first appears at a different location,
legacy header matches are deliberately insufficient to merge it into old mail.
This can leave duplicate legacy/new occurrences until the account is rebuilt.
The current IMAP user has agreed they can start over if needed; no account is
automatically disconnected or reset by this change.

Mappings survive mailbox resets and body/window eviction. Removing/disconnecting
the account, or replacing its server connection namespace through the existing
settings path, clears them with its other IMAP caches. They are not settings
export fields or replicated entities, so transfer formats are unchanged.

SMTP receipts precede assignment of server IDs and retain their existing
header-derived delivery identifier. That value need not be the eventual synced
message's internal ID. Sent-copy verification already searches the raw message's
RFC Message-ID. No queued send is replayed or receipt rewritten by this migration.

Conflicts surface as sync/acquisition errors and remain recorded across restart.
A conflict-review UI and selective repair operation are future work. Current
recovery is rebuilding the affected IMAP account using existing account controls;
account removal still has the workflow-data consequences recorded in the audit.

## Tests and limits

Tests cover migration from v96, repeated opens, durable reservations without a
committed location, concurrent resolution, transactional rollback, account
isolation and cleanup, token boundaries, namespace learning, and durable conflicts.
Scripted-wire tests cover two distinct server messages reusing a Message-ID,
index-only copies, server-search acquisition, cached body reuse, database reopen,
UIDVALIDITY reset, and the plain-server fallback.

Distinct messages without usable server identifiers can still collide under the
legacy header/envelope fallback before ambiguity is observable. The original
collision fixtures remain characterization tests; this work does not claim to
solve indistinguishable plain-IMAP messages or migrate all workflow references.
