# AI requests and evaluations

General AI features use one native request lifecycle. Delegated Web Research
retains its separate runner, credentials, permissions, budgets, and cancellation.
No settings-export or sync format changes are involved: these controls and the
request journal are device-local.

## Request controls

Every frontend general-AI invocation receives a UUID. The native command carries
it through its purpose scope; it is never an instruction or a model-selected ID.
Cancel buttons send that ID to native code and keep awaiting the original IPC
promise. Cancelling aborts a queued request or drops the in-flight reqwest future;
it cannot recall material already received by a provider or guarantee zero billing.
A cancellation arriving before native registration is retained briefly so it
cannot lose that race. IDs cannot be reused while active.

The shared native policy allows two executing requests and eight running/waiting
requests. Queue waits expire after 30 seconds; execution expires after 150 seconds,
including fallback and retry work. Individual HTTP requests retain the 120-second
provider timeout. All permits and registrations are released on failure, timeout,
cancellation, and dropped futures. Contact enrichment's successive batches share
one lifecycle and allowance. Unrelated requests do not cancel each other.

There are at most two transient retries across a provider call's fallback sequence,
for explicit HTTP 429, 502, 503, and 504 responses. Retries retain the schema and
reasoning options. Retry-After is honored up to five seconds; longer, malformed,
and unsupported values are returned as errors. Without a header the delay is one
second. Transport failures/timeouts are not automatically replayed, because the
provider may already have generated an answer. Existing parameter-compatibility
fallbacks remain separate; every HTTP attempt reserves cost before dispatch.

Settings → AI Provider → Request Controls offers an optional daily estimated
spending limit. Native code uses the configured provider/model prices, reserves
UTF-8 input bytes plus schema/framing overhead and the full output-token allowance,
and refuses unpriced requests when a limit is enabled. On success it settles with
reported cost or reported token usage at the selected prices. Missing usage,
interrupted requests, and uncertain failure billing retain their reservations.
A retry/fallback needs its own reservation. Concurrent requests cannot reserve
the same remaining allowance. Changing the limit does not reset spent/reserved
cost. Reservations are saved before dispatch and survive restarts.

This is a device-local estimate, not a provider billing cap: external usage,
incorrect prices, tokenizer/protocol overhead, model-specific billing and provider
adjustments can differ. Enter both prices deliberately (zero means free); configure
provider-side limits too when a hard financial limit is required. A provider
report above an estimate is retained and blocks subsequent work under the limit.
The daily allowance rolls over at the device's local midnight.

## Context and provenance

Every provider call passes through `PreparedContext`. It owns the exact outgoing
payload and a versioned `ContextManifest`: native account/thread scope when
available, JSON-pointer source references, existing message/thread IDs, text
lengths, SHA-256 content revisions, and explicit attachment-truncation flags. A
plain-text summary has one thread excerpt rather than invented message IDs.
Unknown account/source identifiers remain unknown. Feature-specific selection,
consent and quote-removal policies still bound the material before it enters this
contract. Unselected attachment text is absent; the manifest never fetches it.
Source entries describe the selected payload, not all mail or a complete file.
The transient payload is deliberately not serializable as a journal record.

All feature calls name a native `Feature` with prompt and schema versions. The
journal also fingerprints actual system instructions, the actual schema used
(including fallback without a schema), endpoint, outgoing context, and response.
It records each attempt and the logical request outcome, so an HTTP retry and a
rejected answer are distinguishable. It stores no mail bodies, profile text,
questions, goals, instructions, credentials, raw errors, or provider responses.
The model name is the requested model identifier; providers do not necessarily
report an immutable resolved model revision.

The owner-readable, atomically replaced journal and budget file is
`ai-requests.json` in app data. History is bounded to 30 days, 500 attempts and 8 MB.
Unreadable budget storage fails closed. Settings offers explicit recovery that preserves the unreadable file and resets controls only after the user acknowledges that unknown reservations will be lost. Unknown top-level fields are tolerated on downgrade. Settings shows recent attempts and their context/version metadata.
Journal expiry does not reset the current day's allowance.

Cached summaries and suggestions keep their own provenance in the same SQLite
transaction as the winning result, guarded by the existing message revision.
Late results cannot replace newer text or its provenance. These records outlive
the journal and cascade with thread/account/cache deletion. Older caches have
unknown provenance; they are not labelled as newly generated or automatically
resent. Legacy writers clear provenance when replacing the corresponding result.
Changing a prompt or model does not silently regenerate previously cached mail.

Bump the feature's prompt version when instructions/interpretation change, and its
schema version when the response contract changes. Fingerprints detect changes
even if a version bump is missed. This is generation provenance, not proof that a
claim is correct. Existing evidence and structured-output validation remains in
force. No Notes AI, note exclusion metadata, semantic retrieval, or autonomous
writes are introduced here.

## Evaluation foundation

`evals/ai/cases.json` contains synthetic cases across summaries, suggestions,
briefs, chat, reply generation, draft review, contact enrichment and Find Times.
They cover revised facts, explicit and ambiguous deadlines, Spanish requests,
multiple events, unknown event times, DST, superseded requests, missing context,
retrieved sources, truncated attachments, prompt injection and Writing Profiles.
They contain no customer correspondence and are reviewed in git.

Run from the repository root:

```sh
pnpm eval:ai --output /tmp/ai-replay.json
```

Replay exercises real production prompt builders, provider completion handling,
parsers and deterministic graders with checked-in synthetic outputs. It makes no
network requests, reads no mailbox/database/keychain, and is covered by the normal
Rust suite. **Replay passing is not evidence of model quality.** Replay outputs are
fixtures for checking the harness, not gold-standard responses observed from a model.

Explicit live runs use an environment key and send only those synthetic inputs:

```sh
# Set THREESTRANDS_EVAL_API_KEY securely in your environment first.
pnpm eval:ai --live --provider openai --model MODEL --output /tmp/ai-model.json
# Optional: --provider custom --endpoint URL, or --case SUBSTRING
```

Live runs can be billed. Each case uses the same production output ceilings and
retry/validation rules; the corpus has a hard maximum of 50 cases. The CLI requires
an explicit provider and model for live execution and never falls back from replay
to a real service. No live run is part of CI or normal app startup.

Reports include per-case pass/fail checks, prompt/schema/context provenance,
latency (live only), reported tokens/cost when available, and unknown values when
not reported. They omit raw generated answers and prompts. Report schema and
grader versions plus the selected-input/check fingerprint make comparisons
repeatable. Use `--compare BASELINE.json` for a live baseline with identical cases
and graders; replay and live reports cannot be compared as model improvements.
Run several live trials before choosing defaults, and retain model identifiers
and provider pricing alongside the reports.

Deterministic checks cover factual anchors, counts, normalized dates, source
membership, forbidden inventions and uncertainty signals. They are deliberately
narrow: passing can still hide a misleading interpretation. Human review should
score factual support, ownership, supersession, useful abstention, and writing
quality on a blinded sample before changing defaults. Add failures as new
synthetic or explicitly de-identified cases; do not relax checks to accept a
candidate's incorrect output. A later judge-based grader must itself be calibrated
against human labels. Live model performance has not been measured by this change.

History polling returns status only while closed and at most 20 records when open.
Record JSON is encoded and sized at insertion or update, reused during pruning and
persistence, and written outside the state lock. A separate write gate serializes
durable financial mutations without blocking status reads or cancellation during fsync.
Cancel controls match both command and the displayed record owner. Cached generation
details load on demand beside the conversation brief. Context limits are 16 MB and
16,384 fields, above the existing bounded feature inputs. Test Connection remains
metered; Settings explains the price prerequisite when a daily limit is enabled.
