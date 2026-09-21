# Three Strands account and cross-device sync

Three Strands remains a local-first application. Signing in is optional; a
signed-out installation does not contact the Three Strands sync service and
continues to use direct mail/calendar connections and user-provided AI keys.

## Data boundary

An entitled account synchronizes tasks, snippets, Split Inboxes, mail-account
display metadata, calendar-account metadata and selections, retention, and the
portable preference allowlist. Task subject snapshots and evidence, snippet
text, rules, and custom AI endpoint configuration are readable by the service.
Transport uses HTTPS and production PostgreSQL storage must be encrypted at
rest. This is not end-to-end encryption.

The sync protocol cannot carry cached mail or bodies, attachments, drafts,
queued mail, provider mutations, OAuth tokens, AI API keys, crash reports,
telemetry consent, pinned contacts, window/layout state, scroll positions, or
navigation/usage history. Product refresh tokens use the separate
`app.threestrands.account` OS-keychain service and never enter SQLite.

Google product sign-in requests only `openid email profile`. Gmail and Calendar
authorization remain distinct per-device grants; the product identity may be
different from every connected provider account.

## Local development

Start PostgreSQL and the portable service container:

```sh
GOOGLE_OIDC_CLIENT_ID=... \
GOOGLE_OIDC_CLIENT_SECRET=... \
docker compose -f docker-compose.sync.yml up --build
```

The Google Web OAuth client must allow
`http://localhost:8080/v1/auth/google/callback`. Start the desktop application
with `THREESTRANDS_SYNC_URL=http://localhost:8080`; omitting that variable
removes account-service availability without affecting local features.

Grant or revoke the temporary beta entitlement with the service admin binary:

```sh
DATABASE_URL=postgres://... cargo run \
  --manifest-path services/sync-server/Cargo.toml \
  --bin admin -- grant-sync person@example.com
```

Production deploys the service as a stateless OCI container behind TLS and
runs SQL migrations before application rollout. Configure managed PostgreSQL
encryption, backups, point-in-time recovery, and a documented backup expiration
window for deleted accounts. Health probes use `/health/live` and
`/health/ready`. See the step-by-step
[`production deployment runbook`](sync-production-deployment.md) for Google
configuration, secrets, migrations, client builds, rollout, and recovery.

## Synchronization behavior

Local changes apply immediately and enter a durable outbox. The client pushes
after mutations and on a 15-second active loop, with bounded exponential
backoff, plus startup/foreground catch-up. Server sequence numbers—not device
clocks—order changes. Operations are idempotent.

Stale patches merge when their changed fields do not overlap intervening
revisions. Same-field and edit/delete races preserve both sides as a conflict;
Settings lets the user select the cloud or device value for each overlapping
field. Signing out clears only cloud session/sync metadata. Local workflow data
remains available.
