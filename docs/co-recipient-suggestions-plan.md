# Co-recipient contact suggestions

**Status:** Proposed implementation; no application changes yet.
**Planned:** 2026-11-01 against 0.192.2.

## Outcome and scope

After selecting Alice in a message, contacts frequently emailed alongside Alice
move up the suggestions. Selecting Bob as well refines that order toward their
shared circle. The same behavior helps build named contact groups. It works
after one selection, with typed searches and with an empty focused input.

All additions remain explicit. No recipients, profiles, or group memberships are
created from a recommendation alone. Use local message headers, without message
bodies, AI requests, new provider permissions, or network lookups.

Initial scope is composer To/Cc/Bcc fields and the Groups view's Add Members
picker. Calendar invitees, automatic group creation, content-based predictions,
and contact relationship visualizations are deferred.

## Existing integration points

- `src-tauri/src/db/contact_suggestions.rs` merges correspondence, pins, saved
  profiles, and selected system contacts. It filters eligible addresses, sorts
  by pinned status, sent count, received count, and recency, then applies a limit.
- `src-tauri/src/db/contacts.rs::index_contact_message` records outgoing To
  interactions and eligible incoming senders. It recognizes account sending
  identities, but does not index outgoing Cc as contact interactions.
- `src-tauri/src/schema.rs` stores message To, separate Cc, and combined Cc/Bcc
  metadata. The combined field must not be used as visible-recipient evidence.
- `src/RecipientField.tsx` performs debounced lookups with request invalidation,
  excludes chips in its own field, and prioritizes explicitly matched groups.
  `src/Composer.tsx` currently supplies each field independently.
- `src/ContactGroupViews.tsx::ContactMemberPicker` uses address-book profiles and
  `FindOrCreatePicker`, with selection by profile ID or typed address. Groups and
  their candidate address book are shared across accounts.
- Commands, client contracts, and fixtures live in
  `src-tauri/src/commands/contacts.rs`, `src/domain.ts`, `src/data/client.ts`, and
  `src/data/demoClient.ts`.

## Product contract

1. Compose evidence comes only from the selected sending account. Changing From
   immediately invalidates results. Complete To/Cc recipients across the draft
   seed every field; Bcc recipients are excluded as seeds. All already selected
   addresses, including Bcc, are excluded from new suggestions. A Bcc input can
   receive recommendations based on the visible To/Cc set.
2. Groups use local evidence across connected mail accounts, matching the shared
   Groups view. Existing members plus pending selections seed recommendations.
   Explain that suggestions use email history across accounts. Unsaved typed
   addresses can seed a lookup without saving them before Add Members.
3. Name/address matching remains mandatory when text is present. Relationships
   never introduce unrelated text matches. Explicitly matched named groups keep
   their existing precedence. Pinned/favorite contacts retain priority within
   the eligible contact results.
4. Hidden addresses stay hidden in recommendations. Self addresses, malformed
   tokens, and already selected addresses cannot become recommended additions.
   Manual full-address entry continues to work, including for hidden addresses.
5. With no usable seeds or evidence, preserve the existing ordering and search
   behavior. Opening an empty focused input with seeds can show recommendations;
   do not open a list merely because an unfocused draft changed.
6. Show a brief reason such as “Often emailed with Alice” only when supported by
   evidence. Account data stays local, is not exported or replicated, and follows
   cached-mail deletion/retention. Limited cached history means limited evidence.

## Ranking design

Implement one pure Rust scoring helper with an injected clock. Proposed initial
constants: 365-day evidence window, 90-day recency half-life, at most 20 distinct
external visible recipients per message, and two distinct qualifying messages
for a relationship to receive a boost. Keep these named and centralized in the
ranking module, with boundary tests; they are tuning defaults, not settings.

Eligible evidence is delivered sent mail from a recognized account identity,
with a valid date, no draft status, and no bulk/unsubscribe indication. Do not
learn from incoming recipient lists, queued sends, failed sends, or Bcc metadata.
Extract and deduplicate To plus Cc, removing that account's sending identities.
Keep addresses distinct; do not infer aliases from similar names or domains.

For each seed/candidate pair, sum message weights
`2^(-age_days / 90) / (visible_recipient_count - 1)`. Count a duplicate address
once per message, cap repeated evidence to the highest weight per account,
thread, and UTC day, and reject messages above the recipient limit rather than
truncating them. Track distinct-message support separately from this cap.
Normalize each pair by the seed's total eligible message weight so a heavily
used seed does not dominate a group. Clamp future timestamps to age zero.

Rank eligible contacts by pinned status, number of seeds with supported
relationships, mean normalized relationship strength across all seeds, then
existing frequency/recency order. Add normalized email as a final deterministic
tie-breaker. This favors contacts connected to several selected people while
keeping unsupported candidates at their baseline order. Return bounded reason
metadata rather than raw scores to the UI.

Relationship candidates must enter the candidate pool before the result limit;
reranking only today's top eight would miss less-frequent but relevant people.
Compose ranks addresses. Groups map evidence through explicit profile addresses,
rank each profile once using its strongest address score, preserve its primary
address, and leave selected rows available for unselection.

## Implementation sequence

### 1. Add derived visible-recipient evidence

- Add a migration and normalized table keyed by account/message/address, with
  message foreign-key cascade and indexes supporting seed-to-message and
  message-to-recipient lookups. Store date/thread metadata or join those from
  messages; avoid materializing every pair, which grows quadratically.
- Populate it transactionally alongside message ingestion and updates using
  the existing sending-identity rules. Audit Gmail, IMAP, sent reconciliation,
  metadata backfill, and alias refresh paths. Replace a message's prior rows
  on reindex so changed headers cannot leave stale associations.
- Backfill cached sent messages in resumable batches away from keystroke and
  startup-critical paths. Read `recipients_json` and separate `cc_json`; never
  guess Cc from combined Cc/Bcc. Incomplete old headers provide partial evidence
  until normal metadata refresh supplies them. Do not change existing counts.
- Verify account removal, retention, and message deletion cascade; invalidate
  any derived query cache on those paths and identity changes. Initially prefer
  indexed reads without an additional persistent score cache.

### 2. Expose contextual ranking

- Extend compose suggestions with optional seed and exclusion lists; omitted
  context preserves the old API behavior. Validate/deduplicate inputs in Rust,
  bound list sizes and query work, and keep account authorization/selection explicit.
- Add a group-profile ranking command accepting existing member IDs, pending
  profile IDs, and pending typed addresses. Resolve profile addresses in Rust;
  return ranked profile IDs and reasons over the caller's candidate set. Keep
  filtering and existing group mutation APIs intact.
- Share scoring and suppression rules between both commands. Update typed client
  methods, domain models, demo implementations, and provider contract fixtures.
- Bound evidence work using indexed seed lookups and deterministic batch reads.
  If a safety bound is reached, return baseline suggestions without a misleading
  reason. Add a synthetic large-mailbox benchmark and inspect query plans before
  deciding whether a further derived aggregate is needed.

### 3. Wire composer and group picker

- Have Composer pass shared committed recipient context to RecipientField,
  using the existing quote-aware address parsers. Ignore partially typed names.
  Requery on seed/exclusion changes, account changes, removals, group expansion,
  and movement between fields. Preserve draft serialization and send behavior.
- Extract a focused lookup hook if needed. Invalidate in-flight requests as soon
  as query, context, or account changes, including during debounce and on clear.
  Empty-input recommendations must preserve Escape, focus, and explicit selection.
- Rerank Add Members candidates when existing/pending members change. Preserve
  the picker's search, exact-match handling, typed-address validation, selected
  tokens, and explicit submit. Keep generic FindOrCreatePicker changes minimal.
- Preserve the highlighted contact by identity when asynchronous ordering changes;
  if it disappears, reset deliberately. Errors retain ordinary search and manual
  address entry, clear stale relationship reasons, and follow existing logging.

### 4. Coverage, documentation, and release

- Extend Rust contact suggestion tests for one/multiple seeds, weak versus
  frequent unrelated contacts, intersections, recency, bulk limits, repeat-thread
  caps, exact date/count boundaries, deterministic ties, and no-evidence fallback.
- Cover To/Cc inclusion, Bcc exclusion, aliases/self, incoming and draft exclusion,
  duplicate indexing, separate accounts, malformed headers, hidden contacts,
  pins, saved/system contacts, and candidate discovery before truncation.
- Add migration/backfill/restart, update, deletion, retention, and account-removal
  tests. Use structurally different message fixtures; preserve existing contact
  counts and encrypted-transfer regression coverage.
- Test lookup races with renderHook and RecipientField directly. Extend focused
  group picker coverage (extract/export the picker if necessary) for ranking,
  multiple profile addresses, unselection, manual input, and failure fallback.
  Keep a slim Composer test proving shared To/Cc context and account changes reach
  lookups. Keep existing wiring assertions when moving tests to a lower level.
- Extend compose Playwright coverage for deliberate selection and recipient
  preservation. Run `pnpm test`, `cargo test` in `src-tauri`, `pnpm test:e2e`, and
  `pnpm build`. Run root `cargo test --workspace` only if `crates/` is changed.
  Do not start the app or use a browser for visual review; the developer owns it.
- Update contact/compose help and data-boundary documentation. Ship as a minor
  version from the then-current master version, synchronizing package.json,
  tauri.conf.json, Cargo.toml, and the threestrands Cargo.lock entry. Add a plain
  user-facing New entry under Unreleased and run the changelog structure test.

## Completion criteria

Both compose and group editing improve relevant suggestions immediately after
one selection and refine them with additional selections. Existing search,
favorites, suppression, keyboard interaction, group membership, and sending
contracts remain covered. No inferred relationship crosses the chosen account
boundary in compose or reveals historical Bcc participation. Cold start,
incomplete backfill, offline operation, and lookup errors retain useful baseline
behavior. Performance evidence demonstrates bounded indexed work without a
full-mailbox scan for every keystroke.

Implement this application work in a new branch/worktree from master. This
documentation-only planning task does not require a version bump or changelog.
