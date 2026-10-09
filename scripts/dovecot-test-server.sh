#!/usr/bin/env bash
# Control the Dovecot test server used by the IMAP provider integration tests.
#
#   scripts/dovecot-test-server.sh up        build (if needed) + run, wait healthy
#   scripts/dovecot-test-server.sh down      stop + remove the container
#   scripts/dovecot-test-server.sh status    report running state + mapped port
#   scripts/dovecot-test-server.sh logs      tail container logs
#   scripts/dovecot-test-server.sh fingerprint   print the cert SHA-256 pin
#
# Uses the plain `docker` CLI (Rancher Desktop's is at ~/.rd/bin). The compose
# v2 plugin is NOT required. Linux CI already has docker on PATH, so there it
# just works.
#
# Env:
#   DOVECOT_TEST_PORT   host port to map 1143 to (default 11143)
#   DOCKER              docker binary (default: first of ~/.rd/bin/docker, docker)
set -euo pipefail

# Prefer Rancher Desktop's docker, fall back to PATH.
if [[ -z "${DOCKER:-}" ]]; then
  if [[ -x "$HOME/.rd/bin/docker" ]]; then DOCKER="$HOME/.rd/bin/docker"; else DOCKER="docker"; fi
fi

IMAGE=threestrands/dovecot-test:latest
NAME=threestrands-dovecot-test
PORT="${DOVECOT_TEST_PORT:-11143}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CTX="$HERE/docker/dovecot-test"

build() {
  echo "[dovecot-test] building $IMAGE"
  "$DOCKER" build -t "$IMAGE" "$CTX"
}

up() {
  if "$DOCKER" ps --format '{{.Names}}' | grep -qx "$NAME"; then
    echo "[dovecot-test] already running"; status; return 0
  fi
  "$DOCKER" image inspect "$IMAGE" >/dev/null 2>&1 || build
  echo "[dovecot-test] starting $NAME on 127.0.0.1:$PORT -> 1143/STARTTLS"
  "$DOCKER" run -d --rm \
    --name "$NAME" \
    -p "127.0.0.1:$PORT:1143" \
    "$IMAGE" >/dev/null
  # Wait until dovecot answers (max ~30s). `doveadm who` succeeds once the
  # server is up; nc is not shipped in the image.
  for i in $(seq 1 30); do
    if "$DOCKER" exec "$NAME" doveadm who >/dev/null 2>&1; then
      echo "[dovecot-test] healthy"; status; return 0
    fi
    sleep 1
  done
  echo "[dovecot-test] did NOT become healthy in 30s; recent logs:" >&2
  "$DOCKER" logs --tail 40 "$NAME" >&2 || true
  return 1
}

down() {
  if "$DOCKER" ps -a --format '{{.Names}}' | grep -qx "$NAME"; then
    "$DOCKER" rm -f "$NAME" >/dev/null && echo "[dovecot-test] stopped"
  else
    echo "[dovecot-test] not running"
  fi
}

status() {
  if "$DOCKER" ps --format '{{.Names}} {{.Ports}}' | grep -q "$NAME"; then
    echo "[dovecot-test] RUNNING  host 127.0.0.1:$PORT"
  else
    echo "[dovecot-test] stopped"
  fi
}

logs() { "$DOCKER" logs -f "$NAME"; }

fingerprint() {
  "$DOCKER" exec "$NAME" sh -c \
    'openssl x509 -in /etc/dovecot/ssl/dovecot.pem -noout -fingerprint -sha256' \
    | sed 's/^.*=//'
}

case "${1:-}" in
  up) up ;;
  down) down ;;
  status) status ;;
  logs) logs ;;
  build) build ;;
  fingerprint) fingerprint ;;
  *) echo "usage: $0 {up|down|status|logs|build|fingerprint}" >&2; exit 2 ;;
esac
