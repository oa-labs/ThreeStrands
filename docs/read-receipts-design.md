# RFC 8098 read receipts and privacy

Status: proposed, 2026-10-12. This document plans implementation; it does not
enable receipts or change application behavior.

## Product contract

Add three independent capabilities: request a receipt when composing, interpret
receipts received for sent mail, and respond to a request on an incoming message.
Receiving and requesting receipts must remain usable when a provider cannot
safely submit responses.

The recommended first release enables receipt interpretation, offers an
unchecked per-message request control, and leaves incoming responses off.
Users can opt into asking for each response. Automatic responses are a later
milestone with additional authenticity and coordination gates.

Persist `requestReceipt` on the draft with a backward-compatible false default.
Freeze it when queueing, including scheduled sends; cancellation/return-to-draft
preserves the user's choice. A new reply or forward never inherits a sender's
request. Build the destination header from the chosen authorized From identity,
not a free-form address. Initially request from all actual envelope recipients;
per-recipient selection needs separate submission variants and is deferred.
Do not request receipts for known newsgroup destinations. Explain that receiving
clients and account administrators may ignore requests.

Treat receipts as reports from another mail client. Show **Recipient reported
displaying this message**, not a verified read timestamp. Absence of a receipt
means **No receipt received**, never **Unread**. Receipts must not complete
follow-up tasks, change delivery success, or trigger other automation.

## Standards baseline

The normative reference is [RFC 8098](https://www.rfc-editor.org/rfc/rfc8098),
especially sections 2, 3, 5, and 6. Core constraints are consent, no receipt
loops, one response per recipient/message, address checks before automatic
sending, multipart/report encoding, and a null SMTP reverse path. A displayed
report does not establish that a human read or understood the message.

Use [RFC 3503](https://datatracker.ietf.org/doc/html/rfc3503) for IMAP receipt
markers: inspect permanent-keyword support, respect `$MDNSent` and `\Draft`,
preserve markers across copying, and never clear `$MDNSent`. Its legacy
disposition examples do not override RFC 8098.

Maintain a clause-to-code/test checklist during implementation. Generation
starts with `displayed`; parsing recognizes the current disposition vocabulary
and preserves unsupported or legacy values without treating them as displayed.
Internationalized receipt generation under RFC 6533 is a separate extension;
unsupported addresses remain visible and do not get silently rewritten.

## Existing foundations and gaps

| Area | Current code | Planned change |
| --- | --- | --- |
| Shared incoming MIME | `src-tauri/src/mime.rs`, `provider/imap/rfc822.rs` | Extract requests and reports before ordinary body normalization; retain raw metadata |
| Message ingestion | `src-tauri/src/db/ingestion.rs` | Store parsed metadata and receipt associations transactionally |
| Compose and durable sends | `src-tauri/src/correspondence.rs`, `src/Composer.tsx`, `src/correspondence.ts` | Freeze an explicit receipt-request choice and record sent recipient identities |
| Provider contracts | `src-tauri/src/provider.rs` | Separate request-header support, report parsing, response transport, and coordination capabilities |
| SMTP | `provider/imap/send.rs`, `provider/imap/smtp.rs` | Add a typed null reverse path reserved for validated receipt jobs |
| Privacy controls | `src/PrivacySettings.tsx`, native settings storage | Add native-authoritative, device-local response policy |
| Read state | `src/useThreadMutations.ts` | Keep mailbox read mutations independent from receipt authorization |
| Transfer and replication | `transfer.rs`, `sync_projection.rs`, `src/syncedPreferences.ts` | Explicitly exclude consent, automatic-send grants, and executable receipt jobs |

Current SMTP derives MAIL FROM from the authenticated message From identity and
rejects an empty sender. Do not weaken that ordinary-send invariant. Introduce
`ReversePath::Mailbox` and `ReversePath::Null`, with the null variant reachable
only through native construction of a validated MDN submission.

The Gmail REST sender currently submits raw MIME and an optional thread ID.
The documented [send API](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.messages/send)
accepts a Message resource and exposes no explicit envelope-sender argument.
That does not establish the on-wire reverse path. Treat compliant Gmail
response submission as unproven and disabled until there is documented and
verified support. Do not substitute a forged Return-Path header or silently add
SMTP credentials/scopes. Gmail can still request and interpret receipts.

## Privacy settings and precedence

Store response settings in Rust/SQLite rather than trusting a webview boolean
at delivery time. Present the effective policy under Settings → Privacy →
Read receipts, with links from account settings.

| Control | Default | Meaning |
| --- | --- | --- |
| Incoming responses | Never | Silently ignore requests; show no unsolicited prompt |
| Ask for each message | Off | Offer a quiet message-level action after display |
| Automatic responses | Off; unavailable initially | Later, exact-sender rules on explicitly enabled accounts only |
| Account response policy | Inherit | An account can be more restrictive than the device policy |
| Request on outgoing message | Off | Explicit choice for this draft; replies/forwards start off |
| Receipt history | Local | Minimal activity and reported outcomes; no new replication by default |

Never is a hard stop: account and sender rules cannot override it. Within an
enabled mode, account restrictions and explicit sender blocks win over allows.
Model this as a pure policy evaluator returning an outcome and a safe reason
code. Evaluate the same rules when showing actions, queueing, claiming, and
immediately before beginning submission.

Do not add domain-wide allows, address-book-wide trust, or an Always send to
everyone option in the initial design. An address-book entry is not proof of
sender authenticity. A later allow rule applies to the exact parsed sender
address, the chosen account, and the required authenticity evidence.

Changing to Never or blocking a sender cancels affected unsubmitted jobs in
the same native transaction as the setting change. Dismiss any pending prompt.
Revocation and delivery claims share a gate so their ordering is defined. The
UI must state when a submission has already started and cannot be recalled.
Re-enabling a policy does not release canceled work or approve historical
requests. New accounts and restored installations always start at Never.

Responses, image loading, and mailbox read state have independent controls.
Sending a receipt must not load images, unblock backgrounds, invoke sender URLs,
or grant trust to later messages. The privacy help should explain that remote
image requests and other mail clients can reveal activity independently of MDNs.

## Request and report parsing

Add a provider-neutral native module, suggested as `src-tauri/src/mdn/`, with
separate parsing, policy, persistence, MIME generation, and dispatch modules.
Reuse the shared MIME/header parser, bounded traversal, and envelope validation;
do not use substring matching or provider-specific template selectors.

For requests, preserve header multiplicity and parsed mailbox lists, options,
Return-Path, Message-ID, Original-Recipient, and the evidence used to identify
the receiving identity. Use only the actual message's top-level request fields;
a forwarded `.eml`, attached report, or message/partial fragment cannot authorize
a response for its enclosing message.

Handle folding, comments, quoted strings, casing, malformed fields, and
duplicate singleton headers explicitly. Ambiguous requests remain ordinary
mail and are ineligible for sending. Unknown optional options can be ignored;
unknown required options block generation. Do not invent extension behavior.

For reports, validate the multipart/report type, report-type parameter, report
part position, required fields, and bounded field syntax. Extract the structured
report independently of the display body. A missing original Message-ID prevents
automatic association, not ordinary viewing. Unknown extensions stay inert;
invalid or incomplete reports never update a sent-message indicator.

Use a dedicated RFC address comparator: preserve local-part case, canonicalize
quoted local parts, and compare domains without case sensitivity. Do not reuse
the current whole-address lowercase identity helper for this safety check.
Do not strip plus tags, dots, or apply guessed provider alias rules.

Centralize MDN parser byte/count, job, and rate limits in native policy constants
with shared validation and below/exact/above tests. Reuse existing MIME/SMTP
limits where applicable. Any rendering-related limit belongs in
`src/emailRenderingPolicy.ts` under the repository's rendering contract.
Over-limit receipt metadata blocks receipt processing without discarding a
message that is otherwise valid under the existing ingestion limits.

## Consent and display evidence

Keep receipt UI in the trusted reader chrome, outside the sandboxed sender HTML.
Never expose a send command to the iframe. A native command accepts a message
reference and expected policy/request revision; it reloads and validates all
destinations and identities itself. It never accepts arbitrary receipt MIME,
recipient addresses, or a nullable envelope sender from JavaScript.

When Ask is enabled, an eligible, displayed message offers **Send receipt…**
and **Don't send for this message**. Closing the reader is not consent. Use a
confirmation with the destination addresses, sending identity, and a short
explanation of the information disclosed. Default focus stays off Send.
Mismatch, missing Return-Path, or multiple destinations requires conspicuous
per-message confirmation regardless of any future allow rule. Never silently
redirect a receipt to From or Reply-To.

Choose only an authorized account identity. For an alias or Bcc delivery,
preview the exact public address that would be revealed; never silently replace
it with the account's primary/login address. If identity selection is ambiguous
or the provider cannot send as it, leave the request unanswered. Do not disclose
other recipients, linked accounts, or address-book data.

Define display evidence per message: the message body has rendered successfully
in an expanded reader while the app is foregrounded. Scanning the inbox, sync,
search snippets, notifications, AI summaries, mark-read shortcuts, bulk triage,
or the thread-level read timer do not qualify. Use a native-issued read-session
token tied to the message/request revision; the trusted reader acknowledges it.
This isolates authorization from incidental UI events; it is not proof of human
attention or protection against a compromised application process.

First-release exclusions: drafts, known sent copies, reports, fragments,
quarantined/spam messages, list/newsgroup messages identified by semantic
headers, invalid requests, and messages without a usable deduplication identity.
Do not branch on sender brand, domain, provider CSS classes, or a captured template.

## Minimal receipt payload

Implement a dedicated builder and byte-level fixtures before enabling any send.
Its output consists of a generic text explanation and a structured report.
Include the final recipient, disposition, original Message-ID when present,
and reliable original-recipient information when available. Do not infer the
original recipient from To/Cc or manufacture a value from a login address.

Generate a fresh opaque Message-ID with no hostname or installation identifier.
Use a generic subject that does not repeat the original subject. Omit the
optional original-message attachment, original headers/body, Reporting-UA,
device/client version, OS, IP, local hostname, language, tracking links, and
custom exact-display timestamps. Standard transmission metadata still exposes
the send time and routing; explain this in the confirmation.

The per-message action uses manual-action/MDN-sent-manually and displayed.
Later automatic sending must have an explicitly reviewed disposition-mode
policy. Do not emit legacy denied/failed responses, deletion notices, forwarding
notices, or receipts that themselves request another receipt.

Send only to the validated requested destinations and use a null SMTP reverse
path. Keep the From identity authorization, TLS/pinning, recipient validation,
dot-stuffing, size limits, and definite-versus-uncertain delivery semantics.
Reject unsupported internationalized envelopes rather than downgrade them.

## Durable jobs, crash safety, and coordination

Add additive tables for parsed request/report metadata, local response decisions,
receipt submissions, outgoing request recipients, and incoming report events.
Use unique constraints and atomic state transitions. Reuse delivery primitives
and the ordinary undo interval, but keep receipt jobs separate from editable
drafts, scheduled sends, follow-up completion, and send-and-archive effects.

```text
eligible -> suppressed
eligible -> approved -> undo_pending -> reserved -> submitting
undo_pending -> canceled
reserved -> blocked | submitting
submitting -> sent | definitely_not_sent | uncertain
uncertain -> sent_confirmed | unresolved
```

Bind approval to account, recipient identity, message identity, requested
destinations, request digest, policy revision, and local installation owner.
A changed request, restored profile, changed owner, or canceled policy requires
new consent. A replicated record cannot create approval or a sendable job.

Crash after reserving but before recording a result is not permission to resend.
Persist submission intent before any delivery bytes. Once delivery might have
occurred, keep uncertain outcomes terminal for dispatch: reconciliation may
confirm acceptance but absence from Sent never authorizes another submission.
Retry only a definitively unsubmitted attempt under still-valid consent and
the same local reservation; do not reinterpret an existing marker as a new job.
Recovery after restart or a long offline interval holds unsubmitted approvals
for renewed confirmation rather than releasing them unexpectedly.

Deduplicate across mailbox moves and copies using an account-scoped logical
message identity, recipient identity, exact parsed Message-ID, and a bounded
content/request fingerprint. Provider message IDs alone are insufficient.
Missing or conflicting identifiers block sending rather than enable a fallback
that could send twice. Define duplicate-ID collision handling conservatively.
Keep duplicate suppression separate from display-history retention.

For IMAP, check all known copies for `$MDNSent`, then reserve a canonical copy
before submission and propagate the marker to known copies. A reread after an
unconditional STORE is not a lock: two clients can both succeed. The recommended
first-release strong coordination tier requires a tested conditional flag update
using CONDSTORE/UNCHANGEDSINCE on a shared canonical copy, or another demonstrated
atomic provider primitive. A failed or ambiguous reservation sends nothing.
Preserve the marker even when delivery is uncertain or the user cancels after
reservation; avoiding duplicates takes precedence over guaranteeing a receipt.

Different canonical copies, independently copied messages, and other clients
that ignore the marker can still defeat coordination. Document the guarantee
as local at-most-once submission plus coordinated suppression on supported
mailboxes, never global exactly-once delivery. If a shared canonical copy cannot
be established, disable generation in the strong tier. Do not use eventual
replicated sync or an expiring lease as an exclusive-send lock.

Explicit Don't send decisions persist locally; when supported, mark the message
to suppress future requests from cooperating clients. Merely running the default
Never policy does not modify provider flags. Keep the marker out of user labels
and preserve it through native COPY/MOVE and applicable APPEND paths, including
Sent copies. A marker-save failure for a response must be surfaced without
initiating that response. Unsupported receipt capabilities must not prevent
ordinary compose, Sent filing, or mailbox operations from working.

The currently documented primary IMAP capability set lacks both arbitrary
keywords and CONDSTORE. Generation is therefore blocked under this recommended
strong tier unless new evidence establishes a safe mechanism. Requesting and
receiving still work. Do not imply every existing account gains all three
capabilities; display an actionable capability reason. A weaker single-device
response mode is a separate product decision, not an automatic fallback.

## Correlating incoming reports

At outbound queue time, record the frozen RFC Message-ID, account, chosen From,
actual envelope recipients including privately stored Bcc, request flag, and
operation identity. Older sent mail may remain unassociated when equivalent
recipient evidence cannot be recovered safely.

Match a report to an account-scoped sent message by original Message-ID and
plausible original/final recipient evidence. Do not match by subject or provider
thread ID alone. Distinguish matched claims, unassociated reports, conflicting
reports, and unsupported legacy reports. Do not hide the original receipt email.
Alias forwarding can produce several reports; store events rather than overwrite
a single universal read boolean. Deduplicate repeated ingestion and receipt
copies without discarding conflicting evidence.

Treat every report field as untrusted text. Provider-delivered authentication
evidence can inform an explanation, but matching IDs or a DKIM pass is not proof
that the named human read mail. Do not trust sender-injected Authentication-Results
headers or make authenticated/unverified claims without a provider-specific
trusted provenance boundary. Request destinations and received-report sender
identity checks are separate tests.

Show **Receipt received at …** using local arrival evidence and label any
reported date separately. Keep Bcc identities visible only to the sending user;
never include the complete recipient list in an MDN or shared summary.

## Sync, export, retention, and audit

Keep sending permission, automatic rules, approvals, request display tokens,
deduplication secrets, and executable jobs device-local and excluded from both
settings transfer and replicated preferences. New/imported settings cannot
enable response sending. Explicitly exclude these keys from the generic portable
preference merge; partial/stale snapshots cannot restore an allow.

The first release does not change encrypted settings format 5. Add exclusion
regressions, including imports from the frozen preceding schema. If portable
receipt preferences are introduced later, use optional fields with restrictive
defaults, freeze an export from the immediately preceding schema, and continue
supporting all earlier imports. Exporting preferences is not exporting consent.

Keep a local activity view showing suppression/approval, destination and chosen
identity, submission state, and a safe reason code. Do not log message bodies,
raw MIME, secrets, Bcc lists, or full server errors. Display-history deletion
does not erase suppression markers. Retain compact keyed deduplication tombstones
across ordinary mail-cache retention; treat them as sensitive metadata and
exclude them from settings export. Define key loss/profile reset as a loss of
local duplicate history, with generation disabled until account state has been
reconciled. Factory reset cannot promise to erase copies already sent or retained
by providers and recipients.

## Delivery sequence and release gates

1. **Parser and evidence model.** Add request/report extraction, database
   migrations, recipient/message identity rules, and inert incoming-report UI.
   No response network traffic. Preserve ordinary mail and renderer behavior.
2. **Outgoing requests.** Add the per-draft flag, native validation, frozen MIME,
   request tracking, demo support, and compose controls. Test undo, scheduled
   send, identity changes, attachments, drafts from older versions, and Bcc.
3. **Transport and coordination proof.** Add typed null reverse path and a
   dedicated receipt job. Prove on-wire output, provider acceptance/rejection,
   atomic reservation, move/copy handling, crashes, and simultaneous clients.
   Unproven capabilities stay disabled with reasons.
4. **Manual consent and privacy settings.** Enable Never/Ask, trusted reader
   actions, identity/destination preview, cancellation, native rechecks, local
   audit, and exclusion from sync/import. Only proven transports can submit.
5. **Optional automatic rules.** Add exact-sender rules only after trusted
   authenticity evidence exists. Require foreground message display, matching
   request/Return-Path, a single destination, authorized identity, strong
   coordination, and explicit local account/sender opt-in. No background catchup.
   Apply account/destination rate limits and pending-job quotas before approval.
   Exceeding a limit suppresses the response; it never creates a delayed send.
   A sender-controlled Authentication-Results field or missing trusted evidence
   cannot satisfy this gate, even for an allowlisted address.

Release each shipped feature with an appropriate minor bump from the then-current
version, matching package.json, tauri.conf.json, Cargo.toml, and the threestrands
Cargo.lock entry. This planning-only change requires no version bump.

## Automated acceptance coverage

Extend existing MIME, ingestion, provider, settings-transfer, compose, reader,
and sync suites, with focused MDN module tests where new contracts live.

| Area | Required cases |
| --- | --- |
| No disclosure by default | Open, expand, mark read, bulk triage, refresh, restart, restore/import, join sync, and AI analysis produce zero receipt submissions |
| Parsing | Folded/commented/quoted headers, local-part case, domain case, plus tags, duplicate singleton fields, missing IDs, malformed reports, optional/required options, limits |
| Structural isolation | Attached `.eml`, nested report, fragments, and quoted text cannot authorize enclosing-message responses; at least two different MIME fixture shapes |
| Consent | Never precedence, per-account restrictions, focus/Escape, mismatch/multiple destinations, alias/Bcc disclosure preview, stale revisions, revocation racing dispatch |
| Wire privacy | Null reverse path; correct authorized From and destinations; no original subject/body/headers, device identifiers, UA details, request headers, or extra recipients |
| Send security | Header/SMTP injection, malformed/control bytes, unauthorized identity, Unicode downgrade, TLS/pin failures, blocked destinations, and no renderer/proxy bypass |
| Durability | Double click, concurrent worker, crash at every state boundary, cancellation, definite failure, acceptance with failed local commit, uncertain result, offline/restart |
| Coordination | Two sessions competing for reservation, stale MODSEQ, missing keyword support, STORE rejection/ignored STORE, existing marker, multiple copies, UIDVALIDITY reset |
| Incoming evidence | Forged/unsolicited reports, cross-account IDs, unknown recipients, aliases, legacy/unknown dispositions, conflicting reports, repeated ingest; no task automation |
| Compatibility | Old drafts default request off; old database upgrades; frozen preceding transfer fixture; no credentials, jobs, consent, or permissive rules exported/replicated |
| Rendering | Receipt human text and malicious HTML preserve sandbox/CSP and blocked-source flow; original message attachments never replace the report's explanation |

Run `pnpm test` for frontend changes, `cargo test` in src-tauri for native changes,
and `pnpm test:e2e` when compose, triage, or email rendering changes. Use automated
SMTP/IMAP fixtures and concurrency/fault injection for the transport gates.
Do not start the app, use a browser, or take screenshots for agent verification;
hands-on UI and real-provider wire checks belong to the developer.

Completion means the privacy-negative tests are green, every enabled provider
tier has demonstrated transport/coordination behavior, unsupported tiers are
honestly labeled, old settings/drafts import safely, and disabling responses
prevents every still-cancelable disclosure.
