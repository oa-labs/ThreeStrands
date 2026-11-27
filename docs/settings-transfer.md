# Settings and account-list transfer

ThreeStrands moves configuration between desktop installations with a
password-encrypted settings bundle. This transfer includes a richer saved
address book than [CSV/vCard interchange](address-book.md#csv-and-vcard-interchange),
whose standard contact files are unencrypted.

## Included data

Current exports contain:

- appearance, reading, remote-image, selected-account, and non-secret AI
  preferences from the typed allowlist in `src/userPreferences.ts` (plus
  optional `writing_profiles` when non-empty);
- availability preferences and calendar colors;
- availability profiles, each account's default profile, and meeting type
  profiles (omitted when empty/default-only, so such an export still imports
  into builds before 0.182.0);
- account email addresses, provider identifiers, sender names, nicknames,
  colors, order, and non-secret IMAP settings (`imap`, `imap_label_bindings`,
  `gmail_imap_converted`);
- local Split Inbox definitions (including `label_bindings`) and reusable
  snippets;
- signatures and signature defaults (`signatures`, optional for earlier exports);
- saved contact profiles, ordered linked addresses, photos, favorites, birthdays,
  and Keep in Touch settings;
- named contact groups and their membership;
- muted senders (an address, or everyone at a domain, per account);
- blocked senders (an address per account, with when its mail last arrived),
  in a separate `blockedSenders` field that is left out when nothing is
  blocked, so such an export still imports into builds before blocks; and
- the local mail-retention preference.

The import replaces preferences, Split Inboxes, snippets, and saved contacts.
It replaces signatures, groups, muted senders, and blocked senders when the
bundle includes them; an earlier bundle without those fields preserves the
destination's signatures, groups (before 0.79.0), muted senders (before
0.122.0), or blocked senders (before blocks, or when the exporting device had
none). The device-local record of which messages a rule already acted on is
never exported.

Account import uses a two-step preview (`preview_settings_import` followed by
`apply_settings_import(notHere)` or `cancel_settings_import`). Account metadata
merges by email address: an already connected account keeps its connection
status, accounts kept for setup on this device are created with `needs_reauth`
("Connect on this device"), and accounts the user places in `notHere` are
imported with `on_this_device = false` (available from another device) instead
of asking to reconnect here.

In Settings → Mail Accounts, an **Account nickname** such as “Work” or
“Personal” labels the account and supplies its icon's initial. Nicknames are
private to ThreeStrands and sync with the account catalog. **Sender name**
continues to control the name on outgoing mail. Clearing a nickname restores
the sender-name or email fallback.

## Excluded data and trust boundary

Exports never contain:

- Google OAuth access or refresh tokens, IMAP/SMTP passwords, AI API keys, or
  other OS-keychain values;
- cached messages, attachments, search indexes, or correspondence history;
- unsaved mail-derived contact suggestions, co-recipient ranking evidence, or the
  device's “never suggest” list;
- notes, note attachments, conversation snooze schedules, or calendar accounts
  and selections;
- drafts, queued sends, sender delivery-pattern confirmations, or pending
  provider mutations;
- tasks, goals, sync credentials, or device enrollment keys;
- crash reports, logs, telemetry identifiers, or transient window/layout state.

The React layer supplies only typed preferences. The trusted Tauri layer reads
native metadata from SQLite, encrypts and writes the bundle, decrypts imports,
validates every field, and applies native data transactionally. It never reads
or writes the keychain during transfer.

## Format and compatibility

Current exports use version **5**, with the JSON envelope marker
`dispatch-settings` and base64-encoded salt, nonce, and ciphertext. Imports
continue to accept versions 1–4 with compatibility defaults and migrations.
The `.dispatch-settings` extension retains the former product name; new
exports default to `threestrands-settings.dispatch-settings`.

XChaCha20-Poly1305 encrypts the payload. Argon2id derives the 256-bit key from
the password and a random 128-bit salt. Authentication failure is reported as
an incorrect password or damaged file without applying an import. Unknown
fields, unsupported values or versions, invalid limits, and oversized files
are rejected before SQLite changes.

Fields added under the same version require backward-compatible defaults.
Optional v5 fields include `aiFastModel` (0.53), `emailMinimumFontSize`
(0.56, defaulting to `0`), birthdays and Keep in Touch (0.67), calendar colors
(0.73), contact groups (0.79), account `nickname` (0.102.0), `emailZoom`
(0.163, defaulting to `100`), `emailTextColors` (0.165, defaulting to
`contrast`), writing profiles (0.171.0), availability profiles and meeting type
profiles, IMAP settings/label bindings, `showStarredInbox`, reading/snooze
toggles (`markReadOnArchive`, `markReadOnOpen`, `markReadOnTrash`,
`markReadOnSnooze`, `snoozeReturnsUnread`, `afterTriage`), and AI toggles
(`proactiveBriefs`, `threadChat`, `meetingAssistance`). Frozen fixtures under
`src-tauri/tests/fixtures/settings-transfer/` (`v5-0.101`, `v5-0.121`,
`v5-0.132`, `v5-0.140`, `v5-0.170`, `v5-0.181`, and `v5-0.211`) cover preceding
schemas alongside v1–v4 fixtures. Incompatible changes require an intentional
version bump and continued import support for earlier versions. CSV/vCard
interchange, profile merging, and device-local suggestion suppression in
0.83.0 do not change this encrypted format.
