# Dovecot test server

A containerised IMAP server for the IMAP provider integration tests (Phase 2 of
`docs/imap-design.md`). It reproduces the **lowest capability tier** — the
profile the primary account (Proton Mail Bridge) exposes — so the tests
exercise the "neither" (no `CONDSTORE`, no `QRESYNC`) sync path, which the
design makes the daily path and therefore the most-tested one.

## Why this shape

- **Built from `ubuntu:24.04`**, not the official `dovecot/dovecot` image:
  `ubuntu:24.04` is multi-arch so it runs **natively on Apple Silicon** (the
  amd64-only `dovecot/dovecot:2.3.21` crashes under Rosetta emulation; the
  native `2.4.1` image is nearly distroless and can't host the setup scripts).
  Ubuntu's apt ships Dovecot 2.3.21, matching the config era of the
  primary-account survey. Mirrors the `.devcontainer` Ubuntu-base convention.
- Base digest and the Dovecot apt version are **pinned** in the `Dockerfile`;
  update them intentionally.

## Usage

```sh
scripts/dovecot-test-server.sh up          # build if needed, run, wait healthy
scripts/dovecot-test-server.sh status
scripts/dovecot-test-server.sh fingerprint # the cert SHA-256 the client pins
scripts/dovecot-test-server.sh logs
scripts/dovecot-test-server.sh down
```

Host port defaults to `127.0.0.1:11143` (override with `DOVECOT_TEST_PORT`),
mapped to the container's `1143` (STARTTLS). Uses the plain `docker` CLI
(Rancher Desktop's at `~/.rd/bin`); the compose v2 plugin is not required.

Test account: `test@threestrands.test` / `testpassword`.

## Verified fidelity (over a real STARTTLS connection on 1143)

- Advertised capability: `IMAP4rev1 … IDLE MOVE UIDPLUS UNSELECT`, with
  `CONDSTORE`, `QRESYNC`, `SPECIAL-USE`, `LIST-EXTENDED`, `ESEARCH` **absent**.
- STARTTLS offered on the non-standard port 1143; no implicit-TLS 993 listener.
- Self-signed cert, `CN=127.0.0.1`, generated at first boot with a stable
  fingerprint across restarts — the client must fingerprint-pin it.
- System mailboxes present with special-use attributes readable from a plain
  `LIST`: INBOX, Sent, Drafts, Trash, Junk, Archive, "All Mail" (`\All`),
  Starred (`\Flagged`).
- Seeded mail: 2 INBOX messages (one shares a Message-ID with a Sent message to
  exercise the "one message, two locations" identity rule) + 1 Archive message.

## Open verification item

- **`PERMANENTFLAGS` must omit `\*`** (so custom keywords can't be stored, as on
  the primary account). The config targets this, but it is best asserted by the
  Phase-2 Slice-1 Rust integration test that parses a real `SELECT` response,
  rather than by shell scripting an IMAP transcript. Confirm it there.
