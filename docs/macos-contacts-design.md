# Read-only Mac Contacts

Implemented in 0.192.0 for macOS builds. ThreeStrands reads the macOS Contacts
framework, not iCloud's CardDAV server or Google's People API. The operating
system manages accounts and their network synchronization. This adds no Google
OAuth scope and stores no additional provider credentials.

## Connection and scope

**Contacts → Manage Contacts… → Mac Contacts…** reads authorization status
without prompting. **Allow Mac Contacts…** explicitly requests system permission.
The bundle declares `NSContactsUsageDescription`; an unbundled executable without
that description refuses the request rather than triggering macOS termination.
Denied/restricted access points users to System Settings. Limited access, where
available, is labeled and reads only the records macOS exposes.

Discovery lists containers without selecting any by default. Users choose the
address books to cache, keeping a noisy Google account separate from iCloud.
Container names come from macOS, which does not guarantee a provider label or
that every name is unique. Fetches disable contact unification so a linked record
cannot bring excluded accounts' fields into the selected container. Only names,
email addresses, organizations, and job titles are fetched. Phone numbers,
photos, birthdays, provider groups, and private contact notes are outside this
version's scope. Contacts without valid email addresses are omitted.

## Local cache and saved profiles

Schema 93 stores connection selections and source records outside the saved
contact tables. These records and settings are excluded from contact exports,
encrypted settings transfer, and replicated sync. Each Mac opts in separately.
Source IDs are device-local. Source-only profiles use `derived:mac:` IDs and
cannot be deleted or merged as saved contacts. They can be deliberately saved,
favorited, or added to a local group or reminder, creating an independent saved
profile under existing settings-transfer and encrypted-sync behavior.

Saved profiles take precedence over source records sharing an address. Refreshes
never overwrite a local name, notes, favorite, group, or reminder. Saving a source
profile is a snapshot: later Apple changes do not modify that saved profile.
Names alone never merge people. Source cards remain separate when two selected address books contain the same
person; autocomplete deduplicates email addresses. Ambiguous source ownership
is not combined into a new saved person automatically.

Autocomplete includes selected source addresses across mail accounts, deduplicates
by email, retains existing correspondent/favorite ranking, and honors the existing
device-local Never Suggest suppression. Source-only entries carry no fabricated
interaction date or favorite status. Browsing remains available for suppressed
addresses, and typing one explicitly remains possible.

## Refresh and disconnect

Manual refresh and a two-minute background poll replace successful snapshots
atomically. A transient error retains the last successful snapshot. Removed
source contacts disappear, without deleting local profiles or correspondence.
Operations are serialized, including permission requests, source changes, and
background refresh, so an earlier read cannot repopulate a disconnected cache.
Permission revocation clears cached source contacts on the next status check or
background refresh. Disconnect clears the cache and connection settings even
when system permission has already been revoked; saved profiles remain.

## Platforms and verification

Non-macOS builds return an unsupported capability and hide the connection entry.
Potential future adapters are Windows ContactStore, GNOME Evolution Data Server,
and direct CardDAV. Direct Google Contacts remains deferred during OAuth review.

Automated coverage uses injected address books and never reads the developer's
real contacts or displays a permission prompt. Native tests cover opt-in,
selection, cache replacement/deletion, suppression, saved-profile precedence,
and revocation. Component and browser tests exercise the connection, browsing,
and recipient-suggestion wiring. An installed signed build still needs developer
validation of the actual macOS consent dialog and configured account sources.
