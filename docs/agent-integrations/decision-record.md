# Agent integration contract v1

**Decision date:** 2026-11-02. **Baseline:** 0.194.5.
**Scope:** Phase 0 of [the delegated-agent plan](../delegated-agents-plan.md).
**Status:** Offline contract selected and fixtures verified. No live adapters,
credentials, paid calls, account enrollment, or application behavior added.

## Selected integrations

| Capability | First supported contract | Reason |
| --- | --- | --- |
| Model | Direct Anthropic Messages, `claude-sonnet-4-6` | Fits the current native provider protocol and has documented client tool calls. Use a fixed model ID and an isolated agent adapter. |
| Optional paid search | Exa `POST /search`, explicit `type: auto`, five results, highlights | User prefers Exa. Configurable independently of the model with the user's own API key. |
| Page reading | Native HTTPS text reader | Enforces connection policy and source provenance under app control. |
| Routing | Disabled, typed `not_configured` result | No routing provider or precise-location disclosure is approved by this contract. Research can proceed with travel explicitly unavailable. |

The model choice is a narrow compatibility baseline, not a claim that it is the
newest or best model. Anthropic currently labels Sonnet 4.6 legacy; changing it
requires adapter fixtures and availability checks. Newer Sonnet 5.5 has different
thinking/tool behavior. Do not use a moving `latest` alias or silently substitute
a model. Sources: [Sonnet 4.6](https://platform.claude.com/docs/en/models/sonnet-4-6/overview),
[Sonnet 5.5](https://platform.claude.com/docs/en/models/sonnet-5-5/overview).

Existing briefs and chat continue to use their current provider settings and
fallback behavior. Agent v1 supports only the selected model adapter; other
providers return `unsupported` during preflight. This is not permission to change
the user's current provider or copy its key to another endpoint.

Brave was evaluated but is not part of v1. Its published terms restrict search
result persistence, and its product page requires storage rights for storing
results. Tavily was also evaluated; its privacy policy allows query improvement
and possible sharing with other indexes. Neither is an automatic fallback.
Sources: [Brave terms](https://api-dashboard.search.brave.com/terms-of-service),
[Brave plans](https://brave.com/search/api/),
[Tavily privacy](https://www.tavily.com/privacy).

## Optional paid-search configuration

Add a separate agent configuration with `searchProvider: none | exa`, default
`none`. Selecting Exa and saving a key enables the capability for explicit jobs;
it never starts a job. Show the provider, estimated per-query cost, cumulative
allowance, and disclosure before first use. Free provider credits may cover
usage, but the app must not promise free searches or manage billing/auto-recharge.

With search off, invitation web research fails preflight with
`search_not_configured`; existing reports and grounded chat over saved evidence
remain usable. Do not advertise a fresh researched report when web access is
unavailable. Future non-web workflows can declare different required capabilities.

Use OS-keychain service `app.threestrands.mail` with separate accounts
`agents-anthropic-api-key` and `agents-exa-api-key`. These are new planned entries;
the existing `ai-provider-api-key` remains untouched. Reusing an existing
Anthropic key must be an explicit native operation and only when its provider
identity matches. Never expose secrets in React preferences, fixtures, snapshots,
URLs, logs, sync, or settings exports. Delete keys on explicit disconnect and
pause affected jobs; don't delete unrelated AI credentials.

Search grants are per task. Revoking Exa, disabling search, or exhausting credits
prevents further dispatches; reading old results remains possible. A provider
change does not transfer grants or restart old jobs automatically. No subscriptions
or service terms are accepted by this phase.

## Wire contracts

### Anthropic

POST to `https://api.anthropic.com/v1/messages` with `x-api-key`, JSON content
type, and `anthropic-version: 2023-06-01`, matching the existing version baseline.
The version remains documented; v1 omits temperature/top-p/top-k to ease a future
version migration. Use nonstreaming messages and
`tool_choice: {type: auto, disable_parallel_tool_use: true}`. Do not enable
thinking, prompt caching, server tools, files, batch APIs, or provider-managed
agents. Sources: [API versions](https://platform.claude.com/docs/en/api/versioning),
[parallel tool control](https://platform.claude.com/docs/en/agents-and-tools/tool-use/parallel-tool-use).

Keep assistant content blocks and each `tool_use.id`. Accept at most one action
per response. Validate its name, argument schema, task grants, and current worker
generation before dispatch. Append the assistant message and an immediately
following user `tool_result` with the same `tool_use_id` before any later user
turn. If a redirecting instruction arrives mid-call, finish or cancel the
pending protocol exchange before incorporating it. Unknown blocks/actions, invalid
arguments, or unmatched IDs never execute. `max_tokens` is truncation, not success;
`end_turn` is chat text, not an automatically published report. Source:
[tool-call lifecycle](https://platform.claude.com/docs/en/agents-and-tools/tool-use/handle-tool-calls).

The transport fixture freezes `search_web(query)` and its continuation. The
remaining planned actions (`read_page`, `ask_user`, `publish_artifact`) need native
argument/report schemas and workflow tests in their implementation phases. The
phase-0 oracle deliberately refuses them rather than inventing incomplete
workflow schemas. The current forced-schema-output tool in `ai/provider.rs` must
not become an execution path; retain a separate agent parser/transport.

### Exa

POST `https://api.exa.ai/search`, `x-api-key` header and JSON body:

```json
{
  "query": "Example Summit official speakers",
  "type": "auto",
  "numResults": 5,
  "contents": { "highlights": true }
}
```

Use `results[].url`, `title`, optional `highlights`, optional published date,
`requestId`, and optional `costDollars`. Treat highlights as search excerpts,
not first-party page verification. Ignore images, favicons, scores, and other
unknown optional metadata. Missing highlights become an empty excerpt; malformed
results fail closed. An explicit empty result array is valid. Invalid destination
URLs are rejected before they can be read. Source:
[Exa Search API](https://exa.ai/docs/reference/search).

Do not call Exa Agent, Answer, Contents, Monitors, or deep search modes. A model
cannot change search mode, results count, endpoint, or paid extras. Use one
credential-wide request/second limiter initially, below documented default team
quotas; honor account-specific throttling. `402` pauses for credits/key budget,
not automatic retry. Sources: [billing and limits](https://exa.ai/docs/admin/billing),
[pricing](https://exa.ai/pricing).

## Cost, disclosure, and persistence eligibility

The pricing snapshot in [contract-v1.json](contract-v1.json) is dated, not a live
billing promise. Model estimates use $3/million input tokens and $15/million
output tokens; search estimates use $0.007 per auto request with at most five
results. Twenty searches therefore reserve $0.14. The task token allowances
reserve up to $1.20 for model calls at this snapshot, excluding taxes, account
agreements, or provider price changes. Free credits do not reduce reservations.
Sources: [model prices](https://platform.claude.com/docs/en/about-claude/pricing),
[Exa prices](https://exa.ai/pricing).

Reserve the allowed input and output before each dispatch, counting protocol
overhead with the provider tokenizer/token-count capability when implemented.
Retain reservations for ambiguous interrupted requests. Reconcile reported
tokens/cost when available, track missing usage as unknown, and never interpret
an estimate as an exact invoice or advertised hard dollar cap. Apply task totals
across retries, revisions, and follow-ups. A changed pricing basis needs an
explicit snapshot update; unknown pricing disables dollar-ceiling claims but
does not disable token/call enforcement.

Only the model receives selected bounded email context, user instructions, and
chosen business profile. Search receives minimal public entity queries; never
send mail bodies, private customer lists, home addresses, or credentials. Native
query policy must constrain public facts; a prompt or regex cannot prove that
arbitrary free-form model text is safe. Before real dispatch, use user-reviewed
public search subjects and a constrained query builder. Page hosts receive the
user's IP and requested URL. Search setup must explain that provider retention
depends on the account agreement; do not claim zero retention on a normal key.
Anthropic describes standard API deletion within 30 days with exceptions. Exa's
privacy policy defers business-customer processing to customer agreements and
its pricing lists ZDR as an enterprise capability. Sources:
[Anthropic retention](https://privacy.claude.com/en/articles/7996866-how-long-do-you-store-my-organization-s-data),
[Exa privacy](https://exa.ai/privacy-policy), [Exa plans](https://exa.ai/pricing).

Provider storage rights and third-party page rights are not interchangeable.
Exa's public terms grant documented API use but also restrict copying/derivative
uses subject to permitted exceptions; this record does not establish blanket
rights to persist provider output. Phase 3 first enforced this with a free-text
"storage agreement" field that preflight required to be non-empty
(`persistence_rights_required`). That was later removed: any text satisfied it,
so it established nothing and only blocked setup. Whether a provider's terms
allow saved excerpts is between the user and that provider, as with any other
API key the user brings. Setup instead states retention plainly: raw evidence
expires after seven days, and reports and cited excerpts remain until deleted. Source:
[Exa terms §§1.1, 4.1–4.2](https://exa.ai/assets/Exa_Labs_Terms_of_Service.pdf).

Local raw search/page payloads expire after seven days where persistence is
permitted. Retained citations use bounded excerpts and retrieval timestamps;
store no raw HTML indefinitely. Export/sync remains excluded as in the parent
plan. The app cannot promise deletion from providers or existing local backups.

## Frozen policy and failure semantics

All concrete values live in `contract-v1.json`. Freeze them for the first
implementation; port them into one native policy module and test parity with
this fixture before production. They are app limits, not provider guarantees.
Two jobs may run globally, but each has one dispatch at a time. The task allowance
is 20 model calls, 20 searches, 30 page calls, 200k input tokens, 40k output tokens,
and 900 active seconds. Input/output per model call is bounded to 24k/4096 tokens.
Queries are bounded to 400 Unicode scalar values. Responses are capped at 2 MiB,
extraction at 30k characters/page, and stored evidence at 5 MiB/task. Waiting,
queue, and offline time do not consume active seconds. Dispatch attempts and
retries count toward allowances even when their bill is uncertain.

| Outcome | Required behavior |
| --- | --- |
| 2xx | Parse/validate; HTTP success is not contract success. |
| 400/404/422 | Unsupported/invalid; no fallback that drops validation or changes model. |
| 401/403 | Pause for credentials/access; no repeated secret-bearing requests. |
| 402 | Pause for provider credits or budget; no automatic purchase or recharge. |
| 408/429/500/502/503/504/529 | At most two additional attempts under remaining allowances. |
| Interrupted after dispatch | Ambiguous; preserve reservation and checkpoint; no claim of exactly-once billing. |
| Cancelled | No new dispatches or publication. |
| Malformed/truncated response | No tool dispatch/report publication; expose a bounded failure. |

Automatic retry delay is `max(2 * 2^retryIndex, Retry-After seconds)` with delays
up to 300 seconds. Longer, malformed, or HTTP-date Retry-After values pause for
review rather than retrying earlier than requested. The production scheduler
stores `not_before`, releases its worker slot, and permits cancellation during
backoff. Use an injected clock; no blocking sleep. Inspect response bodies only
for bounded known codes; don't log raw provider errors. Sources:
[Anthropic errors](https://platform.claude.com/docs/en/api/errors),
[Exa status codes](https://exa.ai/docs/reference/search).

## Page reader connection boundary

V1 permits only HTTPS on port 443 and `text/html` or `text/plain`, UTF-8/ASCII
encoding. Missing charset means UTF-8 with strict decoding; invalid bytes fail.
Reject PDFs, images, scripts, compressed content, and unsupported encodings
explicitly. This deliberately narrows the parent plan's HTTP(S) allowance. Do
not send Accept-Encoding; keep automatic decompression disabled. Bounded parsing
extracts text without executing code or fetching any subresources.

Reuse `http_client::builder` but create a separate client with no environment
proxy, cookies, automatic redirects, or authorization inherited from other
clients. Bound connect/request time to 10/60 seconds and redirects to three.
Reject signed/private/action URLs before dispatch; user-reviewed public search
subjects do not grant unrestricted URL-following rights.

For each hop: validate scheme/port/userinfo/hostname, resolve DNS, reject the hop
if any answer is non-public, pin the vetted addresses into that connection,
retain TLS verification/SNI for the hostname, verify the peer, and then send.
Revalidate relative and absolute redirects independently and never forward
credentials. Do not resolve once and let another connector resolve again.
V1 rejects IPv4-mapped IPv6, transition/NAT64 ranges, local/link-local/private,
CGNAT, multicast, reserved, and documentation networks. The existing
`net_safety.rs` is a useful starting point, but filters private DNS answers rather
than enforcing this all-answers policy and does not cover the full v1 range set;
do not claim it already satisfies the new contract.

The fixture harness supplies DNS answers and connected peers as a trace. It
checks that requirements reject mixed answers, rebinds, unsafe redirects, and
unsupported payloads. It does not test a real socket, TLS, DNS implementation,
HTML parser, timeouts, or OS proxy configuration. Those are mandatory native
integration tests before phase 3 dispatches real URLs. Phase 0 adds no network
security implementation to the shipped app.

## Fixtures, verification, and remaining release gates

Run from the repository root:

```sh
node --test scripts/agent-contracts.test.mjs
```

Fixtures under `fixtures/` are synthetic, based on the linked vendor
documentation, with example domains and keychain placeholders. They are not
captured live responses. `scripts/agent-contracts.mjs` is a development-only
reference oracle; the app must not import it. Native adapters must run the same
fixtures when implemented, plus native grant, schema, budget, and connector
tests. Fixture passing establishes an internally consistent contract, not
provider availability, useful research quality, or commercial entitlement.

Phase 0's deliverables are the selected adapter record, frozen policy, wire
request/response fixtures, negative cases, and offline conformance tests. Before
real jobs ship: verify configured account/model availability, implement all native policy enforcement,
run content-free live connection checks only through explicit setup, and assess
event/podcast report quality against the later workflow fixtures. No live-check
result is claimed here. Optional routing remains deliberately unavailable; a
future routing contract needs licensing, location disclosure, geocoding,
attribution, and traffic-model decisions before enabling it.
