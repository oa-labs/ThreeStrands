#!/bin/sh
# First-boot setup for the Dovecot test server:
#   1. generate a self-signed cert for CN=127.0.0.1 (matches the primary
#      account's self-signed cert, so the client must use fingerprint pinning);
#   2. create the test user's Maildir and seed a few messages;
#   3. exec dovecot in the foreground (PID 1 via tini).
#
# POSIX sh (the 2.4.1 bookworm image has no bash). Idempotent: a restarted
# container reuses the existing cert and mailbox so the pinned fingerprint stays
# stable across restarts.
set -eu

SSL_DIR=/etc/dovecot/ssl
CERT="$SSL_DIR/dovecot.pem"
KEY="$SSL_DIR/dovecot.key"

mkdir -p "$SSL_DIR"
if [ ! -f "$CERT" ] || [ ! -f "$KEY" ]; then
  echo "[entrypoint] generating self-signed cert (CN=127.0.0.1)"
  openssl req -x509 -newkey rsa:2048 -nodes \
    -keyout "$KEY" -out "$CERT" -days 3650 \
    -subj "/O=ThreeStrands Test/CN=127.0.0.1" \
    -addext "subjectAltName=IP:127.0.0.1,DNS:localhost"
  chmod 600 "$KEY"
  # Print the SHA-256 fingerprint so tests / humans can pin it.
  echo -n "[entrypoint] cert SHA-256 fingerprint: "
  openssl x509 -in "$CERT" -noout -fingerprint -sha256 | sed 's/^.*=//'
fi

# Ensure the dovecot user owns the mail root.
mkdir -p /srv/mail
chown -R 5000:5000 /srv/mail "$SSL_DIR" 2>/dev/null || true

# Start dovecot in the background, wait for its auth socket, seed against the
# running server (doveadm save needs the master running in 2.4), then hand the
# foreground to dovecot by waiting on it. tini reaps everything on stop.
echo "[entrypoint] starting dovecot"
dovecot -F &
DOVECOT_PID=$!

# Wait up to 30s for the master to be ready (auth-userdb socket present).
i=0
while [ $i -lt 30 ]; do
  if doveadm who >/dev/null 2>&1; then break; fi
  i=$((i+1)); sleep 1
done

/usr/local/bin/seed.sh || echo "[entrypoint] seed.sh reported a non-fatal issue; continuing"

echo "[entrypoint] dovecot ready; running in foreground"
wait "$DOVECOT_PID"
