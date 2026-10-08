# Contact management

ThreeStrands keeps a local address book alongside correspondence history. Open
**Contacts** from the sidebar (shortcut `4`). Profiles and groups are shared
across mail accounts; an account-scoped view shows people with correspondence
in that account, plus saved profiles with no correspondence yet.

## Profiles and correspondence

- Browse and search saved profiles and people you have emailed. Favorites come
  first, followed by recent activity. Recipient autocomplete also draws on
  eligible incoming mail and saved addresses.
- Create, edit, or delete a profile. Save a name, role, company, location, bio,
  notes, HTTPS links, photo, favorite status, and birthday with or without a year.
- Link multiple email addresses to one person. The first address is primary;
  you can reorder it. An address can belong to only one saved profile.
- Review correspondence, shared files, activity, and related context across
  the person's linked addresses. Deleting a profile leaves its mail intact;
  correspondence can make the person appear again as a mail-derived contact.
- Optionally ask AI to suggest missing profile details from correspondence.
  Suggestions include evidence and change a profile only when you apply them.

## Groups and keeping in touch

The **Groups** view manages named groups of saved people. Add members from
profiles, bulk selection, mail-derived contacts, or typed addresses. Adding an
unsaved person to a group saves a profile. Composer group suggestions expand
to each member's primary address, with duplicate recipients removed.

The **Keep in Touch** view combines recurring contact reminders and birthdays.
Set a frequency for a person or a selection of people, snooze a reminder, or
mark someone contacted. Mail in either direction across linked addresses and
accounts counts toward the reminder. Birthday-only profiles can appear without
a recurring reminder. These are local workflow reminders; they do not send mail.

## CSV and vCard interchange

Choose **Manage Contacts… → Import CSV or vCard…**. Save or discard open profile edits first. Select a UTF-8 `.csv`,
`.vcf`, or `.vcard` file and review the contacts, skipped records, and omitted
fields before importing. Canceling the review writes nothing.

- Files may contain at most 5,000 records and be at most 10 MiB.
- CSV accepts `Email` / `Email Address`, numbered email columns, common
  Google Contacts `E-mail 1 - Value` columns, and common name/profile headings.
  Quoted commas and multiline notes are supported. Export a CSV to see the
  supported column layout. Additional email columns preserve address order.
- vCard supports common 3.0 and 4.0 records, escaped text, folded lines,
  multiple addresses, and preferred addresses. Exports use vCard 4.0.
- Supported fields are name, addresses, role, company, location, bio, notes,
  HTTPS links, favorite status, and birthday. vCard location, bio, and favorite
  use ThreeStrands extension properties that other applications may ignore.
- Records without valid email addresses are skipped. A record sharing **any**
  address with a saved profile or an earlier imported record is skipped in
  full. Imports add profiles and never overwrite existing people. File IDs do
  not select existing profiles. Use an explicit merge afterward when needed.
- Unsupported fields and photos are omitted with review warnings. Remote photo
  URLs are never fetched. Invalid supported field values reject the preview;
  a persistence failure rolls back the accepted batch.

Choose **Manage Contacts… → Export CSV or vCard…** to export **all saved
profiles across accounts**, regardless of the current search or selection.
Mail-derived people who have not been saved are excluded. Standard files omit
photos, groups, Keep in Touch settings, and correspondence history. Files are
unencrypted. CSV formula-looking cells receive an apostrophe prefix for safe
spreadsheet use; that prefix is visible if another address book imports the cell.

Use [encrypted settings transfer](settings-transfer.md) to move the richer saved
address book, including photos, groups, birthdays, and Keep in Touch settings,
between ThreeStrands installations.

## Merging people

In **All Contacts**, choose **Select**, check 2–51 saved profiles, then choose
**Merge…**. Save or discard any open edits first. Choose which profile to keep
and confirm. Mail-derived profiles must be saved before merging.

The retained profile keeps its ID and primary address. All email addresses,
unique links, notes, bio text, favorite status, and group memberships are
combined; missing profile fields are filled. Conflicting names, roles,
companies, locations, and birthdays are appended to Notes. The retained photo
and configured reminder win; if absent, one is taken from another profile.
Additional photos and reminder configurations are discarded, as the confirmation
explains. The most recent logged contact time is retained.

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

There is no connected Google Contacts/CardDAV address book, ongoing provider
address-book synchronization, automatic duplicate-person detection, or reversible
merge history. Import/export and deliberate selection-based merging are available
today; connected address books remain roadmap work.
