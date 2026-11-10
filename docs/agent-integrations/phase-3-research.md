# Phase 3: real read-only research

Implemented for 0.198.0 on 2026-11-02. This adds the native research loop and setup
flow from [the plan](../delegated-agents-plan.md); it does not complete the
invitation-evaluation workflow in phase 4.

## Setup and delegation

**Settings → Web Research** (also reached from **Agents → Research Settings**)
holds a separate Anthropic key and the required paid Exa key in the
operating-system keychain, using the phase-0 service and
account names. Existing AI provider settings and credentials remain untouched.
Research and Exa search default off. Saving setup never queues work. Disconnecting
a key or editing setup invalidates in-flight generations and pauses affected jobs.

Setup does not ask for a storage agreement; whether provider terms allow saved
excerpts is between the user and the provider. **Check connections** explicitly performs an Anthropic model-availability request and one
public Exa search, disclosed as potentially chargeable. Jobs cannot start until
that configuration has passed the check. Automated tests do not perform these
live account checks or spend provider credits.

Choose **Web research** in the new-task form. Review up to five public subjects,
one per line, and accept the per-task disclosure. Native code builds queries from
those subjects and a fixed vocabulary; a model cannot search arbitrary text,
send the objective to Exa, expand its allowed subjects, choose paid extras, or
change endpoints. Every job remains scoped to its chosen mail account. Source
conversation and personal-task links must belong to that account.

Phase 3 sends only the user's objective and follow-up instructions to the model,
plus retrieved evidence. It does not read or snapshot the linked email body,
addresses, attachments, contacts, other accounts, or calendars. The source link
is navigation. Users must supply relevant context deliberately in the objective.
Selected-message snapshots, profiles, and event/podcast evaluation remain phase 4.
Routing and mailbox search are explicitly unavailable.

## Native execution and protocol

The fixed adapter is Anthropic Messages with `claude-sonnet-4-6`, API version
`2023-06-01`, nonstreaming output, and at most one validated client action per
response. Exa uses only `/search`, auto mode, five results and highlights.
There are no moving-model aliases, custom endpoints, fallback providers,
provider-managed agents, or write-capable tools.

A durable step performs token counting, one model request, one search, one page
read, clarification, or publication. Assistant tool-use IDs and immediately paired
user tool results survive checkpoints and restart. Unsupported blocks, duplicate
IDs, unknown tools, invalid arguments, truncation, and unmatched sources cannot
execute a tool or publish a report. Model end-turn text alone cannot complete
research successfully.

Search yields typed source IDs. Page reads accept a retrieved source ID, never
an arbitrary model URL. Reports use a structured summary, findings with source
IDs and verbatim supporting quotes, and explicit uncertainties. Native publication
checks source membership and excerpt containment. This establishes provenance,
not factual truth: the UI labels the assessment as agent judgment, distinguishes
search excerpts from retrieved pages, and exposes retrieval dates and sources.
Reports and conversation remain inert plain text; raw HTML and model-created
links cannot execute. Only native-validated retrieved source IDs can open the
system browser.

Ordinary follow-up discussion uses saved evidence and has no tool grants. It can
work with Exa disabled while the model remains configured. **Revise report** is
an explicit bounded research continuation with existing subjects and permissions.
Each publication creates an immutable version and a version-specific source map.
A failed revision retains earlier artifacts. Neither research nor discussion
changes mail unread state or completes a personal task.

## Reservations, recovery and activity

Policy values remain frozen against `contract-v1.json`: 20 model calls, 20
searches, 30 page reads, 200,000 input tokens, 40,000 output tokens and 900 active
seconds per task; 24,000/4,096 input/output tokens per model call. Calls, retries,
revisions and discussions share the same task ledger.

Token counting gates the actual request. Each model dispatch conservatively
reserves the full per-call input and output allowance before I/O, then reconciles
reported usage. Interrupted or malformed requests retain reservations and are
shown as unconfirmed. Search attempts count before I/O; missing or ambiguous
charges remain explicitly unknown. Missing prices are never described as a free
request or exact dollar ceiling. Setup shows dated estimates separately.

One credential-wide Exa limiter serializes search starts to one per second.
Transient responses use bounded durable backoff, respecting numeric Retry-After;
long, invalid or date-based delays pause for review. Authentication and credit
errors pause without purchases or automatic credential retries. Cancellation,
deletion, configuration edits and lease expiry cancel awaited work and invalidate
publication. A crash between dispatch and persistence may repeat a read or charge;
reservations survive and there is no exactly-once billing promise.

**Settings → AI → Activity** receives content-free model/search activity and
reported usage, preserving its existing retention limits. It records no objective,
query, source title, excerpt or mail subject. Per-job usage retains unconfirmed
reservations and charges that the existing activity format cannot represent.

## Page-fetch boundary and lifecycle

The native text reader permits public HTTPS on port 443 only. Credentials in
URLs, queries (including signed/token-bearing links), local hostnames and action
paths are rejected. All DNS answers must be public; mixed private/public answers,
reserved ranges, mapped/transition IPv6 and excessive answer sets fail closed.
Each hop pins the vetted addresses into a fresh TLS client, preserves hostname
verification and SNI, checks the connected peer, and revalidates redirects.

Clients have no environment proxy, cookies, inherited authorization, automatic
redirects, compression or subresource loading. Responses are limited to 2 MiB,
three redirects, UTF-8/ASCII HTML or plain text, and 30,000 extracted characters.
The existing inert HTML tokenizer extracts text without executing scripts.
Unsupported payloads fail clearly rather than acquiring another capability.
The sender-HTML iframe, image proxy and remote-image consent remain unchanged.

SQLite schema 96 adds device-local research state with an indexed expiry column.
Total persisted state is bounded to 5 MiB per task. Seven-day cleanup runs in
bounded worker batches, discards raw protocol/extraction text, and retains only
quoted evidence needed by saved reports. Expired unfinished work pauses; further
research requires a new explicit task. Grounded discussion can rebuild context
from retained reports and cited quotes, without restoring raw pages or tool grants.
Reports, user conversation and source
metadata persist until task/account deletion. Account removal cascades research
state and invalidates its workers.

Research settings, verification flags, jobs, sources, reports and keys are absent
from settings transfer and encrypted sync. Existing local database backups can
contain research data; deleting a live task does not erase old backups or
provider-held data. This task does not alter backup retention.

## Verification and limits

Native tests cover recorded model/search fixtures, report provenance, clarification
pairing, budget reservations and ambiguous recovery, duplicate dispatch rejection,
configuration revocation, late results, revision history, expiry, migration from
schema 95, and settings-export exclusion. Real local TLS/socket tests exercise
pinned connections, hostname validation, environment-proxy exclusion, redirect
refusal, compression rejection and exact response-size boundaries. Unit tests
cover mixed DNS answers, private/reserved IPv4/IPv6, unsupported types/encodings,
query grants, malformed model actions and retry semantics.

Frontend tests cover per-job consent and reviewed subjects, native command
payloads, password submission through keychain commands, preservation after failed
setup, and the existing demonstration interaction flow. The demo client never
uses live services; real research setup requires the desktop app.

Provider documentation was rechecked on 2026-11-02:
[Anthropic Messages](https://platform.claude.com/docs/en/api/messages),
[token counting](https://platform.claude.com/docs/en/api/messages/count_tokens),
[API versions](https://platform.claude.com/docs/en/api/versioning),
[Sonnet 4.6](https://platform.claude.com/docs/en/models/sonnet-4-6/overview), and
[Exa Search](https://exa.ai/docs/reference/search).
Sonnet 4.6 remains the fixed, legacy compatibility baseline selected in phase 0.
Live availability, account rights and useful invitation-report quality still need
account-specific checks and phase-4 evaluation; passing synthetic fixtures is
not certification of those things. Hands-on app verification belongs to the developer.

Recorded automated verification:

- `pnpm test`: 217 files, 3,179 tests passed after combining the UI polish and current master.
- `cargo test`: 2,076 passed; 7 existing ignored tests.
- `pnpm test:e2e`: 202 passed.
- Production build, ESLint, and TypeScript passed.
- Offline integration fixtures: 31 passed; changelog contracts: 30 passed.
