# IMAP label reference audit

Focused audit for native label creation and the subsequent rename/delete work.
Creation does not introduce a new label ID scheme, migrate references, or enable
rename/delete. See [native labels](imap-native-labels.md) for shipped behavior.

## Identity and reference owners

| Owner | Stored reference and source | Implication for rename/delete |
| --- | --- | --- |
| Provider catalog | `imap_mailboxes.name`; `plan.rs` derives `folder:<wire name>` or `lf:<relative wire path>` from settings. `native_labels.rs` resolves creation through the same plan. | A name change changes the current ID. A configured container also affects the ID; a display-name match is not identity evidence. |
| Application catalog | Account-scoped `account_labels.label_id`, populated by `db/labels.rs`, `SyncService::cache_label_change`, and `commands/mail.rs`. | Catalog replacement removes missing cache entries, not their dependents. Updating a returned Label alone cannot migrate references. |
| Thread membership | `threads.labels_json`; `db/mutations.rs` and mail ingestion maintain ID arrays. Provider membership comes from `labels.rs` and `refresh.rs`. | Rename needs coherent membership replay and optimistic mutation handling; deletion must remove only the label association. |
| Native server membership | `imap_label_snapshots.labels_json`, scoped by account/message, stores complete server label names. | These are wire names, not app label IDs. A rename changes both representations; an application alias alone cannot update server evidence. |
| Mailbox coordinates | `imap_locations.mailbox`, mailbox sync state, search pins, and copy intents in `db/imap.rs`; saved role/container choices in IMAP settings. | Parent rename can affect descendants and cached UID coordinates. Do not blindly reuse UIDs or clear body/message identities. Role and container settings need their own reconciliation. |
| Split Inboxes | `split_inboxes.match_value` when `match_kind=label`; `db/split_inboxes.rs` uses exact case-sensitive membership matching under the rule's account. | Preserve the rule ID, ownership, order, and label reference. Renaming the Split Inbox's display name does not change its match value. |
| Pending actions and undo | `mutations.payload_json` contains `ThreadMutation::Label.label_id`, alongside account and target-message coordinates. `threadMutations.ts`, `useThreadMutations.ts`, and `commands.ts` also capture label IDs in optimistic state and undo closures. | Pause or resolve in-flight actions during rename. Aliases must remain usable for queued/retried actions and undo created before the rename; changing only current UI state is insufficient. |
| Settings export/import | `TransferSplitInbox.match_value` in `transfer.rs`, written by `db/settings_transfer.rs`, preserves the raw account-scoped reference. | Keep prior exports importable. Any new mapping carried in exports needs a compatible default/migration and preceding-export fixtures; do not repurpose existing fields. |
| Encrypted application sync | Split Inbox records pass through `db/sync_state.rs` and `sync_projection.rs` with their match values. Mail catalogs and device-local IMAP coordinates do not supply a replicated label alias protocol. | A device-local rename map is insufficient: an older reference can arrive later from another device. Define conflict, alias retention, and mixed-version behavior before enabling rename. |
| UI catalog and recency | `useLabelCatalog.ts` caches returned IDs per account; `settings.ts` keys local usage timestamps by label ID. | Refresh visible catalogs and resolve old references. Usage history is best-effort, not authoritative identity. The existing 200-character ID validator can discard recency for long encoded IMAP IDs; retain this as follow-up work. |

Sender mute rules (`db/sender_rules.rs`) match addresses/domains and archive/read
mail; they do not store user-label IDs. Tasks, goals, snoozes, drafts, and reader
presentation primarily refer to messages/conversations, as covered in the
[message identity audit](imap-identity-audit.md). Do not conflate their identity
mapping with label identity migration.

The shared service's existing name byte limit and the IMAP policy's Unicode
character/wire-name limits are separate constraints. Creation retains both;
this audit does not broaden settings or parser limits.

## Creation contract and parsing correction

Creation uses the exact requested full server path, without prepending a label
container. Existing normal `folder:` and `lf:` IDs remain unchanged, including
children of an already configured container. A live exact-name match returns
that label, while a successful CREATE is published only after another LIST
confirms a selectable user mailbox. Case-folded or display-only matches are
not used to guess server identity. A failed verification leaves references and
the local catalog untouched; retry can discover a creation whose reply was lost.

The protocol tests found that the IMAP library retains quoted-string escapes
in LIST names. Native LIST parsing now decodes quoted names once and preserves
literal names verbatim, keeping endpoint names consistent with membership and
CREATE. It also preserves the advertised Important system-view attribute so
that view cannot be mistaken for an ordinary user label.

Compatibility edge: a preceding build may have cached an incorrectly escaped
ID for a label containing quotes or backslashes. Canonical parsing can expose
that label under its correct ID on the next catalog refresh. Existing saved
rules or queued actions using the malformed ID are not silently reassociated;
they may need manual reselection/reapplication. This is a correction to broken
name parsing, not a general rename migration. Ordinary, correctly parsed label
IDs are unaffected. Keep this case in the future alias/migration fixtures.

## Recommended rename/delete foundation

1. Introduce an account-scoped logical label identity with remote-name bindings,
   legacy ID aliases, and explicit unresolved/deleted states. Keep system roles
   separate. Design replicated/exported representation before choosing a schema.
2. Journal app-initiated rename across server and local state, including parent
   paths. Commit or resolve all reference owners together, with restart recovery
   and aliases for earlier queued actions. A returned Label is not a transaction.
3. Treat externally observed disappearance/appearance as ambiguous unless the
   server provides adequate identity evidence. Matching names, similar contents,
   or overlapping message memberships cannot prove a rename. Preserve unresolved
   Split Inbox references and offer explicit relinking rather than guessing.
4. Define delete semantics separately for native labels and ordinary IMAP
   folders. Native label deletion must preserve messages and unrelated labels;
   ordinary folder deletion needs proof it cannot destroy the last message copy.
   Preserve references as unresolved/tombstoned instead of silently retargeting
   them if another label later reuses the same name.

Required future coverage: nested/case/non-ASCII rename, external rename ambiguity,
same-name reuse, pending/retried mutations, undo captured before rename, parent
rename with search pins and stale UIDVALIDITY, interrupted recovery, mixed-device
Split Inbox updates, preceding settings exports, and ordinary-folder last-copy
protection. Rename/delete remain disabled until this contract is implemented.
