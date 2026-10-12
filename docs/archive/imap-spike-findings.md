# Phase 2 Slice 0 — `async-imap` spike findings

Status: **spike complete — RECOMMENDATION: proceed on `async-imap` 0.12.**

This records the outcome of the Slice 0 gate from
[`imap-design.md`](./imap-design.md) ("Spike before committing"). The probe is
a throwaway cargo example, `src-tauri/examples/imap_spike.rs` (dev-dependencies
only — see [Where the code lives](#where-the-code-lives)). It was run against
the local Dovecot test container restricted to the primary account's lowest
tier (`IMAP4rev1 SASL-IR LOGIN-REFERRALS ID ENABLE IDLE LITERAL+ MOVE UIDPLUS
UNSELECT`, STARTTLS on `127.0.0.1:11143`, self-signed `CN=127.0.0.1`).

Reproduce:

```sh
scripts/dovecot-test-server.sh up
FP="$(scripts/dovecot-test-server.sh fingerprint)"      # read the pin at runtime
DOVECOT_TEST_SCRIPT=scripts/dovecot-test-server.sh \
  cargo run --manifest-path src-tauri/Cargo.toml --example imap_spike -- --fingerprint "$FP"
```

The fingerprint is read from the running container at runtime and never
hardcoded; the cert regenerates whenever the container is recreated.

## Result table

| # | Check | Verdict | Evidence |
|---|-------|---------|----------|
| 1 | STARTTLS over a stream we upgrade ourselves + SHA-256 fingerprint pinning (accept real, reject wrong) | **PASS** | Wrong pin (`AA:AA:…`) → handshake rejected: `certificate fingerprint mismatch`. Real pin → STARTTLS upgrade + pinned TLS OK, `LOGIN` OK, `SELECT INBOX` OK. |
| 2 | IDLE enter/done + clean reconnect after a dropped connection | **PASS** | `IDLE entered` → `IDLE DONE clean`. Dropped the connection with `dovecot-test-server.sh down && up`; re-read the regenerated pin; reconnect succeeded on attempt 1. |
| 3 | `UID MOVE` + `APPEND` via `run_command` surface `COPYUID`/`APPENDUID` | **PASS** | `APPEND SpikeSrc` → `AppendUid { validity, uids: [Uid(1)] }`. `UID MOVE 1 SpikeDst` → `CopyUid { validity, from: [Uid(1)], to: [Uid(1)] }`. Both read off the tagged `Done` outcome's `code`. |
| 4 | `UID STORE` of a keyword's result is detectable and consistent with `PERMANENTFLAGS` | **PASS** (with a harness finding) | `PERMANENTFLAGS = [Answered, Flagged, Deleted, Seen, Draft, MayCreate]`. `MayCreate` = `\*`, so the server admits arbitrary keywords; the stored `$SpikeArbitraryKeyword` persisted and a re-fetch confirmed it. Detection via re-fetch is reliable. |
| 5 | No plaintext `LOGIN` before STARTTLS (client policy) | **PASS** | The client only ever calls `login()` on the post-STARTTLS TLS `Client`; `starttls_connect()` has no plaintext-credential path. The server *does* advertise plaintext `AUTH=PLAIN` (so the capability is visible) — the refusal is the client's, by construction. |

All five checks pass. See the per-check notes below for the two that carry a
finding.

## async-imap gaps hit, and the workaround proven

**Gap 1 — `uid_mv`/`append` discard `COPYUID`/`APPENDUID` (confirmed, worked
around).** The typed `Session::uid_mv` and `Session::append` return `()`, so the
UIDPLUS response codes are thrown away, exactly as the design doc warned. The
workaround is proven: send the command through `Session::run_command`, which
returns the `RequestId`, then loop `read_response()` and match the tagged
`imap_proto::Response::Done { tag, status, outcome }` whose `outcome.code` is
`Some(ResponseCode::CopyUid(..))` / `Some(ResponseCode::AppendUid(..))`.
`imap-proto` 0.17 (re-exported as `async_imap::imap_proto`) parses both. This is
clean and does not fight the library — `run_command` is a first-class public
method, and `imap-proto` is a public re-export. **No fallback to Message-ID
matching is needed for UIDPLUS servers.**

One ergonomic note: `ResponseCode`/`Response` borrow from the `ResponseData`
read buffer, so the captured code must be copied into an owned type before the
next `read_response()`. Trivial (the spike's `OwnedCode` does this); the real
provider will map it straight into its own domain type.

**Gap 2 — `QRESYNC`/`VANISHED` not wrapped.** Not exercised (the primary
account doesn't offer them, and the "neither" sync path ships first). No change
to the plan: raw `run_command` is the same escape hatch Gap 1 uses, so when
`CONDSTORE`/`QRESYNC` are added later they can go through it without blocking
v1. The spike demonstrates that escape hatch works end to end.

## Findings that affect the test harness / Phase 2 plan

**The Dovecot test container advertises `\*` in `PERMANENTFLAGS`; the primary
account (Proton Bridge) does not.** This answers the harness's open item from
the Dovecot-server work: the container's `PERMANENTFLAGS` is
`[Answered, Flagged, Deleted, Seen, Draft, MayCreate]` — `MayCreate` is
async-imap's typed name for `\*` — so it **does not** match Bridge's
keyword behaviour, which omits `\*`. Consequences:

- The container is a faithful stand-in for the primary account on *capabilities*
  (`IDLE MOVE UIDPLUS UNSELECT`, no `CONDSTORE`/`QRESYNC`/`SPECIAL-USE`) and on
  STARTTLS + self-signed cert, but **not** on the keyword dimension. The
  label-strategy decision in `imap-design.md` (primary account → **label
  folders**, because Bridge omits `\*`) stands and is unaffected.
- To regression-test the *label-folder* path (the one the primary account
  actually uses), the harness needs a Dovecot config whose `PERMANENTFLAGS`
  omits `\*`. The current container is instead a convenient fixture for the
  *keyword* path a `\*`-capable server would take. **Action for a later slice:**
  either add a second Dovecot variant with `\*` suppressed, or set the mailbox's
  keyword policy so arbitrary keywords are refused, to cover the primary
  account's exact behaviour. Not a blocker for Slice 0.
- The spike's check 4 was reframed accordingly: the capability Phase 2 needs is
  that the client can **detect** whether a stored keyword persisted (STORE then
  re-fetch `FLAGS`) and branch to label-folders when it did not. That detection
  works regardless of the server's `\*` policy; the check passes when the
  observed persistence is consistent with the advertised `PERMANENTFLAGS`.

**Check 4 leaves a `$SpikeArbitraryKeyword` keyword and a couple of small
messages in `INBOX`/scratch mailboxes.** Harmless on a throwaway container
(it is torn down with `down`), but worth knowing if the same container is reused
across manual runs. The move/append scratch mailboxes (`SpikeSrc`/`SpikeDst`)
are deleted at the end of check 3.

**Cert regeneration on `docker restart`/recreate is real and the pin must be
re-read.** Check 2's simulated drop recreates the container, which regenerates
the self-signed cert — a brand-new SHA-256. The spike proves the pinning
verifier correctly *rejects* the old pin against the new cert (that is the pin
working, not a bug) and that re-reading the live fingerprint restores trust.
**Plan implication:** account setup pins the cert the user is shown at setup;
if the server legitimately rotates its cert, the client must surface a
"certificate changed — re-confirm" prompt rather than silently trusting it.
That is already the intended pinning UX; the spike confirms the mechanics.

## Where the code lives (dev-deps rationale)

The probe is `src-tauri/examples/imap_spike.rs` — a **cargo example**, not part
of the lib/bin. Its extra crates are under `[dev-dependencies]` in
`src-tauri/Cargo.toml`:

```toml
async-imap   = { version = "0.12", default-features = false, features = ["runtime-tokio"] }
tokio-rustls = { version = "0.26", default-features = false, features = ["ring", "tls12"] }
rustls-pki-types = "1"
futures = "0.3"
```

Why this placement:

- **Examples compile against dev-dependencies**, so the probe builds and runs,
  but a cargo example is **never linked into the shipped lib/bin**. The default
  `cargo build` / release build does not touch it.
- **Dev-dependencies do not ship.** Verified:
  `cargo tree --manifest-path src-tauri/Cargo.toml --edges normal -i async-imap`
  prints *"nothing to print"* — `async-imap` is absent from the non-dev
  dependency graph, so the app binary does not pull it. (It appears only under
  `--edges dev`.)
- The feature flags match the design doc's recommended stack: `async-imap` with
  `runtime-tokio` (its default runtime is async-std, so the flag matters);
  `tokio-rustls` on the `ring` provider, matching the app's existing
  `rustls`/`reqwest`/updater TLS backend; `rustls` 0.23 is already a shipped
  dependency.
- The custom `ServerCertVerifier` pins the leaf cert's SHA-256 and nothing is
  ever disabled — there is no `dangerous_accept_invalid_certs` and no verifier
  that blanket-accepts. Signature verification still runs through rustls' real
  ring-backed routines; only chain/root trust is replaced by the pin.
- **No app version bump** (spike + docs only; the shipped crate's code is
  unchanged).

When Slice 1 begins, these move from `[dev-dependencies]` to real
`[dependencies]` behind the `ImapSession` trait, and this throwaway example is
deleted.

## Recommendation

**Proceed on `async-imap` 0.12.** Every capability the primary account needs is
reachable:

- STARTTLS on a non-standard port over a stream we own, with a SHA-256
  fingerprint-pinning `ServerCertVerifier` that accepts the real cert and
  rejects a wrong one — the exact pinning design from the doc.
- `IDLE` enter/done and clean reconnect after a drop.
- The one real gap (`COPYUID`/`APPENDUID` discarded by `uid_mv`/`append`) has a
  clean, first-class workaround via `run_command` + the re-exported `imap-proto`
  response codes. That same `run_command` path also covers the future
  `QRESYNC`/`VANISHED` work without a library swap.
- Keyword-store results are detectable via re-fetch, which is what the
  label-folder-vs-keyword branch needs.

No reason to fall back to `imap-next` (which would mean hand-writing the command
flow). The `ImapSession` trait still isolates the choice, so the fallback stays
cheap if a later server surprises us — but nothing in this spike points that
way. The only follow-up is a harness one: add a Dovecot variant whose
`PERMANENTFLAGS` omits `\*` to regression-test the primary account's
label-folder path, since the current container admits `\*`.
