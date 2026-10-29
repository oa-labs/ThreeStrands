# Availability profiles design

Status: phase 1 (profiles and account defaults) built in 0.182.0. Phase 2
(using a different profile for one conversation) built in 0.183.0.

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

A profile has a stable `id`, a `name` (required, unique in any letter case,
trimmed, at most `MAX_AVAILABILITY_PROFILE_NAME_CHARS`), and the three
per-profile settings. Exactly one profile is the **default**: it's used when
an account has no profile of its own, and its hours are the existing
availability preferences, which is what older devices and exports read (see
Compatibility).

## Settings → Availability

With one profile, the tab looks as it does today. Below the meeting defaults,
an **Add Profile** button turns the tab into the multi-profile layout:

- **Profiles.** A segmented control at the top selects the profile being
  edited, with **Add Profile** beside it (disabled at the limit). Below it are
  the profile's **Profile Name** field and, for a profile other than the
  default, **Make Default** and **Delete Profile** buttons. The default
  profile can't be deleted. The working hours, **Default Duration**, and
  **Drive Time Buffer** below edit the selected profile.
- **Shared.** Timezone and **Slot Increment** stay where they were, each
  described as shared by every profile.
- **Account Defaults.** At the bottom, one select per mail account (nickname
  and address) listing the profiles. A new account starts on the default
  profile. This section appears when there are two or more profiles and at
  least one account.

Deleting a profile moves every account that used it to the default profile,
and the inline confirmation says which accounts will move. **Make Default**
changes the fallback, so accounts without a profile of their own switch to
the new default's hours; accounts that chose a profile keep it.

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
| Thread chat availability plans | The conversation's account |
| Places with no conversation or event | The default profile |

Because the timezone stays shared, AI requests that send only the timezone
(Brief, Find Times adjust in words, action analysis) don't change. A profile's
name is never sent to an AI provider.

## Using a different profile for one conversation

When there are two or more profiles, Find Times shows a **Usual Hours**
select beside the Any Time / Mornings / Afternoons chips, and the Proposed
Times card shows it beside the assumed timezone. Its description says the
choice lasts until ThreeStrands quits. Choosing another profile:

- re-runs the search with that profile's windows and buffer, keeping the
  current range and the Mornings/Afternoons and weekday chips. The length
  follows the profile only when nothing stated one: plans built from the
  profile's default carry `durationIsDefault`, and a length from the email,
  a suggestion, or an invitation stays;
- applies to every availability card in that conversation (Find Times,
  Proposed Times, Thread Assist), so the offered-time warnings and the slots
  agree;
- lasts for the conversation, for the rest of the session. It isn't saved and
  doesn't sync. Reopening the conversation after a restart goes back to the
  account default.

In compose, the same control sits on the compose sidebar's Find Times card and
applies to that draft. Changing From switches profile unless one was chosen
for the draft. Choosing the account's own profile again clears the choice, so
it follows the account from then on.

App keeps the choices in session state keyed by `thread:<id>` or
`draft:<id>`, and provides them through `AvailabilityProfileChoiceContext`
(`src/AvailabilityProfileChoice.tsx`): one provider for the open conversation
and a nested one around the compose sidebar. A choice that names a deleted
profile falls back to the account's profile.

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

// Stored beside AvailabilityPreferences, which keeps the shared timezone and
// slot increment and the default profile's hours.
type AvailabilityProfiles = {
  defaultProfile: { id: string; name: string };
  otherProfiles: AvailabilityProfile[];      // up to MAX_AVAILABILITY_PROFILES - 1
  accountDefaults: Record<string, string>;   // lowercased account email → profile id
};
```

- The default profile has no hours of its own in `AvailabilityProfiles`.
  Its hours, length, and buffer are `AvailabilityPreferences`, so there is
  never a second copy to drift. **Make Default** swaps the two profiles'
  hours and names; ids move with their hours, so accounts that chose a
  profile keep it.
- `AvailabilityPreferences`, the shape the native commands take, doesn't
  change. `availabilityWithProfile` combines the shared fields with one
  profile's hours, and `useAppPreferences().availabilityFor(accountId)` returns
  the result for an account's profile. Surfaces call it instead of reading
  settings directly. `MeetingAvailabilityPicker` and
  `CreateCalendarEventDialog`, which read storage directly, use
  `readAvailabilityPreferencesFor(accountId)`.
- Account defaults are keyed by lowercased email address, because account
  ids are email addresses and transfer merges accounts by email. An entry
  that names the default profile or a deleted profile is dropped on save. An
  entry for an account that isn't set up is kept and ignored.
- The limits `MAX_AVAILABILITY_PROFILES` (8, counting the default) and
  `MAX_AVAILABILITY_PROFILE_NAME_CHARS` (40) live next to
  `MAX_WORKING_WINDOWS_PER_DAY` in `src/settings.ts` and are mirrored in
  `src-tauri/src/availability.rs` for transfer validation.
- `availabilityFor` returns one object per profile, replaced only when
  availability settings change. The default profile's object is
  `availabilityPreferences` itself. Meeting cards re-check the calendar
  whenever the preferences object is replaced (see the comment in
  `useAppPreferences.ts`).

Native code: `availability.rs`, `proposed_times.rs`, and the calendar commands
still receive one `AvailabilityPreferences` per request. No Rust availability
logic changes. Only transfer and sync gain the new fields.

## Compatibility

### Local storage

No migration step is needed. `threestrands.settings.availabilityPreferences`
keeps holding the shared fields and the default profile's hours. With nothing
stored under `threestrands.settings.availabilityProfiles`, the user has one
profile named "Default" with those hours. The new key is written only once
there is something beyond that, so a downgrade keeps the user's default
hours.

### Replicated sync

Availability syncs as part of the portable preferences record. Older devices
know only `availabilityPreferences`.

- New devices keep writing `availabilityPreferences`, which is the default
  profile's hours, and write `availabilityProfiles` as a new field.
- When an older device edits its hours, only `availabilityPreferences`
  changes, so new devices see the default profile's hours change. Other
  profiles and the account defaults are left alone. No reconciliation code
  is needed.
- A record that omits `availabilityProfiles` (from an older device) keeps
  this device's profiles.
- Older devices keep the new field in the record without reading it. Their
  single set of hours is the user's default profile, which matches what they
  did before.

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
- Validation mirrors the frontend's: names and ids unique, names trimmed and
  within the character limit, each profile's hours valid under the shared
  timezone and increment, and account defaults lowercased and naming a profile
  other than the default.
- `v5-0.181.dispatch-settings` freezes the last export before profiles.

## Tests

- **Resolver** (unit and `useAppPreferences`): account default chosen
  regardless of letter case, unknown account falls back to the default
  profile, deleted profile falls back, shared fields always come from the
  shared settings, and the object stays the same until settings change.
- **Settings storage** (unit): existing hours become the Default profile;
  limits tested below, at, and above `MAX_AVAILABILITY_PROFILES` and the name
  limit; invalid hours and names rejected.
- **AvailabilitySettings** (component, `settingsHarness`): with one profile it
  shows no profile or account-default controls; add, rename, make default,
  and delete; the default profile can't be deleted; deleting a profile moves
  its accounts; the shared section's help is attached with
  `toHaveAccessibleDescription()`.
- **Phase 2, Find Times card / MeetingScheduler** (component): the profile
  control appears only with two or more profiles; switching re-runs with the
  other windows and keeps the range and chips; the conversation override
  reaches Proposed Times.
- **Phase 2, compose sidebar** (component): changing From switches profile,
  and an override survives a From change.
- **Transfer** (`cargo test` in `src-tauri`): a version 5 export without
  `availabilityProfiles` (produced by the preceding schema) imports as one
  Default profile; round trip with several profiles; validation rejects too
  many profiles, duplicate, untrimmed, empty, or overlong names, invalid
  hours, and account defaults that are uppercase or name an unknown profile.
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
