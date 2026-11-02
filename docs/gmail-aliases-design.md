# Gmail sending aliases

Implemented in 0.191.0. Gmail remains the authority for Send mail as verification,
default address, display names, and Reply-To destinations. The existing
`gmail.modify` grant authorizes read-only
[sendAs.list](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.settings.sendAs/list).
No settings-write scope, custom SMTP credentials, or alias-creation flow is added.

## Discovery and local data

A provider-neutral optional sync hook reads sending identities. Gmail refreshes
on the first mail sync and every fifteen minutes thereafter; a failed discovery
keeps the existing cache and does not fail the mail sync. Settings provides an
explicit refresh. Only the account's own primary address and custom aliases with
`verificationStatus=accepted` enter the allowlist. Entries are normalized for
comparison without stripping plus tags, deduplicated, and checked for header
injection. The primary identity stays first, preserving the existing provider
contract and primary account display-name preference.

Since 0.192.2, primary-only accounts show an **Aliases…** disclosure instead of
repeating the account address and default label. The disclosure keeps discovery
and manual refresh accessible. Accounts with aliases show the full list
automatically; a custom primary Reply-To remains visible when collapsed.

Schema 92 stores a device-local, account-scoped cache and refresh time. Aliases,
Reply-To addresses, and defaults are not imported from settings exports or
replicated sync; each installation discovers them from its own Gmail connection.
Removing the local account clears the cache. The identity picker, reply builder,
MIME builder, signatures, and sent-by-me classification use the same cache.

## Drafts, signatures, and delivery

A new message chooses Gmail's default accepted address. Replies choose the first
matching identity from the original To, Cc, Delivered-To, or X-Original-To headers,
using the existing provider-neutral reply selector. Continuing an own sent message
keeps its sending alias. Drafting and queueing remain
local, including offline. Changing accounts drops the old identity and uses the
new account's default. Choosing an address applies its automatic signature while
preserving manually chosen signatures.

MIME From and Reply-To are built from the allowed identity, not arbitrary draft
headers. Imported server drafts preserve their authored From even when it has
not yet been discovered or was removed; unknown addresses remain correctable,
and cannot pass the send allowlist. The composer displays a removed address as
unavailable rather than showing a different selected address.

Immediately before an outbox item obtains a delivery permit or is claimed, an
explicit Gmail alias is rechecked against the live verified list. A transient
failure remains queued under the existing retry policy; a definite removed alias
fails before any send attempt and remains restorable. Scheduled sends block when
verification fails. The primary-address path retains its existing behavior.
Uncertain-delivery reconciliation matches the actual selected From as well as
the operation Message-ID and SENT label; it never assumes the account address
was the sender or blindly retries an uncertain send.

## Verification

Coverage includes verified/pending/malformed alias discovery, account isolation,
cache replacement and refresh boundaries, offline identity reads, defaults,
addressed replies, per-address signatures, MIME From/Reply-To, removed identities,
pre-claim failure, own-address recognition, and imported server drafts. Composer
and settings tests cover selection, refresh, unavailable identities, and errors.
Browser tests cover selecting, saving, and reopening alias drafts. Live Gmail
account validation remains a developer release check.
