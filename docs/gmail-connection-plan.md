# Gmail connection implementation plan

**Status** · Phase 1 largely implemented (`0.211.0`); Phase 2 proposed.

Start with Gmail over IMAP/SMTP using an app password, close the gaps that
affect everyday mail, and give existing Gmail API users a way to switch without
losing their work. In Phase 2, let users connect Google Workspace accounts using
the OAuth Desktop client JSON supplied by each organization so an installation
can use different clients for different accounts.

Google's reviewer confirmed that the public ThreeStrands client requires CASA
for `gmail.modify`, even when restricted data stays on the desktop and optional
cloud features are disabled. Gmail IMAP/SMTP with an app password uses no OAuth
scope. App passwords require 2-Step Verification and may be unavailable for
some accounts, including accounts under Advanced Protection or organization
policy. Gmail IMAP/SMTP with OAuth uses the restricted `https://mail.google.com/`
scope. [CASA requirements](https://support.google.com/cloud/answer/13465431),
[App-password eligibility](https://support.google.com/accounts/answer/185833),
[Gmail IMAP OAuth scopes](https://developers.google.com/workspace/gmail/imap/xoauth2-protocol).

## Phase 1 Gmail over IMAP and SMTP

Most of Phase 1 has shipped across ThreeStrands' IMAP/SMTP provider and
[Gmail IMAP extensions](imap-gmail-extensions.md):

- **Implemented:** guided **Gmail with an app password** setup (`ImapAccountSetup.tsx`),
  reading, attachment retrieval, All Mail acquisition, `X-GM-RAW` / server search,
  server draft sync, `X-GM-MSGID` / `X-GM-THRID` identity mappings and legacy
  thread repair, native Gmail label create/apply/remove/rename/delete (including
  hierarchies and cross-device authority), label-based archive/undo/snooze return,
  **Switch to Gmail via IMAP…** per-device conversion (`SwitchGmailToImap.tsx`,
  preserving tasks, goals, account preferences, sender rules, signatures, and
  reviewed Split Inboxes while keeping Calendar independent), Sent-copy
  reconciliation for uncertain SMTP sends, and manual sending addresses with
  default From, Reply-To, and per-address signatures.
- **Remaining in Phase 1:** tracked in the [IMAP parity plan](imap-parity-plan.md)
  (Stage 3 resumable, draft/outbox/receipt/localStorage-preserving conversion;
  Stage 4 scale benchmarks and `CONDSTORE`/`QRESYNC` reconciliation), plus
  developer live-account release checks with a real Gmail app-password account.

## Phase 2 Organization supplied OAuth clients

An organization enables the Gmail API in its own Google Cloud project, sets
the audience to **Internal**, creates a **Desktop app** OAuth client, and
distributes Google's downloaded JSON to its users. Each user signs in to Google
and grants ThreeStrands access to their account. Workspace administrators can
apply their existing app access controls. Google's internal-use exception is
tied to the organization's project and audience. The public ThreeStrands OAuth
client follows its own verification requirements.
[Internal-use requirements](https://developers.google.com/identity/protocols/oauth2/production-readiness/restricted-scope-verification#internal-use-only),
[Workspace user data policy](https://developers.google.com/workspace/workspace-api-user-data-developer-policy),
[Google Desktop OAuth](https://developers.google.com/identity/protocols/oauth2/native-app).

### Import and sign in

1. Add **Connect Gmail with OAuth file…** under **Settings → Mail Accounts**.
   A native file picker opens Google's Desktop client JSON. Read the `installed`
   client's `client_id` and `client_secret` in the native backend and show an
   editable display name for the configuration. Validate the file before opening
   a browser. Use the app's existing Google endpoints, scopes, loopback redirect,
   and PKCE flow. Treat endpoints, redirect URLs, and project labels in the JSON
   as untrusted input. Report malformed files and web-client JSON in setup.
2. Complete the browser flow and bind the client to the Gmail identity returned
   by Google. Reconnect and client replacement must match the existing account.
   If a user chooses the wrong Google account, cancels, or is blocked by org
   policy, keep any existing connection intact and explain the failure. Save the
   association only on successful sign-in.
3. Reuse saved clients through an explicit selection when connecting another
   account. Importing Organization B's file must allow Accounts A and B to keep
   syncing under their own clients. Show which client each account uses and make
   it available through **Reconnect** and **Change OAuth client**. IT can provide
   the same JSON to multiple employees. Each employee completes their own sign-in.

### Replace the installation-wide OAuth client with connection bindings

- Extend the current `AuthConfig`/`OAuthApp` flow with a local client catalog
  keyed by client ID and a client binding for each Google connection. Keep the
  existing environment or bundled client as the default for accounts that use
  it. Migrate existing tokens when their issuing client is known. If it is
  uncertain, reconnect the account while preserving local data. Resolve an
  explicit organization binding at startup, authorization, refresh, and
  removal. If its configuration is missing, request repair for that connection
  before sending an OAuth request. [Current configuration](../src-tauri/src/auth.rs),
  [Developer credentials](../DEVELOPERS.md#gmail-oauth-credentials).
- Store the imported client secret and user tokens in the OS keychain. Scope
  token entries and caches by Google identity, client ID, and service. Store
  only non-secret metadata and connection bindings in the local database.
  Keep imported credentials and their bindings device-local. A second device
  can import the same JSON and authorize independently. Keep raw credentials,
  tokens, and JSON contents out of logs, exports, and encrypted replication.
- Keep Gmail and Calendar grants independent. Adding or changing a mail client
  must leave the Calendar binding in place. If Calendar offers the saved client
  as a choice, it must request its own scopes and obtain its own consent. Review
  revocation by grant and project, including mail and calendar bindings on
  enrolled devices. Preserve a sibling connection when grant ownership is
  uncertain. Treat an imported project label as display metadata.
- Stage client replacement until the same Gmail identity authorizes with the
  new credentials. Keep the account and Gmail provider IDs, cached messages,
  draft links, and queued sends in place. Invalidate the old token cache on
  commit and keep rollback possible before commit. The existing environment
  override remains available for a one-client pilot while the importer is built.

**Phase 2 release checks**

- Use synthetic Desktop client fixtures and a stub OAuth server to check two
  clients, separate token refresh after restart, default-client migration,
  mismatched signed-in identity, cancellation, revoked access, and credential
  replacement. Verify that bad input cannot change the endpoints or leak file
  contents into diagnostics and that disconnecting one connection leaves
  unrelated clients and sibling grants usable.
- Smoke-test with two real Internal Workspace projects before release. Connect
  both from their respective files in one installation, restart, exercise mail
  read, search, labels, drafts, and send on both, and reconnect one while the
  other continues syncing. When a public default is configured, include it in
  mixed-account coverage. Check an outside account and a blocked client for
  clear setup errors. Use the developer-owned live-account release checks for
  the real sign-in and app behavior.
