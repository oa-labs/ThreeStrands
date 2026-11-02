# Delegated agents and invitation research

**Status:** Phases 0–2 implemented: offline contracts, fixture runner, and
interactive Agents preview. Phases 3–5 remain proposed.
The preview runs a local demonstration only; real research tools remain unavailable.
Live-service release gates remain explicit.
**Planned:** 2026-11-02 against 0.194.4.

**Phase 0 validated:** 2026-11-02 against 0.194.5. See the
[integration decision record](agent-integrations/decision-record.md) and
[frozen contract](agent-integrations/contract-v1.json). Exa is the initial optional
paid-search integration, with a separate user-supplied key; routing is unavailable
in v1. Model compatibility, storage eligibility, and native security enforcement
are documented separately from passing offline fixtures.

## Outcome

Delegate work from a conversation, continue processing mail, and return to a
full-size task workspace where an agent can ask questions, report progress, and
deliver a sourced result. The user can question the findings, supply context,
redirect the work, and request revisions without starting over.

The first workflow researches invitations to events and podcasts to help the
user decide whether participating is worth the time and effort. Build shared
execution, interaction, and artifact contracts so later workflows can produce
other kinds of results without inheriting invitation-specific fields.

## Product decisions

- Deliver results in a full-size delegated-task detail view, with the report
  beside its conversation. Do not generate virtual email or insert reports into
  provider mail threads. The originating conversation gets a status and link.
- Keep agent execution separate from personal tasks. Completing research must
  not complete “Decide whether to attend.” Linking an existing task or explicitly
  creating a decision task makes the work reachable from the Kanban board.
- Start jobs only at the user's request. Existing automatic briefs do not start
  research, spend a research budget, or authorize web access.
- Execute locally while an installation is running and awake. Navigation and
  closing the detail view do not stop a job. Quit, sleep, and network loss can
  interrupt it; durable checkpoints support recovery. There is no promise of
  execution while every device is off.
- Initial tools are read-only. No sending mail, modifying contacts, registering
  for events, calendar writes, purchases, arbitrary code, or browser automation.
- Keep jobs, chat, sources, reports, and research profiles device-local initially.
  Encrypted report sync and hosted execution are separate future designs.

These are the proposed first-release defaults. Provider selection and operational
limits must pass the validation gate below before implementation is committed to
a particular external service.

## User experience

### Start and discover work

Offer **Research this invitation** from the conversation actions and command
palette, independent of whether AI detected an invitation. It must work for
ordinary prose invitations as well as calendar messages. Open a small request
form with editable objective, invitation type, selected email context, optional
business profile, and optional task linkage. Show the configured model, external
services, and execution limits before starting. Missing credentials or unsupported
capabilities produce actionable setup guidance without creating a running job.

Default questions should ask whether participation is worthwhile and what needs
verification. The user can narrow the objective, for example, “Prioritize
customer introductions; brand exposure matters less.” Do not infer a business
identity or home address from unrelated email.

Provide a lightweight **Agents** workspace listing active, needs-input, and
finished delegated tasks. This is needed in the first release so unlinked work
remains discoverable after the source email is archived or removed. Reuse the
existing workspace navigation patterns rather than building a separate dashboard
system. Task cards and the conversation context panel link to the same detail.

### Work in the detail view

Show the objective, source conversation link, execution status, progress,
estimated usage, and available controls above two areas: conversation and
result. On a narrow window, use accessible tabs that retain each area's state.
Preserve drafts and scroll position when switching away and back.

Progress describes observable work, such as “Checking published speakers,”
without exposing hidden model reasoning. Completion and needs-input notices link
directly to the item and never steal focus. Persist unread activity separately
from mail unread state. System notifications are optional; in-app notices and
the Agents list must work without notification permission.

Support cancel, retry, archive, and delete. Cancellation stops new dispatches and
preserves already committed findings. Archive hides an item from the ordinary
list without deleting it. Delete cancels active work and removes its local data;
late network results cannot recreate it.

### Clarify and redirect

The agent can ask a structured question with a reason, optional choices, and a
free-text answer. Ask only when the missing answer materially changes the work.
Use visible assumptions for minor gaps. Group related questions and avoid asking
again for information already supplied.

Questions identify which research branches they block. Continue independent
branches; enter **Needs input** only when no useful permitted work remains.
Waiting consumes no active execution time and causes no polling model calls.
The user may answer, skip an optional question, or cancel. Required unanswered
questions remain pending; elapsed time never implies an answer or permission.

The user can add context or change direction while the agent works. Persist the
turn immediately, interrupt at a safe checkpoint, and apply it to the next step.
If an in-flight result arrives for an older objective revision, retain it only
as historical evidence; do not publish it as the current recommendation. Display
whether the instruction is pending or incorporated.

Business context can include company, products, ideal customers, target markets,
marketing goals, and time/travel preferences. A per-job answer is not a permanent
preference. Offer an explicit save action for reusable research profiles. Home or
travel-origin location is optional and separate, with a city-level alternative.

### Interact after completion

Follow-up questions can explain or challenge the existing report. Answer from
saved evidence when possible and identify its research date. Requesting fresh
web research starts another bounded run under the task's allowed tools and
remaining budget; increasing permissions or budget requires an explicit user
choice. Depleted budgets never silently reset on retry or follow-up.

Ordinary chat does not rewrite the report. “Revise the report for this new
constraint” creates an immutable new artifact version and atomically updates the
current-version pointer. Keep prior versions readable and show their date and
scope. A failed revision leaves the last completed report intact.

Clarification is distinct from authorization. An answer about travel budget
cannot authorize spending, sending, or booking. Future action tools will require
their own reviewed action and permission contract.

## Invitation research contract

Each report begins with a recommendation, key reasons, important uncertainties,
and questions to ask before committing. It then presents evidence and practical
tradeoffs. Avoid an opaque numeric “worth it” score.

| Topic | Event invitation | Podcast invitation |
| --- | --- | --- |
| Identity and credibility | Organizer, official event identity, date and venue consistency, corroborating history | Show and host identity, official contact consistency, recent publishing activity |
| Relevant people | Published speakers, sponsors, attendee evidence, and their relevance to the user's objectives | Previous guests, subjects, and audience relevance |
| Reach | Published attendance claims and independent corroboration where available | Published audience figures, their provenance, and clearly labeled public proxies |
| Commitment | Ticket cost if published, preparation, event duration, travel | Preparation, recording duration, format, distribution and promotion commitments |
| Business value | Evidence of customer fit, realistic introduction opportunities, suggested approach | Audience fit, potential marketing/sales benefits, useful topics and calls to action |

Separate confirmed facts, organizer claims, estimates, user assumptions, and
agent judgments. Speaker or sponsor lists do not establish attendance. Public
engagement does not establish listener counts. Say “not verified” when evidence
does not support a number or legitimacy judgment; do not label an invitation
fraudulent merely because little public information exists.

Travel estimates require an explicitly chosen origin, a resolved venue, travel
mode, and a routing tool. Show the basis and lookup date, including whether the
estimate models event-time traffic. Search snippets or straight-line distance
must not be described as driving time. With no routing configuration or origin,
complete the rest of the report and explain what is missing.

Bind the job to selected source message IDs and a bounded context snapshot with
a revision/hash, not only the thread's newest timestamp. New email marks the
report as potentially stale and offers an explicit refresh. It does not silently
expand the job or trigger new external calls.

## Existing code and proposed boundaries

Verified integration points in the current tree:

- `src-tauri/src/ai/provider.rs` handles model requests, keychain access, and
  protocol fallbacks. `ProviderReply` currently carries text and a stop reason;
  Anthropic's forced output tool is a schema mechanism, not an executed tool.
- `src-tauri/src/ai/context.rs`, `brief.rs`, and `chat.rs` bound mail context and
  parse existing AI outputs. Preserve their existing product contracts.
- `src-tauri/src/commands/ai.rs` assembles native email context and persists
  results. `ai/usage.rs` and `db/ai.rs` provide usage/activity integration.
- `src-tauri/src/schema.rs` owns migrations. `src-tauri/src/db.rs` and `db/`
  keep SQL behind named Database methods; new commands must respect that boundary.
- `src-tauri/src/lib.rs` holds AppState, registers commands, and starts native
  background work.
- `src/domain.ts`, `src/data/client.ts`, and `src/data/demoClient.ts` define the
  frontend contracts and demo behavior. `src/useWorkspaces.ts`, `App.tsx`,
  `commands.ts`, and workspace preference types own navigation and shortcuts.
- `src/ContextPanel.tsx`, `TaskSidebar.tsx`, and `TaskDetailDialog.tsx` provide
  conversation/task entry points. `aiSettings.ts`, `AiProviderSettings.tsx`, and
  `AiActivityList.tsx` own AI configuration and activity presentation.

Proposed additions are native `agents/` modules for scheduling, policy, workflow,
and tools; `db/agents.rs`; `commands/agents.rs`; and frontend `AgentsWorkspace`,
`AgentTaskDetail`, `AgentConversation`, `ResearchReport`, and a focused data hook.
Names are provisional. Keep orchestration out of React and SQL out of commands.
Do not overload the short brief cache, task notes, or mail tables with reports.

## Persistence and execution

### Durable entities

| Entity | Required responsibilities |
| --- | --- |
| Delegated task | Workflow kind/version, account scope, optional source/thread and task links, objective revisions, context snapshot, tool grants, configuration snapshot, archive/unread state |
| Run | Task and objective revision, trigger, state, checkpoint, lease/generation, retries, reserved/observed usage, errors, timestamps |
| Conversation turn | Ordered user/assistant turn, included context revision, incorporation state, optional structured clarification and answer linkage |
| Step/tool call | Stable execution ID, validated arguments, dispatch state, result reference, timing, usage, retry/replay disposition |
| Source | Validated URL and provenance, title, retrieved time, bounded extracted evidence, content hash, fetch outcome |
| Artifact version | Task/run/revision, type and schema version, structured content, validated citation IDs, creation time; immutable after publication |
| Research profile | Explicitly saved business/travel context and sensitivity settings, independently deletable |

Use additive migrations, foreign keys, indexes for runnable jobs and task
lookups, and transactional state transitions. Keep agent-to-personal-task links
local so existing synced task payloads do not acquire unsupported fields. A task
synced to another device initially has no agent result there; do not claim
cross-device availability. Unlinking or deleting a personal task leaves the
delegated task accessible from Agents.

### Run lifecycle

Use `queued`, `running`, `waiting_for_input`, `paused`, `completed`, `failed`, and
`cancelled` states. Paused carries a reason such as offline, missing credentials,
disabled feature, budget exhausted, or interrupted execution. Failure describes
an unrecoverable or retry-exhausted attempt. A finished task may have later runs;
its previous report remains readable while those runs execute.

Allow one active run per delegated task. Acquire work transactionally with a
lease and generation token, then release database locks before network I/O.
Persist tool intent before dispatch and result/checkpoint before advancing. On
restart, recover expired leases and unfinished steps without replaying committed
steps. Timers and events are hints; SQLite is the source of truth.

Network calls cannot generally be exactly-once. A crash after dispatch but
before result persistence can cause a repeated read or model charge. Record
ambiguous attempts, preserve conservative budget reservations, bound replays, and
explain that limitation in recovery behavior. No write-capable tools are allowed
in this release. Cancellation/deletion and objective revisions invalidate worker
generations; stale workers cannot publish artifacts or issue further calls.

Use an injected clock, cancellation token, and configurable worker concurrency.
Run blocking parsing/database work off async execution threads where appropriate.
Pause on revoked credentials or disabled agent settings. An intentional pause
requires an explicit resume; transient network pauses may recover automatically
within the existing grants and budget. Clarify these reasons in the UI.

### Limits and usage

Centralize agent policy limits in a native module and return them to the UI.
Proposed starting defaults: two concurrent jobs globally, one dispatch per job,
20 model turns, 20 searches, 30 fetched pages, a 15-minute active-time allowance,
and at most two retries per transiently failing step. Validate these against
fixtures before freezing them. Waiting, offline time, and queue time do not
consume active time; retries and all dispatches consume call/usage allowances.

Also bound input/output tokens, query length, response bytes after decompression,
redirects, extracted characters, conversation context, and persisted evidence.
Test below, exact, and above each limit. Reserve allowed usage before dispatch
so concurrent work cannot overspend a shared task allowance. Follow-up runs draw
from the same cumulative task budget unless the user explicitly extends it.

Aggregate model tokens and tool charges by job/run into existing AI activity.
Do not report missing prices as zero or promise an exact monetary ceiling when
provider pricing/usage is unavailable. Enforce call/token limits regardless;
only offer a conservative dollar limit when a validated pricing basis exists.

## Model and tool interfaces

Add an agent-specific typed protocol: assistant message plus validated actions
such as `search_web`, `read_page`, `ask_user`, and `publish_artifact`. Application
code owns dispatch and authorization. Publishing and asking are internal workflow
operations, not external privileges. Model output never directly mutates tasks
or executes arbitrary function names.

Choose the initial supported model/provider pair through fixture-backed adapter
tests. A structured action envelope may be used with the existing transport, or
an adapter may expose native tool calls. In either case, retain call IDs, typed
arguments, tool responses, and stop reasons. Do not mistake schema-output tools
for executable requests or silently fall back to an unvalidated action string.
Leave existing brief/chat provider behavior unchanged. Unsupported configurations
must fail preflight clearly rather than create jobs that cannot run.

Search, page reading, and routing use independent service interfaces, separate
keychain credentials where required, timeouts, cancellation, and deterministic
fake implementations. Select an initial search service during phase 0 after
verifying current API terms, source metadata, quotas, pricing, and retention.
Keep external service selection out of the stored report schema. Routing is an
optional capability; the UI must disclose when travel research is unavailable.

Tools accept only the current job's account/context scope. Optional local mailbox
search is a separate per-job grant, off by default, and initially searches only
the source account. Attachments, other accounts, calendars, and contact graphs
are not implicitly included. A capability increase requires an explicit choice.

## Security, privacy, and data lifecycle

- Treat mail, web pages, source titles, task text, and earlier assistant output
  as untrusted data. Follow authenticated user instructions; enforce tool names,
  scope, arguments, and limits in native code. Prompt wording alone is not an
  authorization boundary.
- Search uses minimal public entity facts. Never send whole email bodies,
  unrelated addresses, private customer lists, or precise home location as search
  queries. Preserve URLs' identity while removing known tracking parameters only
  where safe; do not automatically open every link in an invitation. Show that
  reading an external URL contacts its host. Do not fetch unsubscribe/action
  endpoints or signed/token-bearing private links as ordinary research pages.
- Routing receives only an explicitly permitted origin and destination. Disclose
  that precise locations go to the routing service. Keep credentials out of job
  snapshots, reports, errors, diagnostics, and exported settings.
- Page reading is a separate native text-fetch boundary. Permit public HTTP(S)
  destinations only, reject credentials in URLs and local/private/link-local/
  metadata targets, validate DNS and connected addresses, and revalidate every
  redirect. Bound compressed responses, parser work, content types, and redirects.
  Do not attach browser cookies, execute scripts, load subresources, or reuse mail
  remote-image consent as authorization for research requests.
- Render reports/chat through a constrained structured or sanitized Markdown
  renderer. Disable raw HTML, embedded frames, remote images, and active content;
  validate link schemes. Only renderer-created source links can open externally.
  Do not alter the existing sandboxed sender-HTML rendering boundary.
- Store claim-to-source references with bounded supporting excerpts. Validate
  source IDs and excerpt membership before publishing. This verifies provenance,
  not truth; contradictory or weak evidence remains visible. Model-invented URLs
  cannot become citations without retrieval through the validated tool path.
- Use explicit profile deletion and per-task deletion. Account removal cancels
  and deletes account-scoped agent data. Mail cache eviction/deletion detaches
  source links and removes retained email body snapshots; preserve only the
  explicitly saved report/chat with a visible source-unavailable state. Explain
  that reports and user chat may still contain mail-derived facts until deleted.
- Propose a seven-day retention for raw page extraction/tool payloads; retain
  bounded cited excerpts, source metadata, reports, and conversation until the
  user deletes the task. Cleanup must preserve report provenance and run without
  blocking startup. Never include content in normal diagnostics.
- Exclude jobs, reports, sources, profiles, and precise locations from settings
  transfer and encrypted sync initially. Any portable agent preferences must
  follow the existing transfer allowlist and compatibility defaults; add a frozen
  preceding-schema fixture before changing that schema. AI/search/routing keys
  always remain excluded. Document actual local-backup behavior separately;
  deletion must not promise erasure of existing backups.

## Implementation sequence and exit criteria

### 0. Validate integrations and freeze the first contract

**Complete for offline design and conformance.** The
[decision record](agent-integrations/decision-record.md) selects the model and
Exa adapters, specifies optional paid-search setup, resolves retry/key/disclosure
policy, and freezes connection requirements. Run
`node --test scripts/agent-contracts.test.mjs` for the synthetic request/response
and failure fixtures. This is not live-service certification: account-specific
persistence rights and real native connector checks remain release gates before
external tools are enabled. Routing deliberately returns `not_configured`.

Select and document the initial model adapter, search service, optional routing
service, pricing basis, supported content types, and concrete policy bounds.
Verify current primary documentation at implementation time; this plan does not
assert current vendor support. Produce request/response fixtures for successful,
unsupported, malformed, rate-limited, and interrupted calls. Resolve key storage,
data disclosure, retry semantics, and testable DNS/redirect handling before wiring
real tools. Exit: an adapter decision record and passing contract fixtures.

### 1. Persist and run delegated work

**Implemented for 0.195.0.** See [the phase-1 record](agent-integrations/phase-1-runner.md).
The [verification record](agent-integrations/phase-1-runner.md#recorded-verification-on-2026-11-02)
records intermittent macOS URLSession failures reproduced in a pre-change binary,
followed by a passing full native pre-merge run.
The local runner is disabled by default and accepts only a clearly labeled fake
workflow. Native persistence, command/client contracts, clarification, recovery,
and cancellation are covered automatically. Real-service dispatch is still
unavailable; phase 2 supplies the interactive workspace.

Add the schema/entities, Database methods, native commands, policy, scheduler,
startup/shutdown hooks, and fake workflow/tools. Support enqueue, cancel, pause,
resume, clarification, and restart recovery. Add typed frontend/demo contracts.
Exit: a fake job survives restart, waits for an answer without model polling,
resumes once, and cannot publish after cancellation or deletion.

### 2. Build the shared interactive workspace

**Implemented for 0.196.0.** See the [phase-2 record](agent-integrations/phase-2-workspace.md).
Agents is a full-size workspace with email and personal-task entry points,
clarification, discussion, explicit revisions, saved report versions, and unread
activity. Execution remains an explicitly enabled local demonstration. Reports
are inert plain text; real providers, citations, and web research belong to phase 3.

Implement the Agents list, task detail, conversation, immutable artifact viewer,
unread activity, navigation/shortcut entry points, and optional task linkage.
Use fake jobs and deterministic reports first. Include source-unavailable,
unsupported-provider, needs-input, failure, empty-result, and revision states.
Exit: the complete delegate/leave/return/answer/refine flow works under automated
coverage without conflating personal task status or email unread state.

### 3. Add real read-only research

Implement the validated model loop, search and text-fetch tools, source store,
usage reservations, and explicit disclosure/settings. Add optional account-scoped
mail search and routing only behind their own grants/configuration. Integrate
existing AI activity without logging content. Exit: recorded responses produce
valid cited findings; malicious inputs, exhausted budgets, and partial outages
cannot bypass native policy or fabricate a completed successful report.

### 4. Deliver invitation evaluation

Implement event/podcast workflow prompts and structured report schemas, profile
selection, source snapshots, recommendations, uncertainty labels, and stale-input
refresh. Cover ordinary invitations without calendar attachments. Support
clarifying questions, mid-run redirection, grounded follow-up, explicit fresh
research, and report revisions. Exit: both invitation types produce useful
reports with sources and honest missing-data states, including travel being
unavailable and audience size being unverified.

### 5. Harden and release

Complete migration, crash/recovery, retention/deletion, accessibility, security,
and performance coverage. Document provider setup, local-only execution and
storage, data sent to services, profile handling, and costs. Run required checks
below. Developer performs hands-on app validation; agents do not start a dev
server or use a browser to judge appearance. Ship as a minor version bump from
the then-current version, synchronized across package.json, Tauri configuration,
Cargo.toml, and the threestrands Cargo.lock entry. Add a user-facing Unreleased
changelog entry and update PLAN.MD's available capabilities only when shipped.

## Verification plan

- Database/scheduler tests with injected clocks and fake tools: migration from
  the preceding schema, one active run, concurrency, lease expiry, crash around
  dispatch/commit/publication, bounded replay, queue fairness, cancellation,
  disable/revocation, delete races, restart, and atomic artifact publication.
- Interaction tests: grouped clarification, optional skip, required waiting,
  independent work while blocked, duplicate/stale answers, pending redirection,
  old-generation rejection, follow-up budget accounting, failed revision keeping
  the previous report, and no automatic personal-task completion.
- Tool security tests: malicious mail/pages/tool output, unknown actions,
  malformed arguments, cross-account requests, secret-bearing query attempts,
  private/IPv6/DNS-rebinding/redirect targets, credentials in URLs, oversized or
  compressed responses, invalid citations, active Markdown/HTML, and unsafe links.
- Report fixtures: structurally different event and podcast invitations,
  conflicting organizer claims, sparse public evidence, recent versus outdated
  sources, unsupported audience counts, ambiguous venue/origin, routing failures,
  new mail, and source eviction. No provider/brand-specific behavior branches.
- Component/hook tests with renderHook and direct component renders for list,
  detail, chat, report revisions, keyboard/focus, attached help descriptions,
  narrow-layout tabs, permission disclosure, and late-result invalidation. Keep
  slim App tests for navigation, source/task links, commands, and notifications.
- Regression coverage preserves current briefs, chat, task sync, transfer,
  usage reporting, and sender-HTML security. Recheck any expanded portable
  preferences against preceding transfer exports rather than weakening validation.
- Run `pnpm test` for frontend changes and `cargo test` in `src-tauri` for native
  changes. Run root `cargo test --workspace` if crates change. Run
  `pnpm test:e2e` if compose, triage, or email rendering changes; add deterministic
  agent-flow coverage without live paid services. Run the frontend build/lint
  checks appropriate to touched code and
  `node --test scripts/changelog.test.mjs` after changelog edits.

## Deferred work

Hosted/always-on execution; encrypted report replication and ownership across
devices; write-capable tools and reviewed actions; additional agent workflows;
automatic invitation classification/delegation; recursive agent spawning;
arbitrary plugins/code; and virtual-email delivery. Preserve extension points
for typed workflows and artifacts without implementing a general agent platform
before the first workflow is useful.
