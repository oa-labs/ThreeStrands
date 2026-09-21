# Deploying Three Strands account sync to production

This runbook describes how to deploy the optional Three Strands account and
cross-device sync service. The desktop application remains fully usable without
this service. A release build only offers Three Strands account features when it
has a sync-service URL configured.

The production system has four parts:

1. The `threestrands-sync-server` OCI container.
2. A managed PostgreSQL database.
3. A public HTTPS origin, such as `https://sync.threestrands.app`.
4. A separate Google OAuth **Web application** client for product sign-in.

This service stores service-readable workflow data. It must not be described as
end-to-end encrypted. Review the complete data boundary in
[`account-sync.md`](account-sync.md) before enabling accounts.

## 1. Make the production decisions

Choose and record the following before provisioning infrastructure:

- The public sync hostname and deployment region.
- The managed PostgreSQL provider and backup/PITR retention period.
- How long deleted data may remain in provider backups. Show this finite period
  in the account-deletion confirmation and privacy documentation.
- Tombstone, record-revision, conflict, expired-auth-flow, and processed-operation
  retention periods. The initial schema retains these indefinitely, so add a
  reviewed scheduled cleanup job before storage growth becomes material. Never
  delete tombstones or revisions needed by an unresolved conflict.
- An incident owner and alerts for readiness failures, elevated 5xx/401/403
  rates, database saturation, sync latency, and abnormal conflict volume.

Use separate staging and production databases, hostnames, secrets, and Google
OAuth clients. Do not test production migrations against the only copy of user
data.

## 2. Configure Google product sign-in

In a Google Cloud project dedicated to Three Strands product authentication:

1. Configure the OAuth consent screen and the product name/support contacts.
2. Request only `openid`, `email`, and `profile`.
3. Create an OAuth client of type **Web application**.
4. Add exactly this authorized redirect URI, substituting the production host:

   ```text
   https://sync.threestrands.app/v1/auth/google/callback
   ```

5. Publish the consent screen when ready. While it remains in testing mode,
   explicitly add every beta tester allowed by Google.
6. Put the client ID and client secret in the production secret manager.

This credential is not the desktop Gmail/Calendar credential. The Web client
secret is supplied only to the sync service. Gmail and Calendar authorization
continue to use the app's existing Desktop OAuth client and per-device tokens.

## 3. Provision PostgreSQL

Use a supported managed PostgreSQL deployment; PostgreSQL 17 is used by the
local Compose environment. Configure:

- encryption at rest and TLS-required client connections;
- automated backups and point-in-time recovery;
- private networking or an ingress allowlist that admits only the service and
  migration job;
- high availability appropriate to the beta's recovery objective;
- monitoring for connections, CPU, storage, replication lag, and failed
  backups.

The first migration enables `pgcrypto`, so the migration identity must be
allowed to create that extension. Prefer separate identities:

- a migration role that can create and alter schema objects; and
- a runtime role with only the privileges required to read and modify the
  application tables and sequence.

The current server opens up to 20 database connections per replica. Size the
database or add a transaction-mode pooler so `replicas * 20`, migration jobs,
and administrative connections remain below the provider limit.

Construct a TLS-enforcing connection URL and store it as `DATABASE_URL`, for
example using the exact SSL parameters required by the selected provider. Never
put this URL in source control, image layers, or build logs.

## 4. Build and publish the service image

From the repository root, build the checked-in Dockerfile with an immutable
release tag:

```sh
docker build \
  -f services/sync-server/Dockerfile \
  -t registry.example.com/threestrands/sync-server:0.18.1 .
docker push registry.example.com/threestrands/sync-server:0.18.1
```

Record the resulting image digest and deploy by digest in production. Run the
normal dependency and container vulnerability scans in the image pipeline.

The container runs as the unprivileged numeric user `65532`, listens on port
`8080`, writes structured logs to stdout, and requires no persistent volume.

## 5. Run database migrations

Back up the database and test restoration before the first production
migration. Apply every file in `services/sync-server/migrations` in order from a
one-off deployment job using the migration role. With SQLx CLI 0.8 installed,
the command from the repository root is:

```sh
DATABASE_URL='postgresql://…' \
  sqlx migrate run --source services/sync-server/migrations
```

Then verify the migration history and run a simple connection check. Treat a
failed migration as a stopped deployment; do not start the new application
revision or attempt an automatic down migration.

The current server binary also calls the embedded SQLx migrator at startup as a
safety net. The production pipeline should still run migrations as a distinct,
successful deployment step before increasing application replicas. Until that
startup call is removed, the runtime role must retain the schema access SQLx
needs to inspect and run embedded migrations; strict separation of migration
and runtime privileges is therefore a pre-launch hardening item.

## 6. Deploy the service

Supply these variables from the platform's runtime secret/configuration system:

| Variable | Secret | Production value |
| --- | --- | --- |
| `DATABASE_URL` | Yes | TLS-enforcing PostgreSQL connection URL |
| `GOOGLE_OIDC_CLIENT_ID` | No | Product-sign-in Web OAuth client ID |
| `GOOGLE_OIDC_CLIENT_SECRET` | Yes | Product-sign-in Web OAuth client secret |
| `PUBLIC_BASE_URL` | No | Exact external origin, with no path or trailing slash |
| `BIND_ADDRESS` | No | Usually `0.0.0.0:8080` |
| `RUST_LOG` | No | For example `threestrands_sync_server=info,tower_http=info` |

`PUBLIC_BASE_URL` must be the HTTPS origin users actually reach because it is
used to construct Google's callback URL. Do not expose the container directly.
Put it behind a load balancer or reverse proxy that:

- terminates TLS with a valid, automatically renewed certificate;
- redirects HTTP to HTTPS;
- preserves the original path and query string;
- sets conservative connection/request timeouts;
- enforces a request-size limit no larger than the server's 2 MiB limit;
- rate-limits authentication and token endpoints; and
- does not log query strings, authorization headers, request/response bodies,
  OAuth codes, or user email-derived payloads.

Configure probes as follows:

- liveness: `GET /health/live` (process is serving requests);
- readiness: `GET /health/ready` (database query succeeds).

Start with one replica, verify migrations and sign-in, then scale gradually.
Use rolling updates with readiness gating and graceful termination. The service
is stateless, so replicas require no session affinity.

## 7. Build and distribute the desktop application

Set the production service origin while building every signed desktop release:

```sh
THREESTRANDS_SYNC_URL='https://sync.threestrands.app' \
THREESTRANDS_GOOGLE_CLIENT_ID='desktop-mail-calendar-client-id' \
THREESTRANDS_GOOGLE_CLIENT_SECRET='desktop-public-client-value' \
pnpm tauri build
```

`THREESTRANDS_SYNC_URL` is read at runtime first and falls back to the value
embedded at compile time. Embedding it is the reliable choice for installed
production builds. It must use HTTPS. Omitting it leaves the application in its
existing local-only mode and causes no data upload.

The two `THREESTRANDS_GOOGLE_*` variables above are the existing desktop
Gmail/Calendar credentials, not the server's `GOOGLE_OIDC_*` Web credential.
Follow the platform signing, notarization, and release process after the normal
test suite succeeds.

## 8. Grant beta access

A user may create an account before receiving sync access. Have the user sign in
once so their verified Google identity exists, then run the admin CLI from a
secured operator environment with database access:

```sh
DATABASE_URL='postgresql://…' \
  cargo run --release \
  --manifest-path services/sync-server/Cargo.toml \
  --bin admin -- grant-sync person@example.com
```

To remove beta access without deleting local or cloud data:

```sh
DATABASE_URL='postgresql://…' \
  cargo run --release \
  --manifest-path services/sync-server/Cargo.toml \
  --bin admin -- revoke-sync person@example.com
```

Restrict this command and database access to authorized operators. Email is
only an operator lookup key here; the service's stable internal user ID and
Google issuer/subject remain the account identity.

## 9. Production smoke test

Perform these checks first in staging and then with a controlled production
beta account:

1. Confirm HTTPS and database readiness:

   ```sh
   curl --fail https://sync.threestrands.app/health/live
   curl --fail https://sync.threestrands.app/health/ready
   ```

2. Confirm an unconfigured/signed-out desktop installation makes no requests to
   the service and retains all local behavior.
3. Sign in with Google and confirm the callback returns to the desktop app.
4. Confirm an account without the beta grant sees sync as unavailable.
5. Grant `sync`, review the first-sync disclosure, and enroll the test device.
6. On two clean device profiles, create and update tasks, snippets, Split
   Inboxes, account metadata, calendar selections, and portable preferences.
   Confirm convergence within 30 seconds.
7. Take one device offline, edit on both devices, reconnect it, and confirm both
   disjoint auto-merge and same-field conflict resolution.
8. Revoke a device and verify its next authenticated request fails. Sign out on
   another device and verify its local data remains.
9. Inspect database rows and logs to confirm no mail body, attachment, provider
   token, AI key, or product refresh token is present.
10. Delete the controlled cloud account and confirm sessions and active records
    are removed, then verify the documented backup-expiration behavior.

## 10. Rollout, rollback, and operations

Roll out to internal accounts before widening the beta entitlement. Watch
readiness, HTTP outcomes, database usage, authentication failures, sync lag,
and conflict counts during each expansion.

For an application rollback, redeploy the previous immutable image. Do not
reverse a database migration unless a separately tested down-migration exists;
prefer forward-compatible schema changes and a forward fix. If a client release
must be stopped, remove it from distribution or ship a corrected signed build.
Revoking the `sync` entitlement safely stops cloud traffic for selected users
while preserving their local data.

For an outage:

- leave the desktop client operating locally; its durable outbox retries later;
- remove unhealthy replicas from service using readiness, without deleting the
  database;
- restore PostgreSQL through the provider's PITR process when required;
- verify acknowledged sequence/cursor behavior before reopening traffic; and
- communicate that local mail, tasks, settings, and BYO-key AI continue to work.

Rotate database and Google client secrets through the secret manager and restart
replicas gradually. Existing Three Strands sessions do not contain either
secret; rotating the Google client secret affects new sign-ins, while rotating
database credentials affects service connectivity.

## Pre-launch limitations to track

The current MVP does not expose a Prometheus/OpenTelemetry metrics endpoint,
package the admin CLI in the runtime image, include a built-in retention worker,
or fully separate migration and runtime database privileges. Use platform
HTTP/database metrics initially, run the admin binary from a controlled source
checkout or dedicated operator image, and implement the retention and database
role hardening selected above before a broad production launch.
