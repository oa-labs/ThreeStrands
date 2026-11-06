# IMAP identity audit and regression fixtures

Implementation follow-up: [persistent message identity mappings](imap-identity-mappings.md)
now provide the shared resolver and capability-gated X-GM-MSGID acquisition.
The tables below describe the audited pre-mapping snapshot and retain the
remaining account-conversion and conversation-mapping requirements.

Recorded 2026-11-06 against master `59325de7`. This is the identity foundation
for the essential IMAP parity work. It changes tests and documentation only;
it does not implement account conversion or new protocol extensions.
See [the IMAP design](imap-design.md) for the existing provider architecture.

## Current identity layers

| Identity | Current owner and representation | Lifetime and limitation |
| --- | --- | --- |
| Account | `accounts.email`; `db/accounts.rs` | Shared application account key. Provider is metadata on that row, not part of the key. Changing that metadata alone is not a conversion. |
| IMAP location | `imap_locations`: account, mailbox, UIDVALIDITY, UID | Identifies one location only. A move, folder copy, or UID reset can change it. Never use this as a workflow reference. |
| IMAP message | `identity.rs`, `fetch.rs` | `imap:<encoded-account>:<SHA256>` from normalized Message-ID, or Date/From/Subject/RFC822.SIZE when absent. The derivation function accepts EMAILID, but the live async-imap adapter does not request/expose it. |
| Sticky message mapping | `fetch::resolve_and_cache_body`, index path in `provider/refresh.rs` | An existing exact location mapping wins over newly derived identity. This does not protect a new folder location or a reset UIDVALIDITY from a future change of derivation. |
| Body | `db/imap.rs::BodyCache` | Keyed by account and stable message ID, reused across folders and flag changes. A hash collision can therefore also select the wrong cached content. |
| IMAP conversation | `threading.rs`, `imap_threads`, `imap_message_tokens` | `imap:t:<SHA256>` seeded from the first observed message/reference token. Headers connect replies; matching subjects alone never merge. Creation order chooses the survivor when a late message joins two existing threads. |
| Thread aliases | `imap_thread_aliases`, `db/imap.rs` | Persist old provider thread ID to survivor, scoped by account. Fetches resolve aliases; changed old and surviving threads reach ingestion. These are not a general message-ID or cross-provider alias system. |
| Application conversation | `db/threads.rs::local_thread_id` | `<account>:<provider-thread-id>`. This is the ID exposed to the frontend and most workflow records. Gmail and IMAP currently produce different values for the same conversation. |
| Gmail message/thread | `provider/gmail.rs` | Native Gmail API IDs. Gmail's advertised IMAP extension IDs require explicit representation conversion and account-scoped mappings; never assume wire strings are interchangeable. |

The header normalizer strips brackets, whitespace, and comments and lowercases
tokens. Preserve its existing output for stored IDs during this project. Do not
quietly revise normalization while adding extension support.

## Reference inventory and existing migration behavior

| Consumer | Stored reference and owner | Behavior today / conversion requirement |
| --- | --- | --- |
| Tasks | `tasks.thread_id`, `source_message_id`, `goal_id`; `db/tasks.rs` | Alias ingestion reparents task thread IDs in its transaction. Message IDs stay unchanged. No general message-ID remapping exists. |
| Goals | `goals.account_id`, parent goal ID; `db/goals.rs` | Goals do not directly reference mail. Preserve the account and task-to-goal links. Full account removal deletes account goals. |
| Snoozes | Application ID, provider thread ID, message IDs, header references in `conversation_snoozes.record_json`; `db/snoozes.rs` | Active schedules migrate on a thread alias; the old record is cancelled and a replacement is published. Header references aid recovery when IMAP locations are absent. An account conversion needs to reconcile every identity field, not only the row key. |
| Pending mutations | Application thread ID, optional target message ID, serialized mutation in `mutations`; `db/mutations.rs` | Claims join to the current thread for provider ID; missing threads make work orphaned. Alias ingestion does not rewrite this queue or its payloads. Conversion must pause delivery and migrate or explicitly resolve work. |
| Drafts | Draft UUID plus `source_id`, provider `thread_id`, RFC `reply_id`/`references`, attachment message/provider IDs; `correspondence/model.rs` | JSON payloads in `drafts` and `discarded_drafts` have no alias rewrite. Header reply references and application source IDs are different identity domains. Preserve both. |
| Server draft links | `draft_server_links`, `draft_server_deletions`; `db/server_drafts.rs`, `draft_push.rs`, `draft_pull.rs` | Provider draft IDs, versions, deterministic draft Message-IDs, and local draft UUIDs are distinct. IMAP draft IDs encode UIDVALIDITY:UID. Switching providers cannot reuse native draft coordinates. |
| Outbox and receipts | Draft snapshot, immutable raw bytes, operation UUID, provider receipt, scheduled-send and Sent-copy state in `outbox_messages`; `correspondence/service.rs`, `provider.rs` | JSON snapshots also carry source/thread identities. Do not regenerate bytes, replay completed sends, or interpret an uncertain delivery as unsent during conversion. |
| Reader and navigation | `readerPresentation.ts`, `readingPositions.ts`, `returnNavigation.ts`, `useReaderState.ts` | Device-local thread ID, active/latest message IDs, expansion entries and scroll anchor message ID. No backend alias migration updates localStorage. Conversion must supply mappings for these records too. |
| Attachments and contact indexes | `file_stars`, `file_attachments`, `message_metadata`, `contact_interactions`, co-recipient index; `schema.rs`, `db/ingestion.rs` | Several rows cascade when messages/threads are deleted. Alias ingestion explicitly retains attachment stars while reingesting messages. Rebuilding cached mail without a retention/mapping strategy can destroy local state. |
| AI, triage and automation | Thread-keyed analyses, triage records, unsubscribe source IDs, sender-rule action/source records, `agent_tasks.source_thread_id`; `schema.rs` | Treat cached analyses separately from durable user work; inventory and migrate source references before deleting old threads. Agent task sources can become NULL on thread deletion. |
| Labels and Split Inboxes | Account and provider label IDs; `db/split_inboxes.rs`, `labels.rs` | Label identities need their own mapping. Message/thread identity preservation does not fix renamed labels or old Gmail label IDs in rules. |
| Encrypted replication | `commands/tasks.rs`, `replicated_sync.rs`, `sync_projection.rs`, `db/sync_state.rs` | Task references, snooze schedules, account provider metadata, and rules are replicated application data. IMAP location/token/thread-alias tables are local provider state. Local SQL rewrites alone cannot coordinate conversion with another device. |
| Account removal | `commands/accounts.rs`, `db/accounts.rs` | Local disconnect clears provider caches; full removal also deletes workflow records. Neither is a migration primitive. Keep removal semantics separate from conversion. |

Existing alias ingestion is atomic and only retires old cached threads when a
surviving thread was fetched. It handles selected dependencies, not all rows in
this inventory. Task alias updates currently use direct SQL; publishing changed
replicated task references must be part of the conversion design. Snooze alias
updates already use their publishing path.

## Identity contract for subsequent stages

1. **Keep existing application message and conversation IDs stable.** New
   capabilities, a new folder, reconnect, or UID reset must not silently re-key
   known content. Do not turn X-GM-MSGID, EMAILID, or X-GM-THRID into replacement
   primary keys for known rows.
2. **Add external identity mappings inside the existing provider/database seam.**
   Design account + identity namespace + external ID to internal message/thread
   mappings. Namespaces distinguish Gmail API IDs, IMAP extension IDs, OBJECTID,
   and header evidence. Convert documented wire representations explicitly.
   Record new mappings even when an exact-coordinate sticky mapping wins, so
   future copies and UID resets can reuse the same internal identity.
3. **Require sufficient evidence before attaching a new mapping.** A repeated
   Message-ID or matching envelope is not proof that two different external
   messages are the same. Preserve conflicting candidates and expose unresolved
   workflow links. Do not choose the first candidate, match on subject, or delete
   work whose message cannot be uniquely identified.
4. **Keep external thread membership separate from workflow identity.** A server
   thread ID may help group messages, but merging existing internal conversations
   must emit durable aliases and migrate dependents atomically. Splits need an
   explicit policy; never move a task to an arbitrary child conversation.
5. **Keep the account stable during conversion.** Validate the new connection,
   retain old ID evidence, pause mutation/send dispatch, reconcile new identities,
   and commit a resumable conversion before retiring the old connection. Specify
   mixed-provider enrolled-device behavior before publishing provider metadata.
6. **Preserve existing formats.** New mappings can initially be device-local, but
   any replicated conversion coordination or transfer fields require compatible
   defaults/migrations and regression coverage for preceding formats. Do not add
   a required transfer field or change an existing field's meaning implicitly.

These decisions define the next implementation's constraints, not a schema
migration shipped by this audit. Before archive/extension implementation, specify
the mapping schema, atomic lookup/update path for both fetch tiers and server
search, collision state, alias publication, and upgrade/backfill behavior.

## Ambiguity and uncovered boundaries

- Distinct messages reusing a Message-ID currently derive the same identity.
  Two missing-ID messages with equal Date/From/Subject/size also collide even
  when their bodies differ. The fixtures characterize these limitations; they
  are not promises to preserve incorrect deduplication. Replace their
  characterization assertions deliberately when collision handling is added.
- An unchanged capability advertisement does not exercise extension ID parsing.
  The live identity fetch currently ignores both OBJECTID and Gmail identities.
  Capability fixtures ensure today's IDs remain stable, but do not prove that
  future extension enablement survives moves or UID resets. That requires the
  external mapping implementation and new upgrade tests.
- Headerless/duplicate-ID mail without a trustworthy external identifier may not
  be safely matchable across a reset or provider conversion. Preserve unresolved
  references; do not claim universal automatic recovery.
- Existing conversation aliases do not cover pending mutations, draft snapshots,
  frontend storage, or every replicated reference. Safe account conversion stays
  blocked on those migration policies even when ordinary sync tests pass.

## Regression fixture coverage

The reusable corpus is `src-tauri/src/provider/imap/fixtures/identity.json`.
It contains literal persisted message IDs and raw RFC messages: root/reply,
unrelated same-subject mail, a reply preceding its parent, missing Message-ID,
and two structurally distinct collision cases. Profiles describe plain IMAP and
optional extension advertisement; the scripted server really answers CAPABILITY.

| Coverage | Test owner |
| --- | --- |
| Literal persisted IDs and account-scoped ambiguity characterization | `identity.rs::tests` |
| Real wire header parser yields the same identity on body and index tiers under both profiles, with no body fetch | `provider/tests/identity_contract.rs` |
| Folder overlap deduplicates; move and UID reset survive closing/reopening SQLite and capability advertisement, without refetching known bodies; task source/thread links remain valid | `provider/tests/identity_contract.rs` |
| Reused UID under a new UIDVALIDITY cannot return its previous owner's body | `provider/tests/identity_contract.rs` |
| Same subjects stay separate; header-linked reply and late parent preserve conversation identity under both profiles | `provider/tests/identity_contract.rs` |
| Exact-coordinate sticky mapping wins over newly supplied EMAILID | Existing `fetch.rs::tests::a_sticky_id_is_reused_even_if_the_inputs_would_now_derive_differently` |
| Late merge reparents tasks without changing source message IDs; ingestion failure rolls back the old message/thread relationship | Extended `db/tests/ingestion.rs` |
| Snooze alias migration preserves message references; a later ingestion failure rolls back schedule changes | Extended `db/tests/snoozes.rs` |

Fixtures deliberately do not claim safe provider conversion, extension parsing,
draft/outbox migration, cross-device conversion, or frontend alias restoration.
Those are explicit remaining work, not behavior inferred from these tests.

Validation uses the full `cargo test` suite in `src-tauri`. In this Tart VM,
`Dispatch Dev Signing` is absent, so test execution uses
`CARGO_TARGET_AARCH64_APPLE_DARWIN_RUNNER=env cargo test` to run the test binaries
directly. No signing configuration or application behavior is changed. Local
documentation links and `git diff --check` are checked separately; no frontend
code, transfer schema, dependencies, or release notes change in this stage.
