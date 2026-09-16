# Multi-account plan: connecting more than one Gmail account

Status: proposed. Extends the "multiple accounts and unified inbox" item already
listed under **Next** in `PLAN.MD`.

This delivers a single milestone: connect a second (and third, ...) Gmail
account without re-authenticating the first, see one merged inbox by default,
narrow to a single account in one keystroke, and never send a reply from the
wrong identity.

## UX design

### What leading clients do

| Client | Switching model | Unified view | Visual distinction |
| --- | --- | --- | --- |
| Gmail (web/mobile) | Avatar menu; switching fully reloads the mailbox | Only on mobile, as an opt-in "All Inboxes" | Colored avatar per account |
| Outlook | Every account's folder tree sits in the left rail at once | Focused Inbox unifies *within* one account, not across accounts | Icon per account in the rail |
| Superhuman | `Cmd+K` fuzzy "Switch to ..."; keyboard only | None — strictly one account at a time | Small avatar chip |
| Spark | Smart Inbox merges every account by default | Yes, on by default | Colored dot per message tied to its source account |
| Mimestream (Gmail-only, native Mac) | Sidebar section per account, plus a unified smart mailbox | Yes, opt-in alongside per-account view | Color stripe per account |

Dispatch's own thesis (`PLAN.MD` §2.1–2.3) is speed and *focused triage inside
one view*, not an extra layer of per-account chrome. A Gmail/Outlook-style full
context switch — reload the whole app state to look at a different mailbox —
fights that. Spark's and Mimestream's default-unified inbox fits better: one
merged, keyboard-triaged view, with per-account scoping as a fast filter
rather than a mode change. Superhuman's instant, keyboard-only switching is
worth borrowing for the moments a single account *is* wanted (composing from a
specific identity, triaging one client's mail before a call).

### Concrete UX

1. **Account switcher.** The sidebar brand button (`App.tsx:665`, currently a
   plain "D" that opens Settings) becomes a popover: connected accounts as
   colored initial circles with email and unread count, "All accounts" pinned
   and active by default, "+ Add account" at the bottom running the existing
   Continue-with-Google flow. With one account connected the popover still
   exists but has nothing to add, so the rail stays visually identical to
   today.
2. **Unified inbox by default.** `list_threads`/`search_threads` merge every
   connected account's threads, sorted by `last_message_at` as today. Each row
   gets a small colored dot (the account's assigned color) next to the sender
   once more than one account is connected; with exactly one account it's
   omitted, so the common case is a pixel-for-pixel no-op.
3. **Fast account scoping.** Command palette gains `Switch to <email>` and
   `Show all accounts`; `Cmd/Ctrl+0` returns to the unified inbox, while
   `Cmd/Ctrl+1..9` map to accounts in sort order, the same convention browsers
   use for tabs. These are ordinary entries in the
   existing command registry (`commands.ts`, `PLAN.MD` §9), so they inherit
   remapping, the palette, and shortcut help for free.
4. **Settings → Accounts.** A dedicated section beside Appearance/Reading/AI/Privacy
   in the existing Settings modal (`App.tsx:1285`). Lists every account with
   status (Connected / Needs re-auth / Syncing), last sync time, a color
   swatch, reorder, Reconnect, and Disconnect. Disconnect explains explicitly that it
   deletes the local cache for that account while leaving Gmail untouched —
   consistent with the local-first framing in `PLAN.MD` §12.
5. **Composer identity.** A From selector appears only when more than one
   account is connected. Reply/Reply All/Forward lock to the thread's owning
   account (never offer a switch mid-reply); New Message defaults to the
   most-recently-used account. The composer header always states the sending
   address — sending from the wrong identity is the worst failure mode this
   feature can introduce, so it must never be ambiguous.
6. **Badges stay combined.** The inbox icon and OS-level unread badge always
   show the total across accounts. Per-account counts only appear in the
   switcher popover and Settings, so single-account users see no new chrome.

```
┌ sidebar ─┐
│  (D)  ←── click opens:
│          ┌─────────────────────────────┐
│          │ ● All accounts        (12)  │
│          │ ─────────────────────────── │
│          │ 🟦 you@work.com        (7)  │
│          │ 🟩 you@personal.com    (5)  │
│          │ ─────────────────────────── │
│          │ + Add account                │
│          └─────────────────────────────┘
│  Inbox   │
│  ...     │
└──────────┘
```

## Scope and decisions

- Support any number of connected Google accounts; design and test against
  roughly five, since that already exceeds realistic personal use.
- One connected account keeps today's UI unchanged. Every affordance above
  (account dots, From selector, switcher list, `Cmd+N` shortcuts) only appears
  once a second account exists.
- Reuse the app's single OAuth `client_id`/`client_secret`
  (`DISPATCH_GOOGLE_CLIENT_ID`/`SECRET`, `auth.rs:60-75`) for every account —
  Google OAuth apps aren't per-end-user, only the token and consent are.
- Key accounts by their Gmail address, not a synthetic UUID. This matches code
  that already exists: `outbox_messages.account` and `draft.account`
  (`correspondence.rs:42-43, 254-259, 475-476`) already store the sender's
  email as an identity string, anticipating exactly this. `sync_state` and
  `mutations` already have an `account_id TEXT` column (`db.rs:64-86`); today
  it's hardcoded to the literal string `'default'`.
- AI provider keys, appearance, and privacy settings stay global, not
  per-account — there's no real scenario for a different AI key per Gmail
  address.
- Migration: on first launch after upgrade, the single existing connected
  account (keyring entry `google-oauth-default`) is renamed to a real
  `accounts` row keyed by its actual Gmail address (read via
  `gmail.users.getProfile`), and every `'default'` literal in `threads`,
  `mutations`, `sync_state`, drafts, and outbox is rewritten to that address in
  one migration transaction. No re-authentication required.
- Removing an account deletes its local threads/messages/drafts/outbox rows
  and its keychain entry. Gmail server-side state is never touched, matching
  the project's local-first stance.

## Current code and prerequisites

The whole stack currently assumes exactly one account:

- `auth.rs`: `GoogleAuth` is a single struct with a fixed keychain
  `SERVICE`/`TOKEN_KEY` (`auth.rs:17-18`) and two process-wide `OnceLock`
  caches (`TOKEN_CACHE`, `AVAILABLE_CACHE`, `auth.rs:26-35`) — there is
  nowhere to hang a second identity's tokens today.
- `db.rs`: `threads.provider_thread_id` is globally `UNIQUE`
  (`db.rs:28`), which breaks the instant two Gmail accounts can both have a
  thread with the same provider ID. Every account-scoped query hardcodes the
  string `'default'` (`db.rs:86, 321, 344, 380, 391, 401, 422`).
- `sync.rs`: `SyncService` wraps one `Database` and one `GoogleAuth`
  (`sync.rs:14-21`); `polling_loop` (`sync.rs:87`) drives one adaptive-backoff
  loop for that pair.
- `lib.rs`: `AppState` holds `auth: Option<GoogleAuth>` and
  `sync: Option<SyncService>` (`lib.rs:21-27`) and spawns exactly one polling
  task and one correspondence worker at startup (`lib.rs:224-250`).
- `correspondence.rs`: `Correspondence` also holds a single
  `auth: Option<GoogleAuth>`, used by `compose_identity()`/`refresh_identity()`
  to stamp drafts and outbox items with the sender's email, and to pause
  outbox delivery when the reconnected identity doesn't match a queued draft's
  account (`correspondence.rs:475-476, 768, 840, 878-879`) — this pause-on-
  mismatch logic already exists for the single-account case and generalizes
  directly to "route to *this* draft's account" once more than one exists.
- `src/domain.ts` / `src/data/client.ts`: `MailClient` has no account
  parameter anywhere; `AuthStatus` is a single `{ configured, connected }`
  value (`domain.ts:60-63`).

Extend native and demo (`src/data/demoClient.ts`) implementations together, as
`phase-2-compose.md` did for drafts.

## 1. Account storage and multi-credential OAuth

### Deliverables

- An `accounts` table keyed by Gmail address, holding display name, assigned
  color, connection status, and sort order.
- `GoogleAuth` scoped to one account's tokens instead of a global singleton;
  connecting a second account never disturbs the first.
- Add-account flow identifies *which* Google account was just authorized
  (rather than asking the user to type it), and treats re-authorizing an
  already-connected address as reconnect, not a duplicate.

### Implementation

Add a migration:

```sql
CREATE TABLE accounts (
    email TEXT PRIMARY KEY,
    display_name TEXT,
    color TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('connected','needs_reauth')),
    sort_order INTEGER NOT NULL,
    connected_at TEXT NOT NULL,
    last_synced_at TEXT
);
```

Change `GoogleAuth` (`auth.rs`) from a struct with fixed constants to one
constructed per account: `GoogleAuth::new(client_id, client_secret, email)`,
with `TOKEN_KEY` derived as `format!("google-oauth-{email}")`. Replace the two
process-wide `OnceLock` caches with per-instance `Arc<Mutex<Option<Tokens>>>`
fields, since there's now one `GoogleAuth` value per connected account rather
than one for the whole process.

`authorize()` (`auth.rs:95-164`) keeps its PKCE/loopback flow unchanged, but
after exchanging the code, call Gmail's profile endpoint
(`https://www.googleapis.com/gmail/v1/users/me/profile` or the `openid`
ID token's `email` claim, already in scope via `SCOPES`) to learn the
account's address before saving tokens under that address's keychain entry.
Serialize add-account attempts behind a single app-wide mutex so two
simultaneous "Add account" clicks can't race two loopback listeners into a
confusing double browser prompt.

Add Tauri commands `list_accounts`, `add_account`, `remove_account`,
`reconnect_account`, `set_account_color`, `reorder_accounts`, replacing today's
singular `google_auth_status`/`connect_google`/`disconnect_google`
(`lib.rs:111-137`); keep those three as thin wrappers over the first
connected account for the demo client's single-account tests if that's cheaper
than migrating every call site at once.

### Acceptance

Connecting a second account does not require re-authorizing the first.
Reconnecting an already-connected address updates its one row rather than
creating a duplicate. Removing an account deletes exactly its own keychain
entry and leaves other accounts' tokens intact. `GoogleAuth::available()` for
one account is unaffected by another account's connect/disconnect.

## 2. Schema and sync engine scoping

### Deliverables

- `threads`, `messages`, and the FTS index are scoped by account; a thread ID
  collision between two Gmail accounts cannot merge or overwrite unrelated
  mail.
- Each connected account gets its own sync cursor, its own adaptive polling
  loop, and its own failure/backoff state — one account's expired cursor or
  rate limit never blocks another's sync.

### Implementation

Migrate `threads` to `UNIQUE(account_id, provider_thread_id)` instead of a
bare unique `provider_thread_id` (`db.rs:28`), add `account_id TEXT NOT NULL
REFERENCES accounts(email)` to `threads` and `thread_search`, and thread it
through every `Database` method that currently reads/writes the literal
`'default'` (`list_threads`, `search_threads`, `mutate_thread`, `sync_status`,
`cursor`, `finish_sync`, `fail_sync`, `begin_full_sync`,
`upsert_gmail_thread`, `delete_gmail_thread` — `db.rs:122-559`). `sync_state`
and `mutations` already carry an `account_id` column, so those two tables need
no shape change, only real values instead of `'default'`.

Change `SyncService::new(database, auth)` (`sync.rs:21`) to
`SyncService::new(database, auth, account_id)`, and have `lib.rs` hold a
registry (`Arc<Mutex<HashMap<String, ConnectedAccount>>>`, one entry per
connected email, each owning its `GoogleAuth`, `SyncService`, and polling
task handle) instead of the single `Option<SyncService>` today
(`lib.rs:21-27, 224-234`). Adding an account spawns one more `polling_loop`
task; removing one aborts its task and its rows. Keep per-account mutation
delivery serialized as it is today (`deliver_mutations`, `sync.rs:226`), since
Gmail's per-account operation ordering guarantee doesn't extend across
accounts anyway.

### Acceptance

Two accounts each with a thread sharing the same provider thread ID (feasible
since Gmail IDs aren't globally unique) stay fully separate in storage and
search. Killing network access or exhausting quota for one account's poller
leaves the other account's sync and UI responsive. A migrated install's
existing mail, cursor, and pending mutations all survive under the real
address instead of `'default'`.

## 3. Unified inbox and account-aware UI

### Deliverables

- The inbox merges all connected accounts by default, with a small
  per-account color indicator on each row once more than one account exists.
- Instant account scoping via the sidebar switcher, the command palette, and
  `Cmd/Ctrl+1..9`.
- A Settings → Accounts section for add/remove/reconnect/reorder/recolor.

### Implementation

`list_threads`/`search_threads` (`db.rs:122, 176`) take an optional
`accountId` filter; omitted or `"all"` merges every account, matching today's
behavior when only one exists. `Thread` (`domain.ts:1-14`) gains
`accountId: string`; the thread row component reads the connected account's
color from a small client-side account list rather than duplicating color
data per row.

Add an `AccountSwitcher` component behind the sidebar brand button
(`App.tsx:665`), an `Accounts` settings panel alongside `AccountSettings`
(`App.tsx:1425`), and extend `SettingsSection` (`App.tsx:96`) accordingly.
Register `account.switch`, `account.showAll` commands in `commands.ts` with
default keys `Cmd/Ctrl+1`.. `9` bound by sort order, following the same
`Command` shape used everywhere else (`PLAN.MD` §9) so remapping and the
palette work without special-casing accounts.

`AuthStatus` (`domain.ts:60-63`) becomes a list; extend `MailClient`
(`client.ts:13-27`) with `listAccounts()`, `addAccount()`,
`removeAccount(email)`, `reconnectAccount(email)`, `setAccountColor(email,
color)` in both the Tauri and demo clients.

### Acceptance

With one account connected, the inbox, sidebar, and settings are visually
identical to before this feature. With two or more, every thread shows the
correct account's color, `Cmd+2` filters to the second account in place
instantly (no full reload), and `Show all accounts` returns to the merged
view without losing scroll position or selection where reasonably possible.

## 4. Compose, reply, and send-as correctness

### Deliverables

- Reply/Reply All/Forward always send from the thread's owning account.
- New Message defaults to the most-recently-used account, with an explicit
  From selector once more than one account exists.
- The composer always visibly states the sending address; outbox delivery for
  one account pausing (e.g. token expiry) never blocks another account's
  queued sends.

### Implementation

`correspondence.rs` already stamps `draft.account`/`outbox_messages.account`
with an email string and already pauses delivery when the reconnected
identity doesn't match a queued item's account (`correspondence.rs:475-476,
768, 840, 878-879`) — generalize `compose_identity()`/`refresh_identity()`
from "the one connected account" to "the account registry entry for this
draft's `account`". `Correspondence` moves from a single `auth: Option
<GoogleAuth>` field to the same account registry `lib.rs` uses, so a draft
addressed to `you@work.com` resolves and sends through that account's
`GoogleAuth`/provider client regardless of which account is currently
"active" in the UI.

Extend `Composer.tsx` with a From selector, hidden when only one account is
connected (matching `phase-2-compose.md`'s existing single-account
assumption), and populate it from `listAccounts()`. Reply/Reply All/Forward
pass the source thread's `accountId` and never expose the selector for those
modes.

### Acceptance

Replying to a thread in account B always sends from B even when account A is
the active/filtered account in the UI. Disconnecting or expiring account A's
token pauses only A's outbox items (existing per-item pause behavior,
`correspondence.rs:878-879`) while B's queued sends continue normally. The
composer's stated From address always matches what actually sends.

## 5. Keyboard, palette, and shortcut help

### Deliverables

- `Cmd/Ctrl+0` for all accounts, `Cmd/Ctrl+1..9` account switching, and
  `Switch to <email>`/`Show all accounts` palette entries are discoverable the
  same way every other command is.
- Shortcut help (`shortcuts.open`) documents the new bindings once more than
  one account exists.

### Implementation

Register the commands from item 3 through the existing command registry so
they inherit user remapping, collision detection, and the
development-mode warning for actions without a command (`PLAN.MD` §9). Gate
their visibility in the palette and shortcut help on `accounts.length > 1`, so
a single-account install's palette and help screen show nothing new.

### Acceptance

Keyboard-only pass: connect a second account, switch between accounts with
`Cmd+0`/`Cmd+1`/`Cmd+2` and the palette, compose/reply from each, disconnect one, and
confirm the other's shortcuts, unified inbox, and outbox are unaffected
throughout. Update the automated keyboard acceptance suite referenced in
`PLAN.MD` §9 to include this pass.

## Delivery sequence and review gates

1. **Schema and accounts table:** migration, `GoogleAuth` per-account refactor,
   upgrade-path migration of the existing single account to a real address.
   Verify against a copy of a real pre-upgrade database.
2. **Multi-account sync engine:** per-account cursors/polling, thread/message
   scoping, contract tests proving two accounts with colliding provider thread
   IDs stay isolated.
3. **Add/remove/reconnect UI:** Settings → Accounts, sidebar switcher, account
   colors — usable and testable with real Gmail accounts before touching
   compose.
4. **Unified inbox and scoping:** merged list, per-row indicator, palette and
   `Cmd+N` filtering.
5. **Send-as correctness:** composer From selector, reply/forward account
   locking, per-account outbox pause verification.
6. **Keyboard and release acceptance:** full command/shortcut-help pass,
   regression run of the existing single-account Playwright suite (must stay
   green unchanged), then the new two-account keyboard-only pass.

Run Rust unit/integration tests, the demo-client contract tests
(`src/data/providerContract.test.ts`), and Playwright at each gate. As with
Phase 2, browser fixtures can't validate real keychain isolation between two
accounts or actual Gmail behavior when two accounts share a provider thread
ID pattern — include controlled real-account verification with two
disposable test Gmail accounts before declaring this ready.
