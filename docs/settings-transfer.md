# Settings and account-list transfer

Dispatch can move its configuration between desktop installations without
moving mail or credentials. Every export is password-encrypted; there is no
plaintext export mode.

## Included data

The version 1 payload contains:

- appearance, reading, remote-image, selected-account, and non-secret AI
  preferences from the typed allowlist in `src/userPreferences.ts`;
- account email addresses, display names, colors, and ordering;
- local Split Inbox definitions; and
- the local mail-retention preference.

The import replaces preferences and Split Inbox definitions. It merges account
metadata by email address. An account already connected on the destination
keeps its connection status; an account present only in the export is created
with `needs_reauth` and shown as “Connect on this device.”

## Excluded data and trust boundary

Exports never contain:

- Google OAuth access or refresh tokens;
- AI API keys or any other OS-keychain value;
- cached messages, attachments, search indexes, or correspondence history;
- drafts, queued sends, or pending provider mutations;
- crash reports, logs, or telemetry identifiers; or
- window geometry and transient layout state.

The React layer supplies only the typed preference object. The trusted Tauri
layer reads account and Split Inbox metadata from SQLite, encrypts and writes
the complete bundle, decrypts imports, validates every field, and applies the
native data transactionally. It never reads or writes the keychain during a
transfer.

## Format version 1

The file is a JSON envelope with the marker `dispatch-settings`, version `1`,
and base64-encoded salt, nonce, and ciphertext fields. The payload is encrypted
with XChaCha20-Poly1305. Its 256-bit key is derived from the user's password
using Argon2id with a random 128-bit salt. Authentication failure is reported
as either an incorrect password or a damaged file without attempting an
import.

The encrypted payload has its own version and a strict schema. Unknown fields,
unsupported enum values, invalid limits, oversized files, and unsupported
versions are rejected before SQLite is changed.

Changing the payload requires a new schema version and an explicit migration;
the application must not deserialize an old file directly into the current
database schema.
