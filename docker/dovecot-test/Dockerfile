# Dovecot test server for the IMAP provider integration tests.
#
# It deliberately reproduces the LOWEST capability tier — the profile the
# primary account (Proton Mail Bridge) exposes, captured 2026-10-08 in
# docs/imap-design.md "The primary account":
#
#   after login:  AUTH=PLAIN ID IDLE IMAP4rev1 MOVE UIDPLUS UNSELECT
#   STARTTLS on a non-standard port (1143), self-signed cert, CN=127.0.0.1
#   PERMANENTFLAGS has no \*  (custom keywords CANNOT be stored)
#   no CONDSTORE / QRESYNC / SPECIAL-USE advertised, but LIST returns
#   \Sent \Archive \Junk \Trash \Drafts \All \Flagged attributes
#
# This is the "neither" sync path, which the design makes the daily path and
# therefore the most-tested one. Higher tiers (CONDSTORE/QRESYNC) get their own
# profiles later.
#
# Built from ubuntu:24.04 rather than the official dovecot image on purpose:
#   - ubuntu:24.04 is multi-arch, so it runs NATIVELY on Apple Silicon (no
#     Rosetta — the amd64-only dovecot/dovecot:2.3.21 crashes under emulation)
#     and on CI's x86-64;
#   - it ships a full userland (bash, chmod, openssl, …) so the cert-gen and
#     seed entrypoint can be ordinary scripts (the official 2.4 image is nearly
#     distroless and cannot host them);
#   - apt installs Dovecot 2.3.21, which matches the 2.3-era config profile the
#     primary-account survey was taken against.
# This mirrors the .devcontainer convention of building on an Ubuntu base.
#
# The base is pinned by digest and the Dovecot package version is pinned below,
# matching the Node pinning convention in .devcontainer/Dockerfile. Update both
# intentionally, never float them.
FROM ubuntu:24.04@sha256:534baea6a22c03a63003dbc8dbe78fe34bc0d7e595d9a9dc9834884ff530eb55

ENV DEBIAN_FRONTEND=noninteractive

# dovecot-imapd pulls dovecot-core. openssl generates the self-signed cert at
# first boot; tini is PID 1 for clean signal handling (fast `docker stop`).
# python3 (stdlib only) backs the tiny SMTP sink the Phase 4 submission live
# tests send to — no pip, no aiosmtpd, so the image build stays as
# offline-friendly as the apt step already is.
# Pin the exact Dovecot version so server behavior is reproducible; refresh it
# deliberately when the base digest moves.
ARG DOVECOT_VERSION=1:2.3.21+dfsg1-2ubuntu6.5
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        "dovecot-core=${DOVECOT_VERSION}" \
        "dovecot-imapd=${DOVECOT_VERSION}" \
        openssl \
        python3 \
        tini \
    && rm -rf /var/lib/apt/lists/*

COPY dovecot.conf /etc/dovecot/dovecot.conf
COPY dovecot-acl /etc/dovecot/dovecot-acl
COPY entrypoint.sh /usr/local/bin/entrypoint.sh
COPY seed.sh /usr/local/bin/seed.sh
COPY smtp-sink.py /usr/local/bin/smtp-sink.py
RUN chmod +x /usr/local/bin/entrypoint.sh /usr/local/bin/seed.sh /usr/local/bin/smtp-sink.py

# IMAP STARTTLS on the non-standard port the primary account uses (1143), so
# the test exercises "STARTTLS on a port we upgrade ourselves" rather than
# implicit TLS on 993. The Phase 4 SMTP submission live tests reach the tiny
# sink on 1465 (implicit TLS) and 1587 (STARTTLS), sharing the IMAP cert.
EXPOSE 1143 1465 1587

ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/entrypoint.sh"]
