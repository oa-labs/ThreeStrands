# Cross-device sync

Three Strands is local-first and operates no sync backend. There is no Three
Strands account, sign-in, database, or API. Cross-device sync is an optional
beta that replicates end-to-end encrypted, immutable events through storage
the user chooses, called a **connector**. The devices that share that data form
a **sync group**.

## Turning it on

Sync is off by default. It becomes active when the user turns on beta
features in Settings (or, in development, when `THREESTRANDS_REPLICATED_SYNC`
is set). Until then, local mutations record nothing for replication.

## Connectors

A connector is where a device reads and writes the group's encrypted files.
Every device in a group uses the same connector: the same shared folder,
bucket and prefix, or endpoint. A device can have more than one connector.
Changes go to all of them, and existing changes are copied to a newly added
one automatically, so a second connector keeps devices syncing when one
provider is down.

| Kind | Where the data lives | What each device needs |
|---|---|---|
| **Shared folder** | A folder a sync app already replicates: Dropbox, iCloud Drive, OneDrive, Google Drive, Syncthing, and so on | The sync app, and its own copy of the folder |
| **S3 storage** | A bucket, with an optional folder prefix, on any S3-compatible service: AWS S3, Cloudflare R2, Backblaze B2, Wasabi, MinIO, and so on | The endpoint, region, bucket, prefix, and an access key |
| **IPFS** | A Kubo-compatible RPC endpoint, or one dedicated Filebase bucket per group | The RPC URL and, if the endpoint needs one, its access token |

Every kind stores the same bytes in the same layout
(`threestrands-sync/objects/…` and `threestrands-sync/heads/…`), so the files
can be copied from a folder to a bucket or back with ordinary tools. Storage
providers see only ciphertext: file sizes and timing, not contents. Three
Strands never treats one provider differently from another; the provider
choices in Settings only fill in the endpoint fields.

Connector settings are per device. They are never part of a settings export.
Secrets (S3 access keys and IPFS tokens) are stored only in the operating
system's keychain, never in the app database.

### S3 storage

- **Test connection** checks that the key can list, write, read, and delete
  under the prefix, and whether the prefix already holds a sync group. A
  connector can be added only after the current settings pass that test.
- The least a key needs is `s3:ListBucket` limited to the prefix, plus
  `s3:GetObject`, `s3:PutObject`, and `s3:DeleteObject` on `bucket/prefix/*`.
  Settings shows this policy ready to copy. Reading the bucket's versioning
  setting is optional.
- Endpoints must use HTTPS. Plain HTTP is accepted only for this computer
  (for example a local MinIO). IP-address and localhost endpoints need
  path-style addressing. Redirects are never followed.
- If the bucket keeps old versions of files, deleted and replaced files keep
  using storage until the bucket's lifecycle rules remove them.
- **Edit… → Replace credentials…** tests a new key against the same bucket
  before it replaces the old one.

### Removing a connector

**Disconnect and keep data** stops using a connector on this device. **Delete
files and disconnect** (shared folders and S3) also removes this group's files
from that storage. It does not erase copies elsewhere: other devices, provider
version history, or other connectors. Pinned IPFS data stays with the provider
until it is removed there.

## Joining a sync group

A device replicates only after it joins a group. On a device's first setup,
Settings offers:

- **Join with a code from another device** (fastest). On a device that already
  syncs, choose **Devices → Add a device**, then paste the code on the new
  device. The code sets up the same connectors and joins the group in one step.
- **Create a new sync group**, for the first device. The recovery phrase is
  shown once and never stored anywhere.
- **Join with the recovery phrase**, with no other device needed.
- **Ask another device to approve this one**. Both screens show a short code to
  compare before syncing starts.

The last three need a connector added first. Settings checks whether the
connector already holds a group and steers toward joining it rather than
starting a separate one.

Every join method gives the new device all of the group's keys, including
keys from before the group last changed them, so it can read the group's whole
history. A device that was offline while the keys changed also catches up on
everything it missed. Devices must all run 0.28.5 or later to join a group
whose keys have changed; an older app reports the invitation as damaged.

### Join codes

A join code is one pasteable string (it starts with `TSJOIN1-`) that carries:

- the connectors chosen when it was created, with their credentials unless
  **Include credentials** was turned off for a connector (a shared folder
  carries only its name, and the new device chooses its own copy);
- a one-time invitation to the group's keys.

**Anyone with a join code can join the group and use the credentials in it.**
Send it only to yourself, through a private channel. These safeguards limit the
risk:

- **Single use.** The inviting device admits the first device that redeems the
  code and refuses any later one. A refused attempt shows as a warning on the
  inviting device.
- **Expiry.** A code lasts 1 hour, 24 hours, or 7 days. Open codes are listed
  under **Devices → Join codes** and can be cancelled.
- **Key rotation.** When a code is used, expires, or is cancelled, the
  inviting device rotates the group's keys. A code that leaks later opens only
  data written before that point.
- **Tamper resistance.** The code names its invitation by content address. The
  new device verifies the invitation's signature and keys against the code, so
  a look-alike object planted in shared storage is never used.
- **Visibility.** Every device shows "*Name* joined with a join code from
  *Inviter*", with an option to revoke that device. The device list marks
  devices that joined this way.
- **No stored secrets.** The inviting device keeps only a record of the
  invitation, never the code text or its secret. The new device stores
  credentials from the code only in its keychain.

Credentials in a code don't expire with the code, so a key limited to the
group's bucket and prefix is safest.

A device that joins with a code can read synced data immediately. Its own
changes reach other devices once the inviting device finishes adding it, which
happens automatically the next time that device syncs. Until then Settings
shows "Waiting for *Inviter* to finish adding this device". Only the inviting
device can finish the join. If it doesn't sync again before the code expires,
use the recovery phrase or ask another device to approve this one instead.

A join code made by one version of Three Strands keeps working in later
versions. A code made by a newer version asks for this app to be updated.

### Groups from earlier test versions

Versions 0.29.0 and 0.30.0 each changed the sync format, and neither can read
or join a group made by an earlier test version. When it upgrades a device that belonged to such a
group, the device leaves it:
- Local data, connectors and the beta toggle stay.
- Settings shows a one-time notice explaining the reset.

To sync again, update every device first. Then, on each connector:
1. Delete the old group's files (**Delete files and disconnect**).
2. Add the connector again.
3. Create a new group on one device and join it from the others.

Settings recognizes a connector that still holds an old group. It won't
create or join a group there, and it explains what to do. A shared folder that
hasn't finished syncing a new group can look the same for a moment, so the
message also suggests waiting for the sync app and trying again.

For the same reason, a recovery-phrase join waits if the group's key changes
haven't all reached the connector yet. A device that still ends up missing a
key, for example because a key change happened just before it joined, gets it
from another device the next time both sync.

## How devices stay in step

Each device keeps one encrypted copy of its synchronized data on every
connector, and replaces it whenever that data changes. To catch up, a device
reads each other device's latest copy and merges it with its own. Nothing else
accumulates: however long a device has been away, it catches up by reading the
other devices' current copies. A connector holds about one copy per device,
plus the group's key changes and join records.

- **Edits on two devices at once** to the same field both stay until you pick
  one under **Resolve Conflicts**. Until then, every device shows the same
  one.
- **Deleted items** are removed from the copies. An old copy on a device that
  was away doesn't bring them back.
- **Editing an item on one device while it is deleted on another** keeps it
  deleted.

## Data boundary

Replicated entities are tasks, snippets, Split Inboxes, mail-account display
metadata, calendar-account metadata and selections, retention, and the
portable preference allowlist. Device roster names are also synchronized so
each device has the same name in every roster; the hostname is used as the
initial name. Cached mail and bodies, attachments, drafts, queued mail,
provider mutations, OAuth tokens, AI API keys, crash reports, telemetry
consent, and device navigation state never leave the device. Storage
providers see only ciphertext.

Gmail and Calendar authorization remain separate per-device grants. Connector
credentials travel only inside a join code the user creates and hands over
themselves, never through replicated data.

## Account removal

- **Disconnect** removes an account from this device only. While this device
  is in a sync group, the account stays in the synchronized catalog (marked
  as needing reauthorization) so other devices keep it.
- **Remove from every device** records a synchronized deletion, pushes it,
  then removes the account locally. It requires a device in a sync group.

## Leaving and turning it off

**Leave this sync group** (under Advanced) stops syncing on this device and
forgets its keys for the group. Local data and connectors stay, so the device
can rejoin later. Other devices keep listing it until it is revoked from one
of them.

Turning the beta off stops replication without deleting local data, keys, or
group membership.
