# Availability profiles design

Today ThreeStrands has one set of availability preferences: a timezone,
working windows, a default meeting length, a slot increment, and a drive-time
buffer (Settings → Availability). Every surface that finds or checks times
uses it. That works for someone with one kind of life. It breaks for the
common case of one person with several roles: hours that suit work meetings
are the wrong answer when someone from church asks for a time on a personal
account, and the reverse.

This document describes **availability profiles**: named sets of hours ("Work",
"Personal", "Church"), a default profile for each mail account, and a way to
use a different profile for one conversation.

## Principles

- **Hours depend on the role. Busy time does not.** A profile decides when
  the user is willing to meet. It never decides which calendars count as
  busy: an elder meeting at 7 pm blocks a 7 pm work call no matter which
  profile is in use. The existing **Calendars used for availability** choice
  in Settings → Calendar Accounts stays global.
- **The account is a good guess, not a rule.** Board mail arrives at the work
  address, and a church member writes to whichever address they have. Each
  account chooses a default profile, and every card that uses one shows which
  and lets the user switch.
- **Always say which hours are in use.** When more than one profile exists,
  every card that finds or checks times names its profile. Choosing silently
  would be worse than having one set of hours.
- **One profile looks like today.** A user who never adds a second profile
  sees the same Availability settings and the same cards as before, with no
  new controls.

## What a profile holds

| Setting | Where it lives | Why |
| --- | --- | --- |
| Working windows | Per profile | This is the main reason profiles exist. |
| Default meeting length | Per profile | A church coffee is 60 minutes; a work check-in is 30. |
| Drive-time buffer | Per profile | Personal meetings tend to be in person; work ones tend to be video. |
| Timezone | Shared | Where the user is doesn't change with their role. |
| Slot increment | Shared | This is a display preference, not a matter of role. |
| Calendars counted as busy | Shared (per calendar account, as today) | See the first principle above. |

A profile has a stable `id`, a `name` (required, unique, trimmed, at most
`MAX_AVAILABILITY_PROFILE_NAME_CHARS`), and the three per-profile settings.
Exactly one profile is the **default**: it's used when no account default
applies, and its values are the ones mirrored into the legacy field for
older devices (see Compatibility).

## Settings → Availability

With one profile, the tab looks as it does today. Below the meeting defaults,
an **Add Profile** button turns the tab into the multi-profile layout:

- **Profiles.** A segmented control or list at the top selects the profile
  being edited, then that profile's name, working windows, length, and buffer.
  A profile's ⋯ menu has Rename, Make Default, and Delete. The default profile
  can't be deleted, and the last remaining profile can't be either.
- **Shared.** Timezone and slot increment move into a section labeled as
  applying to every profile.
- **Account defaults.** One row per mail account (nickname and address, as
  in Settings → Mail Accounts) with a select of profiles. A new account starts
  on the default profile. This section appears only when there are two or
  more profiles and two or more accounts.

Deleting a profile moves every account that used it to the default profile,
and the confirmation says which accounts will move.

The help text for this tab should say plainly that profiles change only
which hours are offered, and that every selected calendar still counts as
busy.

## Which profile a surface uses

Each surface chooses its profile from the mail account it belongs to. If an
override exists for the conversation, it wins; otherwise the surface uses
that account's default profile, or the default profile when there is no
account.

| Surface | Account that decides |
| --- | --- |
| Find Times card in the thread context panel and Thread Assist meeting suggestions | The conversation's account |
| Checks of times the sender offered (Proposed Times card, Brief offered times) | The conversation's account |
| Recipient and invitation sections (RecipientSections, InvitationSection) | The conversation's account |
| Compose context sidebar (ComposeContext) | The From account. Changing From switches profile unless the user has overridden it. |
| Meeting availability picker for an existing event | The event's calendar account, when it matches a mail account; otherwise the default profile |
| New calendar event dialog | The selected calendar's account, as above |
| Thread chat availability plans and other places with no conversation | The default profile |

Because the timezone stays shared, AI requests that send only the timezone
(Brief, Find Times adjust in words, action analysis) don't change. A profile's
name is never sent to an AI provider.

## Using a different profile for one conversation

When there are two or more profiles, the Find Times card's summary line gains
a profile control, for example
`Next week · Mornings · 30 min · Work hours ▾`. Choosing another profile:

- re-runs the search with that profile's windows, length, and buffer, keeping
  the current range and the Mornings/Afternoons and weekday chips;
- applies to every availability card in that conversation (Find Times,
  Proposed Times, Thread Assist), so the offered-time warnings and the slots
  agree;
- lasts for the conversation, for the rest of the session. It isn't saved and
  doesn't sync. Reopening the conversation after a restart goes back to the
  account default.

In compose, the same control sits on the compose sidebar's Find Times card and
applies to that draft.

The control is a plain select. It isn't an AI affordance and sends nothing
anywhere.

The override is deliberately short-lived. If users keep overriding the same
correspondents, that's evidence for a contact- or group-level default (see
Later), not a reason to persist overrides now.

## Data model

Frontend (`src/domain.ts`, `src/settings.ts`):

```ts
type AvailabilityProfile = {
  id: string;
  name: string;
  workingWindows: AvailabilityWindow[];
  defaultDurationMinutes: number;
  driveTimeBufferMinutes: number;
};

type AvailabilityProfiles = {
  timeZone: string;
  slotIncrementMinutes: number;
  profiles: AvailabilityProfile[];      // 1..MAX_AVAILABILITY_PROFILES
  defaultProfileId: string;
  accountDefaults: Record<string, string>; // normalized account email → profile id
};
```

- `AvailabilityPreferences`, the shape the native commands take, doesn't
  change. A single resolver, `availabilityFor(profiles, accountEmail,
  overrideId?)`, flattens the shared fields and one profile into
  `AvailabilityPreferences`. Surfaces call the resolver instead of reading
  settings directly. That includes `MeetingAvailabilityPicker` and
  `CreateCalendarEventDialog`, which call `readAvailabilityPreferences()`
  today.
- Account defaults are keyed by normalized email address, the same key as
  writing profiles (`writingProfileKey`), because account IDs differ between
  devices and transfer merges accounts by email. An entry for an account that
  no longer exists, or that points to a deleted profile, is ignored and
  pruned on save.
- New limits `MAX_AVAILABILITY_PROFILES` (proposed: 8) and
  `MAX_AVAILABILITY_PROFILE_NAME_CHARS` (proposed: 40) go next to
  `MAX_WORKING_WINDOWS_PER_DAY` in `src/settings.ts` and are mirrored in the
  native transfer validation, the way `MAX_WORKING_WINDOWS` is.
- The resolver's output is memoized per (profile, shared fields), so the
  object identity stays stable across renders. Meeting cards re-check the
  calendar whenever the preferences object is replaced (see the comment in
  `useAppPreferences.ts`).

Native code: `availability.rs`, `proposed_times.rs`, and the calendar commands
still receive one `AvailabilityPreferences` per request. No Rust availability
logic changes. Only transfer and sync gain the new fields.

## Compatibility

### Local storage

On first read, an existing `threestrands.settings.availabilityPreferences`
value becomes a single profile named "Default" with the same windows, length,
and buffer, and with the shared timezone and increment. The new value is
stored under a new key. The legacy key keeps being written (below), so a
downgrade keeps the user's default hours.

### Replicated sync

Availability syncs as part of the portable preferences record. Older devices
know only `availabilityPreferences`.

- New devices keep writing `availabilityPreferences` as the flattened
  **default profile**. They write `availabilityProfiles` as a new field.
- When a new device receives an `availabilityPreferences` change without a
  matching `availabilityProfiles` change (an older device edited its hours),
  it applies those values to the default profile and the shared fields. Other
  profiles and the account defaults are left alone.
- Older devices ignore the new field. Their single set of hours is the user's
  default profile, which matches what they did before.

### Settings transfer

`TransferPreferences` uses `deny_unknown_fields`. Follow the pattern used for
`writing_profiles` and `calendar_colors`: add `availability_profiles` as an
optional field under format version 5, with `#[serde(default)]` and
`skip_serializing_if` when there is only one profile.

- `availability_preferences` keeps its meaning and is always written as the
  flattened default profile.
- An export from a user with one profile is byte-compatible with today's
  format and still imports into earlier builds.
- An export with several profiles won't import into builds older than this
  feature. That's the same trade-off writing profiles made, and it doesn't
  need a version bump.
- Importing an export that has no `availability_profiles` (every earlier
  export) produces one "Default" profile from `availability_preferences`.
- Account defaults in an import are kept only for accounts present after the
  account merge.
- Update `docs/settings-transfer.md` ("availability preferences" becomes
  "availability profiles and account defaults").

## Tests

- **Resolver** (unit): account default chosen, override wins, unknown account
  falls back to the default profile, deleted profile falls back, and
  shared fields are always taken from the shared settings.
- **Settings storage** (unit): migration of the legacy value; limits tested
  below, at, and above `MAX_AVAILABILITY_PROFILES` and the name limit; legacy
  key mirrors the default profile.
- **AvailabilitySettings** (component, `settingsHarness`): with one profile it
  shows no profile or account-default controls; add, rename, make default,
  and delete; the default profile can't be deleted; deleting a profile moves
  its accounts; the shared section's help is attached with
  `toHaveAccessibleDescription()`.
- **Find Times card / MeetingScheduler** (component): the profile control
  appears only with two or more profiles; switching re-runs with the other
  windows and keeps the range and chips; the conversation override reaches
  Proposed Times.
- **Compose sidebar** (component): changing From switches profile, and an
  override survives a From change.
- **Transfer** (`cargo test` in `src-tauri`): a version 5 export without
  `availabilityProfiles` (produced by the preceding schema) imports as one
  Default profile; round trip with several profiles; validation rejects too
  many profiles, duplicate or empty names, and an unknown `defaultProfileId`.
- **Sync** (Rust and `useAppPreferences`): an `availabilityPreferences`-only
  update from an older device lands on the default profile without touching
  the others.
- **App wiring** (one slim `<App />` test): a conversation on an account set
  to "Personal" offers that profile's hours.

`pnpm test:e2e` isn't required. The change doesn't touch compose, triage, or
email rendering.

## Phasing

1. **Profiles and account defaults.** Data model, migration, transfer, sync,
   settings UI, resolver, and every surface switched over to the resolver.
   This is useful without an override: the personal account offers personal
   hours.
2. **Per-conversation override.** The profile control in Find Times and the
   compose sidebar, and conversation-scoped state shared by the thread's
   availability cards.

Both phases are new functionality, so each is a minor version bump with a
`### New` changelog entry (for example: "Keep separate availability for work
and personal life in **Settings → Availability**, and choose which one each
mail account uses when finding times.").

## Later, not in this design

- **Contact or group defaults.** A contact group ("Church") or a contact could
  choose a profile, which would take precedence over the account default.
  This fits the role idea better than accounts do, and contact groups already
  exist. Build it if users keep overriding the same people.
- **Roles beyond availability.** Writing profiles are per account today. If
  roles become a first-class idea, a role could pair an availability profile
  with a writing profile. Wait for evidence that writing by role is needed.
- **Per-profile busy calendars.** This is deliberately left out (see the first
  principle). A request for it is more likely a request for "don't show this
  calendar's events as busy", which belongs in Calendar Accounts.
- **Date overrides** ("away the week of the 10th") and **AI choosing the
  profile** from the conversation. AI selection would also have to follow the
  AI affordance rules and could only suggest a profile, with the user
  confirming.

## Decisions

Decided on 2026-10-29:

- The current hours migrate into a profile named **Default**.
- Account defaults live in Settings → Availability, in one section that lists
  every account.
- A per-conversation override doesn't survive a restart. Revisit only if
  users ask for it.
