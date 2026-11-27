# Contact management

ThreeStrands keeps a local address book alongside correspondence history. Open
**Contacts** from the sidebar (shortcut `5`). Profiles and groups are shared
across mail accounts; an account-scoped view shows people with correspondence
in that account, plus saved profiles with no correspondence yet. User
walkthroughs live in [the Contacts guide](../site/docs/contacts.md); this
document records data ownership, ranking, interchange, merge, and platform
boundaries.

## Profiles and correspondence

- Browse and search saved profiles and people you have emailed. Favorites come
  first, followed by recent activity. Recipient autocomplete also draws on
  eligible incoming mail and saved addresses.
- Create, edit, or delete a profile. Saved profiles store name, email addresses,
  phone number, time zone, role, company, location, bio, notes, HTTPS links,
  photo, favorite status, and birthday (with or without a year).
- Link multiple email addresses to one person. The first address is primary and
  can be reordered. An address can belong to only one saved profile.
- Inspect correspondence, shared files, **Meetings**, **Notes** mentions,
  activity, and related context across the person's linked addresses. Deleting a
  profile leaves its mail intact; correspondence can make the person appear
  again as a mail-derived contact.
- Optionally ask AI to suggest missing profile details from correspondence.
  Suggestions include evidence and change a profile only when you apply them.

## Groups and keeping in touch

The **Groups** view manages named groups of saved people and surfaces **Recent
Emails** across group members. Add members from profiles, bulk selection,
mail-derived contacts, or typed addresses. Adding an unsaved person to a group
saves a profile. Composer group suggestions expand to each member's primary
address, with duplicate recipients removed.

After you select a recipient in the composer, co-recipient suggestions prioritize
people you frequently email together. The focused field can suggest people
before you type; To and Cc supply context across fields, while Bcc recipients
do not seed recommendations. Everyone already addressed is excluded from
suggestions. Adding members to a group uses its existing members and pending
selections to prioritize related people from local history across accounts.
Suggestions never add anyone until you choose them and confirm the group change.

Relationship evidence uses visible To/Cc headers from cached sent mail. It stays
on this device, is excluded from exports and encrypted sync, and disappears
with its source mail under retention or account removal. Devices with different
cached history may suggest different people. No message bodies or AI requests
are needed; Bcc history is excluded.

The **Keep in Touch** view combines recurring contact reminders and birthdays.
Set a frequency for a person or a selection of people, snooze a reminder, or
mark someone contacted. Mail in either direction across linked addresses and
accounts counts toward the reminder. Birthday-only profiles can appear without
a recurring reminder. These are local workflow reminders; they do not send mail.

## Mac Contacts (macOS)

macOS builds read the system Contacts framework (`CNContactStore`), not
iCloud's CardDAV server or Google's People API. The operating system manages
accounts and network synchronization, adding no Google OAuth scope and storing
no additional provider credentials. Non-macOS builds return an unsupported
capability and hide the connection entry.

### Connection and scope

- **Contacts → Manage Contacts… → Mac Contacts…** checks authorization status
  without prompting. **Allow Mac Contacts…** explicitly requests system
  permission. The app bundle declares `NSContactsUsageDescription`; an unbundled
  executable without that description refuses the request rather than triggering
  macOS termination. Denied/restricted access points users to System Settings;
  limited access reads only the records macOS exposes.
- Discovery lists containers without selecting any by default so users can keep
  a noisy account separate from iCloud. Container names come from macOS, which
  does not guarantee a provider label or uniqueness.
- Fetches disable contact unification (`setUnifyResults(false)`) so a linked
  record cannot bring excluded accounts' fields into the selected container.
- Fetched fields: names, email addresses, phone numbers (`CNContactPhoneNumbersKey`),
  organizations, and job titles. Photos, birthdays, provider groups, and private
  contact notes are not fetched. Contacts without valid email addresses are
  omitted, and caching is bounded by `MAX_SYSTEM_CONTACTS` (100,000).

### Local cache, precedence, and refresh

- Schema 93 stores connection selections and source records outside saved
  contact tables. Settings and cached source records are device-local and
  excluded from contact exports, encrypted settings transfer, and replicated
  sync. Each Mac opts in separately.
- Source-only profiles use `derived:mac:` IDs and cannot be deleted or merged as
  saved contacts. Editing, favoriting, adding a reminder, or adding one to a
  group saves an independent ThreeStrands profile as a snapshot under existing
  settings-transfer and sync rules; later Apple changes never overwrite a saved
  profile.
- Saved profiles take precedence over source records sharing an address. Names
  alone never merge people; source cards stay separate across containers while
  recipient autocomplete deduplicates by email, retains correspondent/favorite
  ranking without fabricating interaction dates or favorites, and honors
  **Never suggest**.
- Manual **Refresh now** and a two-minute background poll replace successful
  snapshots atomically, retaining the last snapshot on transient error.
  Operations (permission requests, source changes, background refresh) are
  serialized behind `ACCESS: Mutex<()>` (`src-tauri/src/system_contacts.rs`) so
  an earlier read cannot repopulate a disconnected cache. **Disconnect Mac
  Contacts** and permission revocation clear cached source contacts and
  connection settings while leaving saved profiles and mail intact.

## CSV and vCard interchange

Choose **Manage Contacts… → Import CSV or vCard…**. Save or discard open
profile edits first. Select a UTF-8 `.csv`, `.vcf`, or `.vcard` file and review
the contacts, skipped records, and omitted fields before importing. Canceling
the review writes nothing.

- Files may contain at most 5,000 records and be at most 10 MiB.
- CSV accepts `Email` / `Email Address`, numbered email columns, common
  Google Contacts `E-mail 1 - Value` columns, phone columns, and common
  name/profile headings. Quoted commas and multiline notes are supported.
  Additional email columns preserve address order. Spreadsheet formula
  apostrophe prefixes on exported phone/text cells are reversed on import.
- vCard supports common 3.0 and 4.0 records, escaped text, folded lines,
  multiple addresses, `TEL` phone numbers, and preferred addresses. Exports use
  vCard 4.0.
- Supported fields are name, addresses, phone number, role, company, location,
  bio, notes, HTTPS links, favorite status, and birthday. vCard location, bio,
  and favorite use ThreeStrands extension properties (`X-THREESTRANDS-LOCATION`,
  `X-THREESTRANDS-BIO`, `X-THREESTRANDS-FAVORITE`) that other applications may
  ignore.
- Records without valid email addresses are skipped. A record sharing **any**
  address with a saved profile or an earlier imported record is skipped in full.
  Imports add profiles and never overwrite existing people. File IDs do not
  select existing profiles; use an explicit merge afterward when needed.
- Unsupported fields and photos are omitted with review warnings. Remote photo
  URLs are never fetched. Invalid supported field values reject the preview; a
  persistence failure rolls back the accepted batch.

Choose **Manage Contacts… → Export CSV or vCard…** to export **all saved
profiles across accounts**, regardless of the current search or selection.
Mail-derived people who have not been saved are excluded. Standard files omit
photos, groups, Keep in Touch settings, and correspondence history. Files are
unencrypted. CSV formula-looking cells receive an apostrophe prefix for safe
spreadsheet use.

Use [encrypted settings transfer](settings-transfer.md) to move the richer saved
address book, including photos, groups, birthdays, and Keep in Touch settings,
between ThreeStrands installations.

## Merging people

In **All Contacts**, choose **Select**, check 2–51 saved profiles, then choose
**Merge…**. Save or discard any open edits first. Choose which profile to keep
and confirm. Mail-derived profiles must be saved before merging.

The retained profile keeps its ID and primary address. All email addresses,
unique links, notes, bio text, favorite status, and group memberships are
combined; missing profile fields (including phone number and time zone) are
filled. Conflicting names, phone numbers, roles, companies, locations, and
birthdays are appended to Notes. The retained photo and configured reminder win;
if absent, one is taken from another profile. Additional photos and reminder
configurations are discarded, as the confirmation explains. The most recent
logged contact time is retained.

The other profiles are removed. Mail and attachments stay intact, and their
history becomes available under the combined addresses. A merge exceeding
profile limits fails without changing any profiles or groups. Changes and their
sync records commit together; peer devices apply removed profiles before
transferring their addresses to the retained profile.

## Never suggest an address

Choose **Manage Contacts… → Manage recipient suggestions…**. Hide one of the
open profile's addresses or enter another address. **Never suggest this address**
removes it from automatic recipient suggestions across accounts on this device.
It applies to saved, favorite, and mail-derived addresses and survives deleting
or re-importing a profile. **Allow suggestions** reverses it.

Hiding an address does not delete its profile or mail, hide it from Contacts,
block incoming messages, or prevent you from typing the address explicitly.
Deliberately selecting a group can still add it. These preferences stay on this
installation; they are excluded from contact files, encrypted settings transfer,
and cross-device sync.

## Storage and remaining boundaries

Profiles, photos, groups, and reminder settings live in local SQLite. Optional
[end-to-end encrypted cross-device sync](cross-device-sync.md) replicates saved
profiles and groups, including imports and merges. Correspondence history is
rebuilt from each device's mail cache; it is not contact sync data.

macOS builds can read selected system address books. Direct Google Contacts and
CardDAV connections, Windows/Linux system address-book access, automatic
duplicate-person detection, and reversible merge history remain future work.
