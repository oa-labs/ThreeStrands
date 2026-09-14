# Address book: a dedicated contacts management screen

Status: proposed. Extends the "address book" item listed under **Next** in
`PLAN.MD`. Builds on compose autocomplete, which is already implemented (see
below) — this doc scopes the standalone screen that's still missing.

## Why this exists

Compose already suggests recipients from local mail history and lets a user
pin favorites (see "Available today"). But there is deliberately no imported
Google address book — see the [original design
rationale](#original-design-rationale-no-imported-address-book) below — so
someone with **zero send/receive history** (a just-added family member, a new
hire, anyone you're emailing for the first time) never appears as a
suggestion, and the only way to add them today is the inline "Pin \<email\> as
a contact" row that appears in the compose dropdown once you've typed their
full address. That's a real but narrow fix: there is nowhere to *see* your
pinned contacts, edit a display name, remove one you pinned by mistake, or add
someone before you're mid-compose. This came up directly: a user typing their
spouse's address found it wasn't offered as an option, because there was no
history and no standalone place to add it ahead of time.

## Available today

Implemented as local-history compose autocomplete, not a management screen:

- `pinned_contacts` table (`src-tauri/src/db.rs:144`, `(account_id, email)`
  primary key) — manually favorited addresses that always outrank
  history-derived suggestions and survive with zero messages.
- `Database::list_contact_suggestions` (`src-tauri/src/db.rs:554`) — ranks
  past correspondents mined from this account's own cached `messages` (who it
  sent to, who it heard from), merged with `pinned_contacts`. Excludes senders
  whose message carries List-Unsubscribe/one-click metadata (bulk/automated
  mail) from the "heard from" side, unless the account also sent that address
  mail directly or pinned it. Bounded to the most recent 20,000 messages per
  account.
- `Database::pin_contact` / `Database::unpin_contact`
  (`src-tauri/src/db.rs:719`, `:740`) — add/remove a pinned favorite.
- Tauri commands `list_contact_suggestions`, `pin_contact`, `unpin_contact`
  (`src-tauri/src/lib.rs:413`, `:425`, `:437`), all scoped by `account_id` —
  see "Account scoping" below.
- `ContactSuggestion` type (`src-tauri/src/models.rs:165`,
  `src/domain.ts:90`) — `email`, `displayName`, `sentCount`, `receivedCount`,
  `lastInteractedAt`, `pinned`.
- `RecipientField` (`src/RecipientField.tsx`, 214 lines) — the To/Cc/Bcc
  combobox in `Composer.tsx`. Debounced suggestion queries as you type,
  arrow-key navigation, a pin/unpin toggle per suggestion row, and a synthetic
  "Pin \<email\> as a contact" row when the typed text is a complete address
  not already known — this is the one existing way to add a contact with no
  history.

### Account scoping

Every suggestion, pin, and unpin is scoped to `draft.account` (the identity
the draft is sending from), not global. Two connected Gmail accounts keep
fully separate pinned lists and history-derived rankings. A dedicated screen
must preserve this: it needs an account selector (or shows the currently
active account's book, matching whatever the rest of Settings does for
per-account sections), never a cross-account merged list.

## Gap: no standalone management surface

Concretely missing:

1. A place to **see every pinned contact** at once (today: only surfaces
   inline, one at a time, inside a compose dropdown).
2. A way to **add a contact before composing anything** — e.g. right after
   onboarding, or proactively adding a household/team address.
3. **Editing** a pinned contact's display name after the fact (today: the
   display name is set once, at pin time, from whatever was typed).
4. Seeing **why** something ranks the way it does (sent/received counts,
   last interaction) outside the compose dropdown's tooltip-free rows.
5. **Removing** a contact from being suggested at all — today `unpin_contact`
   only removes the pin; if that address also has real sent/received history,
   it keeps appearing (ranked lower). There's no "never suggest this address"
   affordance, which may matter for the automated-sender exclusion case (see
   above) that a user wants to override in either direction.

## Proposed UX

A new **Settings → Address Book** section, alongside Appearance/Reading/
AI/Privacy/Accounts (`App.tsx`'s existing Settings modal — see
`docs/multi-account.md`'s "Settings → Accounts" precedent for the sibling
section this should match in style).

- List every contact returned by `list_contact_suggestions("", <large limit>)`
  for the active account — pinned ones first (already the sort order the
  backend returns), then by sent/received history.
- Each row: display name (editable inline), email, a pinned toggle (reuses
  `pin_contact`/`unpin_contact`), and small sent/received counts + last
  contacted date for context.
- A "+ Add contact" row at the top: free-text name + email fields, calling
  `pin_contact` directly — the proactive add path that's missing today.
- A search/filter box for large histories (reuse the same prefix/substring
  matching `list_contact_suggestions` already does server-side).
- Optionally: a per-contact "never suggest" action for the removal case in
  gap (5) above — would need a new backend concept (e.g. a `blocked` flag
  alongside `pinned` in `pinned_contacts`, or a separate table) since nothing
  today models permanent suppression.

## Data model changes likely needed

- `list_contact_suggestions` currently defaults to a small `limit` (8, per
  `RecipientField`'s call) intended for a compose dropdown. A management
  screen wants the *full* list, not a top-N — either raise the cap, add an
  explicit "list all" mode (`limit: None` → no truncation), or paginate. Check
  `src-tauri/src/lib.rs:413`'s `limit.unwrap_or(8)` and
  `db.rs:554`'s `limit.clamp(1, 50)` — the current 50 ceiling would need
  raising or replacing with real pagination for a mailbox with hundreds of
  correspondents.
- Editing display name independent of pin state needs either a new command
  (`update_contact_display_name`) or extending `pin_contact` to upsert the
  name onto an already-pinned row without re-pinning semantics (it already
  does an `ON CONFLICT ... DO UPDATE SET display_name` per
  `db.rs:719`-area, so this may already work — verify before adding a new
  command).
- "Never suggest" (gap 5) needs a new column or table; don't build it until a
  concrete request confirms it's wanted, per the project's general bias
  against speculative schema.

## Open questions

- Should the Address Book screen show contacts that have real history but
  were never pinned (i.e. today's full `list_contact_suggestions` output), or
  only explicitly pinned ones, with unpinned-but-frequent contacts staying
  compose-only? Leans toward showing both, clearly distinguishing "pinned"
  from "frequent," since hiding real history from a "contacts" screen would
  be confusing.
- Any import path (e.g. a one-time CSV import) for users migrating from
  another client, or is "type it once during compose" sufficient forever?
  The original design deliberately avoided importing Google Contacts (data
  quality — "thousands of junk entries" was the motivating complaint); a
  manual CSV import is a much narrower, user-controlled version of the same
  idea and may be worth revisiting only if requested.
- Does removing an account's `pinned_contacts` rows belong in
  `Database::remove_account` (`db.rs`) alongside `triage_events`? Check
  current behavior before shipping the management screen — an orphaned
  pinned-contacts row for a removed account should probably be cleaned up the
  same way.

## Original design rationale: no imported address book

When compose autocomplete was first built, the explicit choice was to mine
local send/receive history (plus manual pins) rather than import Google's
People/Contacts API, because a typical Google address book accumulates
thousands of low-quality entries (one-off senders, mailing lists, autofilled
junk) with no reliable signal for "would I actually want to email this
person again." Local history is inherently high-precision: every suggestion
is someone the account has actually corresponded with. This document's
proposed screen keeps that constraint — it's a management UI over the same
locally-derived data, not a door to importing an external address book.
