# Cross-device sync

Three Strands is local-first and operates no sync backend. There is no Three
Strands account, sign-in, database, or API. Cross-device sync is an optional
beta that replicates end-to-end encrypted, immutable events through storage
the user chooses: a local sync folder (which may itself live in Google Drive,
Dropbox, iCloud Drive, OneDrive, Syncthing, and so on) or a user-configured
IPFS RPC endpoint.

## Turning it on

Sync is off by default. It becomes active when the user turns on beta
features in Settings (or, in development, when `THREESTRANDS_REPLICATED_SYNC`
is set). Until then, local mutations record nothing for replication.

A device replicates only after it is enrolled in a sync space: it starts a new
space (and is shown a recovery phrase once), is approved by an already
enrolled device, or joins with the recovery phrase.

## Data boundary

Replicated entities are tasks, snippets, Split Inboxes, mail-account display
metadata, calendar-account metadata and selections, retention, and the
portable preference allowlist. Device roster names are also synchronized so
each device has the same name in every roster; the hostname is used as the
initial name. Cached mail and bodies, attachments, drafts, queued mail,
provider mutations, OAuth tokens, AI API keys, crash reports, telemetry
consent, and device navigation state never leave the device. Storage
providers see only ciphertext.

Gmail and Calendar authorization remain separate per-device grants.

## Account removal

- **Disconnect** removes an account from this device only. While this device
  is enrolled, the account stays in the synchronized catalog (marked as
  needing reauthorization) so other devices keep it.
- **Remove from every device** records a synchronized deletion, pushes it,
  then removes the account locally. It requires an enrolled device.

Turning the beta off stops replication without deleting local data, keys, or
enrollment state.
