# Gmail connection implementation plan

**Status** · Proposed, 2026-11-21

Start with Gmail over IMAP/SMTP using an app password. Close the gaps that
affect everyday mail and give existing Gmail API users a way to switch without
losing their work. In Phase 2, let users connect Google Workspace accounts using
the OAuth Desktop client JSON supplied by each organization. An installation
can then use different clients for different accounts.

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

ThreeStrands already supports reading, attachment retrieval, sending, draft
sync through the account's Drafts mailbox, routine triage, All Mail, and
archive search. With Gmail's advertised extensions it can create, apply, and
remove native labels, and archive a message while keeping its other labels.
The existing [IMAP parity plan](imap-parity-plan.md) holds the detailed protocol
work and the backlog for ordinary IMAP servers.

### App-password setup

- Add a guided **Gmail with an app password** route through the existing IMAP
  setup. Prefill Gmail's IMAP and SMTP settings, link to Google's app-password
  instructions, and make the required credential clear. Verify both endpoints
  before activating the account and store the credential in the OS keychain.
- Use advertised capabilities and special-use mailboxes for behavior after
  sign-in. Verify Inbox, All Mail, Sent, Drafts, Trash, and Spam mappings. Keep
  manual setup and generic provider behavior available through the same IMAP
  implementation. Report authentication or mailbox mapping problems with the
  account intact. Keep Calendar authorization independent.
- Document first sync, search coverage, Drafts and Sent behavior, and how to
  replace a revoked app password through **Reconnect**. The new-account route
  can ship independently of provider conversion.

### Conversation identity and safe account conversion

1. Add capability-gated `X-GM-THRID` retrieval and lookup. Bind Gmail's server
   thread ID to stable internal conversations, reusing the established
   `X-GM-MSGID` message mappings. Gmail exposes the same numeric IDs over IMAP
   and the API, with different representations. Preserve existing internal IDs
   and record conflicting evidence for review. Use verified thread or parent
   message associations to surface a reply draft in its conversation.
   [Identity mappings](imap-identity-mappings.md),
   [Gmail IMAP extensions](https://developers.google.com/workspace/gmail/imap/imap-extensions).
2. Add **Change connection** to the existing Gmail account. Stage and validate
   the new IMAP and SMTP settings, verify that the mailbox matches the existing
   account with authoritative Gmail identifiers, and preview unresolved links.
   Keep the old provider binding recoverable until cutover commits. Keep
   unmatched references visible for repair. A sender header or matching subject
   alone provides insufficient evidence to move a reference.
3. Journal the cutover so restart can resume it or recover the old binding.
   Preserve the account row, tasks, goals, snoozes, notes and mail links, saved
   searches, Split Inboxes, sender rules, signatures, reader state, drafts,
   scheduled and queued sends, and existing delivery receipts. Rebind provider
   references with verified IDs and reconcile server drafts before resuming
   writes. Coordinate enrolled devices around the provider change and hold
   writes and sends that could use old bindings or replay an uncertain delivery.
   [Identity audit](imap-identity-audit.md),
   [Conversion acceptance criteria](imap-parity-plan.md#3-offer-safe-connection-conversion).

### Triage, sending, and sync parity

| Gap | Implementation and acceptance |
| --- | --- |
| Native label rename and delete | Add verified IMAP `RENAME` and `DELETE` for native user labels. Give saved labels stable references or explicit aliases, preserve Split Inboxes and pending actions across rename, and retain a deleted or unresolved state across devices. An externally renamed label with ambiguous identity needs explicit relinking. Verify that deleting a Gmail label preserves messages and unrelated labels. Keep ordinary folder deletion safeguards. [Label reference audit](imap-label-reference-audit.md) |
| Uncertain SMTP delivery | Reconcile against the provider's own Sent copy where it saves one. Use the outgoing RFC Message-ID, sender, and reliable server evidence to confirm the copy. A missing or ambiguous copy keeps delivery uncertain and leaves the retry decision with the user. Exercise a lost SMTP reply and delayed Sent visibility. [Gmail Sent behavior](https://support.google.com/mail/answer/78892) |
| Sending addresses and signatures | Build on the existing manual sending addresses with a default From, Reply-To, and reply selection from original recipients or delivery headers. Keep local per-address signature defaults and test which configured aliases Gmail SMTP accepts. Automatic discovery and verification of Gmail's Send mail as settings and importing Gmail's saved signatures remain Gmail API features. [Gmail SendAs settings](https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.settings.sendAs) |
| Search and sync at scale | Exercise large All Mail, overlapping labels, recent Inbox changes during backfill, offline search, and reconnect. Record commands, bytes, and time before tuning. Build on Inbox IDLE and use incremental flag reconciliation such as CONDSTORE/QRESYNC where advertised and supported. Retain visible partial-search feedback when a query hits a coverage limit. [All Mail and search](imap-all-mail.md), [IMAP resynchronization extensions](https://www.rfc-editor.org/rfc/rfc7162.html) |

**Phase 1 release checks**

- Verify the full workflow on a Gmail test account with an app password and on
  a plain IMAP account. Cover setup, reconnect, receive and search, archive and
  undo, labels, Drafts across clients, SMTP and Sent, and scheduled sends.
- Use scripted protocol tests for capability loss, conflicting identity,
  UIDVALIDITY reset, interrupted conversion with an enrolled second device,
  and uncertain SMTP delivery. Use focused UI tests for setup, conversion
  review, and partial results. Exercise an existing Gmail API account with
  linked workflow data and a nonempty Outbox before offering conversion widely.
- Keep general IMAP folder and keyword work in the parity backlog. Report the
  capabilities available on the connected server and the Gmail-specific
  conveniences that still require the API.

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
