# ThreeStrands product plan

**Status:** Current product baseline and forward-looking roadmap
**Reviewed:** 2026-10-02 against the 0.55.1 codebase; provider, drafts, and
roadmap status corrected 2026-10-26 for 0.174.0
**Contact capabilities updated:** 2026-10-08 for 0.83.0

## Product direction

ThreeStrands is a source-available, keyboard-first desktop email client for people
who spend much of their day in their inbox. Its aim is to make important
conversations easy to process while keeping follow-ups, scheduling context, and
control of personal data close to the mail itself.

Project licensing is governed by the [PolyForm Perimeter License 1.0.1](LICENSE).
The source can be inspected, built, used, and modified for permitted purposes;
providing others with a competing product built from it is prohibited.

The product is local-first. The desktop app connects directly to Gmail and to
IMAP/SMTP servers; it does not require a ThreeStrands account or an application-operated mail backend.
Mail is cached in SQLite for fast reading, search, and offline work. The mail
provider remains the source of truth for mail. The app stores credentials in the
operating system's credential store and sends mail through a durable local
outbox. Optional AI sends bounded mail context to a provider the user
configures. Optional cross-device sync replicates selected
workflow data through storage the user chooses.

This document distinguishes implemented behavior from future work. “Available”
means present in the repository, with automated coverage; it is not a claim
that every native integration has completed live-account or release
validation. The older phase plans linked below are useful implementation
history, but their milestone labels and uncompleted-item lists are no longer
the product status.

## Current feature set — website draft

This section is written at the level of a future product website. It describes
features in the current desktop app without promising a release date, a
particular platform package, or always-on service behavior.

### Move through mail quickly

- Navigate conversations, messages, accounts, and views from the keyboard.
  Use a command palette and shortcut help to discover actions.
- Read and triage multiple Gmail and IMAP accounts in one merged view, or focus on one
  account. Archive, restore, trash, mark as spam, mark read or unread, star,
  label, and process selected conversations in batches, with undo for
  reversible actions.
- Search the local mail index; when relevant mail is outside the cache, server
  search can bring older results into the local view. Filter the mailbox by
  unread, starred, important, or unanswered conversations.
- Create account-specific Split Inboxes using a sending domain, Gmail label,
  or address pattern. Keep high-volume mail in its own view while the main
  inbox stays focused.

### Keep the whole conversation moving

- Compose, reply, reply all, and forward with rich text, attachments, and
  reusable snippets. Recipient suggestions come from correspondence history
  and addresses pinned in the composer.
- Drafts save locally and to the account's Drafts on the server, so they
  follow you to other devices and mail apps. A queued send has a ten-second undo window, and the
  durable outbox preserves pending work across restarts. Sent mail is
  reconciled with Gmail when the delivery result is uncertain.
- Turn a conversation into an action, follow-up, or waiting-for task; create
  standalone tasks too. Set due dates and repeating follow-ups. Waiting and
  follow-up tasks linked to a conversation can complete when a new reply
  arrives.
- Browse, add, and edit contacts outside the composer, with multiple addresses,
  notes, profile details, correspondence history, groups, birthdays, and Keep in
  Touch reminders. Import and export CSV/vCard address books, explicitly merge
  saved profiles, and reversibly hide addresses from recipient suggestions on
  this device. See [contact management](docs/address-book.md) for merge rules,
  interchange limits, and sync boundaries. A conversation context panel brings
  people, related tasks, and meetings beside the message.
- Unsubscribe when a message advertises a supported unsubscribe method, with
  confirmation before the request.

### Bring schedule and assistance into context

- Connect Google Calendar separately from mail. See today's schedule or a
  week view, choose visible calendars, preview calendar invitations, and
  check available times against selected calendars and working hours. Put
  selected times into a draft reply. Create events with invitees from the week
  view or a reviewed meeting suggestion, after checking the event dialog.
- Optionally connect an AI provider with your own API key for thread
  summaries, draft assistance, and suggested tasks or meetings. Suggested
  actions stay proposals until you review them; the app does not create a
  task or send a message from AI output on its own. Thread Chat can answer
  questions with selected readable attachments and explicitly requested mailbox
  search. Contact enrichment suggests profile details from correspondence for
  the user to apply. Proactive briefs require a separate opt-in; enabled briefs
  send qualifying conversations after the reader stays on them. Settings show
  requests, tokens, and estimated costs, with an optional fast model for
  summaries, reply drafts, and contact enrichment.

### Stay in control of local data

- Read cached mail and work on drafts while offline. Mail changes and sends
  queue locally and resume when the app can reach Gmail.
- Adjust local mail retention and remote-image loading. Sender HTML is
  sanitized and contained in a sandboxed reader; remote images stay behind
  the native proxy and the chosen image-loading setting.
- Transfer settings, account display metadata, saved contacts, snippets, and
  Split Inboxes with a password-encrypted export. Mail, drafts, queued sends,
  OAuth credentials, and AI keys are excluded; accounts need authorization on
  the new device.
- Inspect sync diagnostics and use opt-in crash reporting. Local database
  backups and startup recovery protect access when the cache is damaged.

**Encrypted sync:** End-to-end encrypted cross-device sync can replicate tasks,
goals, saved contacts, snippets, Split Inboxes, selected preferences, and account
display metadata through a shared folder, S3-compatible storage, or an IPFS connector. Devices
can join with a code, recovery phrase, or approval from another device.
Cached mail, attachments, drafts, queued sends, and provider credentials stay
on each device. See [cross-device sync](docs/cross-device-sync.md) for the
precise data boundary and connector behavior.

## Product roadmap — website draft

These are directions for unfinished product work, not a dated commitment or
an assertion that a feature is already in beta. Keep this section separate
from the current feature set when publishing a website.

| Direction | What users could expect |
| --- | --- |
| Snooze | Available in 0.190.0: put conversations aside, reschedule them in Snoozed, and return them automatically while a connected installation is running. |
| Gmail sending aliases | Available in 0.191.0: use verified aliases, Gmail defaults, addressed replies, per-address signatures, and server drafts. |
| Connected address books | Import existing Google Contacts and keep them connected to local profiles, notes, and correspondence history. |
| Smarter inbox rules | Explore inspectable, user-controlled classification beyond today's domain, label, and address rules. |
| More mail providers | Harden IMAP/SMTP (design and progress in `docs/imap-design.md`; read, triage, server search, and sending work today), then Microsoft 365. |
| Encrypted sync reliability | Keep hardening optional encrypted sync across devices and app versions, especially upgrades and interrupted joins. |
| Shared and mobile workflows | Explore a mobile companion and shared conversations or drafts where demand justifies the additional architecture. |

The roadmap should be revised as research and reliability work change
priorities. Contact browsing and editing, meeting proposals, available-time
replies, and reviewed calendar-event creation exist today. Task follow-ups exist
today. Scheduled sending with one fixed sending device and synchronized
summary visibility is implemented, and drafts follow the account across devices; conversation
snoozing is implemented in 0.190.0.

## Competitive feature-gap backlog

Assessed 2026-10-31 against the 0.189.1 repository and current competitor
documentation. This is an internal planning backlog, separate from the website
roadmap above. It records capability gaps, not measured reliability or usability
deficits. Priorities are proposals for the current individual, Gmail-first Mac
audience; they are not delivery commitments. Update each row as work is scoped
or shipped, and link its implementation issue or design here when one exists.

The existing roadmap already names snooze, Gmail aliases, connected address
books, smarter rules, Microsoft 365, and shared/mobile exploration. This backlog
makes their scope actionable and also captures broader calendar support,
execution while devices are closed, and proactive assistance.

| Gap | Current behavior and missing capability | Proposed next step | Sequence / status |
| --- | --- | --- | --- |
| Conversation snooze | Gmail and IMAP conversations can be snoozed individually or in batches, with rescheduling, undo, new-reply return, and optional encrypted schedule sync. Returns need a running, connected installation. | Complete developer live-account validation alongside the existing release checks. | Implemented in 0.190.0; see [snooze design](docs/snooze-design.md). |
| Gmail sending aliases | Verified aliases, Gmail defaults, addressed replies, per-address signatures, and alias-preserving server drafts are implemented. | Complete developer live-account checks with an alias, including queued and scheduled delivery. | Implemented in 0.191.0; see [Gmail alias design](docs/gmail-aliases-design.md). |
| General rules and richer saved views | Account-specific Split Inboxes match a domain, label, or address pattern. Sender muting archives and marks mail read. Compound rules, reusable search views, and views across accounts remain gaps. | Define a first rules contract with explicit conditions and actions, previews, and account scope; support saving a search as a view. Decide whether rules run locally or at the provider. | Near-term 3; planned, unscoped. |
| Connected address books | Local contact profiles and CSV/vCard import/export exist; ongoing Google Contacts and CardDAV sync do not. | Start with Google Contacts; define ownership, matching, conflict, deletion, and local-notes preservation before implementing sync. Track CardDAV as a later extension. | Near-term 4; planned, unscoped. |
| Mobile access and workflow continuity | Desktop sync carries selected workflow data and provider drafts follow the account. No mobile companion or web client exposes the full workflow on a phone. Windows and Linux builds remain experimental. | Scope a mobile companion around reading, triage, tasks, and follow-ups; decide supported platforms, background notifications, and encrypted workflow-data access before committing to a build. | Expansion; discovery needed. |
| Microsoft 365 / Exchange | Gmail sign-in and password/app-password IMAP exist; dedicated Microsoft authentication, mail, and calendar integration do not. IMAP is not a substitute for modern authentication or delegation. | Define Microsoft authentication and provider contracts first, then mail and calendar support. Scope delegated/shared mailbox support explicitly rather than assuming basic account support includes it. | Expansion; planned, unscoped. |
| Broader calendar support | Google Calendar supports event editing, invitations, availability, and task time blocking. Microsoft calendars and CalDAV/iCloud connections remain missing. | Define a calendar-provider contract and invitation/availability semantics; prioritize Microsoft with the Microsoft 365 work, then evaluate CalDAV/iCloud. | Expansion; discovery needed. |
| Scheduled sending while devices are closed | Send Later uses one fixed sending computer, which must be awake, online, and running the app; missed times require review. | Evaluate provider-native scheduling or an explicitly authorized execution service. Record the architecture and privacy decision before promising delivery while every device is closed. Preserve uncertain-send reconciliation and duplicate prevention. | Architecture decision required. |
| Proactive assistance and setup friction | Optional summaries, reply drafts, draft review, mailbox-search chat, Today reminders, and reply-completing follow-up tasks already exist. AI requires a user-supplied key; broader automatic classification and prepared follow-up drafts remain opportunities. | Research opt-in follow-up detection, reviewable prepared drafts, and classification; assess setup friction separately. Preserve explicit data-sharing controls and user review before sending. | Discovery; below core workflow gaps. |
| Shared mail workflows | Personal tasks and cross-device sync exist; team comments, collaborative drafts, assignments, shared conversations, and delegated inbox workflows do not. | Validate team demand before designing permissions, shared state, revocation, and collaboration infrastructure. Coordinate delegated mailbox requirements with Microsoft support. | Deferred pending audience decision. |

### Suggested focus for the next few weeks

Conversation snooze is implemented in 0.190.0 and Gmail sending aliases in
0.191.0. Continue with richer rules/views and connected Google Contacts. This is a
sequencing recommendation, not a promise to finish the remaining work within that period.
Continue the existing reliability and live-account release checks alongside
feature work. If broader adoption becomes the immediate goal, revisit the
sequence: mobile continuity and Microsoft 365 are the larger adoption blockers.

### Comparison references

The comparison uses [Gmail snooze](https://support.google.com/mail/answer/7622010?hl=en),
[Gmail filters](https://support.google.com/mail/answer/6579?hl=en),
[Gmail sending identities](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.settings.sendAs),
[Apple Mail rules](https://support.apple.com/en-ae/guide/mail/mlhlp1190/mac),
[Outlook modern authentication](https://learn.microsoft.com/en-us/exchange/clients-and-mobile-in-exchange-online/outlook-for-ios-and-android/setup-with-modern-authentication),
[Outlook shared mailboxes](https://support.microsoft.com/en-us/outlook/sharing/about-shared-mailboxes-shared-folders-and-shared-calendars-in-outlook),
[Spark features](https://sparkmailapp.com/features),
[Superhuman features](https://superhuman.com/plans/mail), and
[Shortwave's assistant and platform support](https://www.shortwave.com/docs/guides/ai-assistant/).
Competitor capabilities can change; recheck relevant references when scoping work.
Our current behavior is documented in the help guides for
[compatibility](site/docs/compatibility.md), [triage](site/docs/triage.md),
[composition](site/docs/compose.md), [calendar](site/docs/calendar.md),
[search](site/docs/search.md), and [AI](site/docs/ai.md).

## Current product model and decisions

### Audience and scope

The primary user is an individual who wants a fast daily mail workflow across
one or more mail accounts. Tasks, calendar context, and optional AI support
that workflow. ThreeStrands is not yet a general-purpose mail client: Gmail is
the mature adapter, and an IMAP/SMTP adapter (aimed first at Proton Mail
Bridge) reads, triages, searches, and sends but is still being hardened.

The app does not operate a backend for mailbox access or cross-device sync.
Polling, task reconciliation, queued sends, and replication run while the
desktop app is active and catch up when it starts or resumes. A future feature
that needs execution while all devices are closed requires an explicit new
architecture decision; a scheduled-send UI alone would not provide it.

### Implementation shape

```mermaid
flowchart LR
    UI[React and TypeScript UI] --> IPC[Typed Tauri commands]
    IPC --> Core[Rust mail, task, calendar, and sync services]
    Core --> DB[(Local SQLite and FTS5)]
    Core --> Vault[OS credential store]
    Core --> Gmail[Gmail API]
    Core --> IMAP[IMAP and SMTP servers]
    Core --> Calendar[Google Calendar API]
    Core --> AI[User-selected AI provider]
    Core --> Transport[Optional encrypted sync connectors]
```

- Tauri 2 hosts the React/Vite interface. The Rust side owns provider calls,
  OAuth, credentials, SQLite, MIME and attachment handling, image fetching,
  and correspondence delivery. The browser development build uses fixtures;
  it does not deliver real mail.
- Gmail sync uses initial import, incremental History cursors, adaptive
  polling, startup/resume catch-up, and recovery when a cursor expires.
  Accounts have separate identities and sync state. Local mail actions use a
  durable mutation queue and reconcile with Gmail.
- Sending uses a separate durable outbox with immutable MIME snapshots,
  cancellation during the undo window, and explicit handling for uncertain
  delivery. Outbox records are local to an installation. Drafts are pushed to and
  pulled from the account's Drafts (Gmail Drafts or the mapped IMAP mailbox),
  never through optional cross-device sync.
- The SQLite cache provides local full-text search, workflow records, and
  periodic backups. Settings include a local mail-retention policy. Provider
  data and application-owned workflow data have deliberately different sync
  boundaries.
- Split Inbox rules currently match a domain, label, or address pattern and
  are scoped to one account. Snippets are shared across accounts locally.
  Tasks can be standalone or linked to a thread; a new inbound reply can
  complete a linked waiting or follow-up task. AI action extraction offers
  reviewable proposals rather than directly mutating tasks or calendar.

### Security and privacy boundaries

- OAuth uses a desktop authorization flow with PKCE. Tokens and optional AI
  keys remain outside the webview and mail database, in the OS credential
  store. The app talks directly to Google and to an AI provider only when
  that integration is configured and used.
- Sender-authored HTML is untrusted. The reader sanitizes unsafe capabilities,
  preserves safe layout, and renders inside a sandboxed, CSP-scoped iframe.
  Remote images use the native proxy and can be blocked. Attachment operations
  stay behind native commands.
- The encrypted settings export contains a versioned, validated allowlist and
  deliberately excludes message content and secrets. Its format is persistent
  across app versions; changes require compatibility handling. See
  [settings transfer](docs/settings-transfer.md).
- Optional replicated sync is off by default and uses a user-chosen connector,
  not a ThreeStrands account. It carries only the application-owned data listed
  in [the sync guide](docs/cross-device-sync.md). Transport providers see
  ciphertext, although file sizes and timing remain visible to them.
- Local mail storage is not described as end-to-end encrypted. Gmail and
  recipients process ordinary email, and someone with access to an unlocked
  local profile may be able to read the cache.

## Work remaining before broader release

1. **Validate the native workflow end to end.** The repository has substantial
   automated unit, integration, and Playwright coverage. The
   [features document](docs/features.md) explicitly leaves live Gmail OAuth, synchronization, sending,
   attachment, and recovery checks to controlled real-account validation.
   Keep those checks as release gates until they are recorded as complete.
2. **Harden the current scope.** Continue testing large mailboxes, provider
   rate limits, offline/reconnect behavior, uncertain send outcomes, database
   recovery, and cross-device sync compatibility. Report progress in user
   terms rather than treating a feature's presence in code as proof of
   production reliability.
3. **Build the near-term roadmap deliberately.** Verify scheduled-send
   behavior around native sleep and shutdown; define snooze behavior before
   adding it.
   Extend the existing standalone address book with connected provider address
   books after defining ownership and conflict behavior.
   Mature replicated sync before presenting it as a standard feature.
4. **Expand integrations from tested contracts.** The Rust provider traits
   isolate Gmail-specific access. Add another provider only with its own
   threading, folder, search, mutation, and send semantics plus contract and
   fault-recovery tests. Keep AI optional and user-reviewed.

## Operating principles

1. Do not lose or duplicate mail or send it unexpectedly. Reconcile uncertain
   outcomes rather than retrying a send blindly.
2. Make common actions fast and keyboard-accessible. Keep typing scopes,
   dialogs, undo, and focus behavior predictable.
3. Treat email, attachment names, provider responses, and AI output as
   untrusted. Preserve safe sender layout without weakening the reader's
   isolation boundary.
4. Keep local, Gmail, AI-provider, and connector data boundaries explicit in
   both the product and its marketing claims.
5. Make offline, reconnect, failed mutation, and recovery states visible and
   repairable. Do not promise always-on execution from a desktop-only app.

## Source documents

- [Scheduled send plan](docs/scheduled-send-design.md) — fixed sending-device ownership, local scheduling, and encrypted summary visibility across devices; implemented in 0.98.1, with
  Command/Ctrl+Shift+L added in 0.98.2.
- [Design principles](DESIGN.md) — current keyboard, workspace, form, and AI
  review decisions.
- [Cross-device sync](docs/cross-device-sync.md) and [replicated sync design](docs/replicated-sync-design.md) — sync scope and trust model.
- [Settings transfer](docs/settings-transfer.md) and [email rendering policy](docs/email-rendering-policy.md) — persistent-format and reader boundaries.
- [Features and known limitations](docs/features.md) and [contact management](docs/address-book.md) — current capabilities, shortcuts, and outstanding live-account checks.
- [IMAP/SMTP provider design](docs/imap-design.md) — provider design and phase progress.
- [Today view design](docs/today-design.md) — what Today works out without AI, and the phase 2 AI layer (meaning sort, Top of mind strip).
- [Archived plans](docs/archive/) — completed plans (multi-account, database recovery, modularization, and layer boundary, Phase 2 compose, IMAP spike, Files workspace) kept for design history.
