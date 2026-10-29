# Settings and account-list transfer

ThreeStrands moves configuration between desktop installations with a
password-encrypted settings bundle. This transfer includes a richer saved
address book than [CSV/vCard interchange](address-book.md#csv-and-vcard-interchange),
whose standard contact files are unencrypted.

## Included data

Current exports contain:

- appearance, reading, remote-image, selected-account, and non-secret AI
  preferences from the typed allowlist in `src/userPreferences.ts`;
- availability preferences and calendar colors;
- availability profiles and each account's default profile (omitted when
  only the Default profile exists, so such an export still imports into
  builds before 0.182.0);
- account email addresses, provider identifiers, sender names, nicknames, colors, and order;
- local Split Inbox definitions and reusable snippets;
- saved contact profiles, ordered linked addresses, photos, favorites, birthdays,
  and Keep in Touch settings;
- named contact groups and their membership;
- muted senders (an address, or everyone at a domain, per account); and
- the local mail-retention preference.

The import replaces preferences, Split Inboxes, snippets, and saved contacts.
It replaces groups and muted senders when the bundle includes them; an earlier
bundle without those fields preserves the destination's groups (before 0.79.0)
or muted senders (before 0.122.0). The device-local record of which messages a
mute already archived is never exported. Account metadata merges by
email address. An already connected account keeps its connection status; an
account present only in the export is created with `needs_reauth` and shown
as “Connect on this device.”

In Settings → Mail Accounts, an **Account nickname** such as “Work” or
“Personal” labels the account and supplies its icon's initial. Nicknames are
private to ThreeStrands and sync with the account catalog. **Sender name**
continues to control the name on outgoing mail. Clearing a nickname restores
the sender-name or email fallback.

## Excluded data and trust boundary

Exports never contain:

- Google OAuth access or refresh tokens, AI API keys, or other OS-keychain values;
- cached messages, attachments, search indexes, or correspondence history;
- unsaved mail-derived contact suggestions or the device's “never suggest” list;
- drafts, queued sends, or pending provider mutations;
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
Account `nickname`, added in 0.102.0, is optional and defaults to absent for
earlier exports. The frozen `v5-0.101` fixture covers the preceding schema.
Incompatible changes require an intentional version bump and continued import
support for earlier versions. Frozen historical fixtures cover those contracts.
CSV/vCard interchange, profile merging, and device-local suggestion suppression
in 0.83.0 do not change this encrypted format.

`emailMinimumFontSize`, added in 0.56, defaults to `0` (off) for earlier exports;
valid nonzero values are whole CSS-pixel sizes from 12 through 32. Contact
birthdays and Keep in Touch settings added in 0.67 default to absent/reminders
off. Calendar colors added in 0.73 default to the standard palette. Contact
groups added in 0.79 are optional so older exports preserve local groups.
