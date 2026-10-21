//! Sending ([`MailSend`]): identities, SMTP delivery, and the Sent copy.

use async_trait::async_trait;

use crate::provider::{Delivery, DeliveryReceipt, MailSend, ProviderError, ProviderResult};

use super::mutate::as_write_session;
use super::{db_err, ImapProvider};

#[async_trait]
impl MailSend for ImapProvider {
    async fn sender_identity(&self) -> ProviderResult<String> {
        // The ACCOUNT address (the key drafts are filed under), NOT the IMAP
        // login. The tick pauses every item whose `draft.account` differs from
        // this value, so returning the login would silently pause ALL sends
        // for an account whose login differs from its address (e.g. Proton
        // Bridge login `joel` for `joel@proton.me`). `docs/imap-design.md`
        // ("Identities"), brief fact 6. The login is still what IMAP LOGIN and
        // SMTP AUTH use — that is unchanged, it just is not the identity.
        Ok(self.account_id.clone())
    }

    async fn identities(&self, _account: &str) -> ProviderResult<Vec<crate::provider::imap::SendingIdentity>> {
        // The one source of truth: the ACCOUNT address first and always, then
        // the stored list, normalized/validated. The first entry is the
        // account's own sending address (its key), not the login, for the same
        // reason `sender_identity()` returns the account address. No DB round
        // trip — the settings are carried in the provider.
        Ok(crate::provider::imap::account_identities(
            &self.account_id,
            &self.settings.identities,
        ))
    }

    fn message_id_domain(&self, from_address: &str) -> ProviderResult<Option<String>> {
        // An IMAP send generates `<id@sender-domain>`, where the domain is the
        // domain of the From address actually used. An invalid domain is an
        // error BEFORE any send — never a silent fallback to the reserved
        // `threestrands.local` (which would leak the client name).
        crate::provider::imap::message_id_domain_for_address(from_address).map(Some)
    }

    async fn prepare_delivery(&self) -> ProviderResult<Box<dyn Delivery>> {
        // Build the endpoint + credentials from the account's non-secret
        // settings and its SMTP password (falling back to IMAP when no
        // separate password was saved). An empty host / zero port is InvalidOperation
        // BEFORE any network. The password is never formatted — it goes only
        // into SmtpCredentials (which redacts Debug).
        let endpoint = crate::provider::imap::send::smtp_endpoint_from_settings(&self.settings)?;
        let creds = crate::provider::imap::send::smtp_credentials(&self.settings, &self.smtp_password);

        // Connect + authenticate. Any failure here is a DEFINITE non-delivery
        // (nothing sent); map it to the existing ProviderError class the
        // run-1 client already chose (ReauthenticationRequired stays that
        // class, TLS/connect/timeout stays TransientTransport, etc.).
        let session = crate::provider::imap::smtp::SmtpSession::connect(&endpoint, &creds)
            .await
            .map_err(crate::provider::imap::send::prepare_failure_to_error)?;

        // The permit owns the live session plus what send_once needs to derive
        // the envelope and the receipt: the account address (for the provider
        // id) and the identity list (to validate the From).
        let identities = crate::provider::imap::account_identities(&self.account_id, &self.settings.identities);
        Ok(Box::new(crate::provider::imap::send::ImapDelivery::new(
            session,
            self.account_id.clone(),
            identities,
        )))
    }

    async fn find_sent_copy(
        &self,
        _operation: &str,
        _expected_sender: &str,
    ) -> ProviderResult<Option<DeliveryReceipt>> {
        // IMAP keeps no server-side sent copy we can confirm an uncertain send
        // against: only WE would APPEND to `\Sent`, and if the connection
        // dropped before we could, there is nothing to look up. So this is a
        // clean `Ok(None)`, NOT an error — `reconcile_with_provider` reads it
        // together with `verifiable_delivery = false` and resolves the send to
        // the terminal `Unverifiable` state ("may have been sent"). Returning
        // an error here instead would make `reconcile_with_provider` abort via
        // its `map_err(error)?` and the send would never reach that branch.
        // (Run 4 may add the Sent-copy APPEND; it would revisit this.)
        Ok(None)
    }

    /// Save the sender's own copy of a just-sent message into the Sent mailbox,
    /// or decide nothing is needed, driven by the engine's durable job (run 4b;
    /// `docs/imap-design.md`, "Sending" steps 3–4). The whole decision is the
    /// pure [`crate::provider::imap::write::decide_sent_copy`] over a leased-session IO; this
    /// method only resolves the inputs (Message-ID, Sent mailbox, override) and
    /// leases the session.
    async fn save_sent_copy(
        &self,
        request: &crate::provider::SentCopyRequest,
    ) -> ProviderResult<crate::provider::SentCopyStep> {
        // Nothing needs locating or uploading when the server owns Sent
        // copies. This decision must precede mailbox resolution and network IO.
        if self.settings.server_saves_sent
            || self.store.sent_copy_detection().map_err(db_err)? == Some(true)
        {
            return Ok(crate::provider::SentCopyStep::Done);
        }
        // The Message-ID is required to locate our copy; a message without one
        // can never be found again, so refuse PERMANENTLY (abandons the job).
        let message_id = crate::provider::imap::send::raw_message_id(&request.raw).ok_or_else(|| {
            ProviderError::InvalidOperation(
                "cannot save the Sent copy: the message has no Message-ID header to locate it by; \
                 refusing permanently"
                    .into(),
            )
        })?;

        // Resolve the Sent mailbox from the plan exactly as run 4a does (a WIRE
        // name; never re-encoded). A missing Sent mailbox is permanent.
        let plan = self.build_sync_plan()?;
        let mailbox = self
            .mailbox_with_role(&plan, crate::provider::imap::MailboxRole::Sent)
            .ok_or_else(|| {
                ProviderError::InvalidOperation(
                    "cannot save the Sent copy: this account has no Sent mailbox configured; \
                     refusing permanently"
                        .into(),
                )
            })?;

        // Lease one session for the whole decision (the command slot, like
        // every other write path).
        let mut session = self.open_session().await?;
        let result = {
            let write_session = as_write_session(session.as_mut())?;
            let mut io = crate::provider::imap::write::SessionSentCopyIo {
                session: write_session,
                store: &self.store,
                mailbox,
                raw: &request.raw,
                message_id,
                server_saves_sent: self.settings.server_saves_sent,
                config: crate::provider::imap::write::SaveSentConfig::default(),
                now: self.now,
            };
            crate::provider::imap::write::decide_sent_copy(&mut io, request, crate::provider::imap::SENT_COPY_VERIFY_DELAY).await
        };
        let _ = session.logout().await;
        result
    }

    fn server_drafts(&self) -> bool {
        self.drafts_mailbox().is_some()
    }

    async fn find_server_draft(&self, message_id: &str) -> ProviderResult<Option<crate::provider::ServerDraft>> {
        self.find_draft_copy(message_id).await
    }

    async fn create_server_draft(&self, raw: &[u8], _thread: Option<&str>) -> ProviderResult<crate::provider::ServerDraft> {
        self.write_draft_copy(raw, None).await
    }

    async fn update_server_draft(&self, id: &str, raw: &[u8], _thread: Option<&str>) -> ProviderResult<crate::provider::ServerDraft> {
        self.write_draft_copy(raw, Some(id)).await
    }

    async fn delete_server_draft(&self, id: &str) -> ProviderResult<()> {
        self.delete_draft_copy(id).await
    }

    async fn list_server_drafts(&self) -> ProviderResult<Vec<crate::provider::ServerDraft>> {
        self.list_draft_copies().await
    }

    async fn fetch_server_draft(&self, id: &str) -> ProviderResult<crate::provider::FetchedServerDraft> {
        self.fetch_draft_copy(id).await
    }

    async fn server_draft_version(&self, id: &str) -> ProviderResult<Option<String>> {
        self.draft_copy_version(id).await
    }
}
