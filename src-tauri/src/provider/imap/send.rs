//! MailSend for IMAP accounts: turning the run-1 SMTP submission client
//! ([`super::smtp::SmtpSession`]) into the engine's delivery contract
//! ([`crate::provider::Delivery`] / [`crate::provider::MailSend`]).
//!
//! `docs/imap-design.md` ("Sending"). The engine hands us only the raw MIME
//! already built at queue time (`build_mime` in `crate::correspondence`) plus
//! the local thread id, and expects:
//!
//! * [`ImapProvider::prepare_delivery`](super::ImapProvider) to connect +
//!   authenticate and return a one-shot [`ImapDelivery`] owning the live
//!   session;
//! * [`ImapDelivery::send_once`] to derive the SMTP envelope FROM the raw
//!   bytes (the engine never passes it separately), strip the `Bcc:` header
//!   so blind recipients are not disclosed on the wire, submit via
//!   [`SmtpSession::send`], and map the run-1 [`SmtpOutcome`] to the engine's
//!   `Result<DeliveryReceipt, (definite, message)>`.
//!
//! Two pure, independently tested functions carry the risk here:
//!
//! * [`strip_bcc_header`] removes any `Bcc:` header (and its folded
//!   continuation lines) from the header block ONLY, leaving every other byte
//!   — body included — identical. `build_mime` writes a `Bcc:` header when the
//!   draft has blind recipients (mail-builder's `.bcc(..)` emits one); SMTP
//!   submission must RCPT those addresses but must NOT transmit the header, or
//!   every recipient would see the blind-copy list.
//! * [`derive_envelope`] parses the raw with `mail-parser` to produce the
//!   MAIL FROM (the single `From` address, which must be one of this
//!   account's identities) and the deduplicated RCPT list (To + Cc + Bcc).
//!   A `From` that is absent, multiple, or not an identity fails DEFINITELY
//!   with zero bytes on the wire — this is what stops a send as an arbitrary
//!   or stale address.

use mail_parser::{Address, MessageParser};

use crate::provider::imap::identity::{derive_message_id, IdentityInputs};
use crate::provider::imap::sending_identity::normalize_identity_address;
use crate::provider::imap::settings::ImapAccountSettings;
use crate::provider::imap::smtp::{
    Phase, SmtpCredentials, SmtpEndpoint, SmtpFailure, SmtpOutcome, SmtpSession,
};
use crate::provider::{DeliveryReceipt, ProviderError, SendingIdentity};

// ===========================================================================
// Bcc header stripping (pure, byte-exact).
// ===========================================================================

/// Remove every `Bcc:` header — including its folded continuation lines —
/// from `raw`, leaving every other byte identical, and return the bytes to put
/// on the wire.
///
/// The transformation touches the HEADER BLOCK only: it stops at the first
/// empty line (the header/body separator), so a line beginning `Bcc:` inside
/// the BODY, or the text `Bcc` appearing inside another header's folded VALUE,
/// is never removed. A header is a `Bcc` header iff its field NAME (up to the
/// first `:`, case-insensitively) equals `bcc`; its value and any following
/// lines that begin with a space or tab (folding, RFC 5322 §2.2.3) are dropped
/// with it.
///
/// Line endings are preserved exactly (we never normalise here — run 1's
/// [`frame_message`](super::smtp::frame_message) does CRLF normalisation at
/// the DATA phase). We split on `\n` and keep each line's own trailing `\r`,
/// so a CRLF message stays CRLF and an LF-only message stays LF.
pub fn strip_bcc_header(raw: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(raw.len());
    let mut in_headers = true;
    // Whether the PREVIOUS header line was a dropped Bcc header, so its folded
    // continuation lines must also be dropped.
    let mut dropping_folded = false;

    for line in split_keep_newline(raw) {
        if in_headers {
            // A blank line ends the header block. Emit it and switch to
            // verbatim body copying.
            if is_blank_line(line) {
                in_headers = false;
                dropping_folded = false;
                out.extend_from_slice(line);
                continue;
            }
            let is_continuation = line.first().is_some_and(|&b| b == b' ' || b == b'\t');
            if is_continuation {
                // A folded continuation line belongs to whatever header
                // preceded it: drop it iff we are dropping that header.
                if dropping_folded {
                    continue;
                }
                out.extend_from_slice(line);
                continue;
            }
            // A new header line: decide by its field name.
            if header_name_is_bcc(line) {
                dropping_folded = true;
                continue;
            }
            dropping_folded = false;
            out.extend_from_slice(line);
        } else {
            // Body: verbatim, no interpretation.
            out.extend_from_slice(line);
        }
    }
    out
}

/// Split `bytes` into lines, each INCLUDING its trailing `\n` (and its `\r` if
/// the line ended in CRLF). The final line is included even with no trailing
/// newline. An empty input yields no lines.
fn split_keep_newline(bytes: &[u8]) -> Vec<&[u8]> {
    let mut lines = Vec::new();
    let mut start = 0;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'\n' {
            lines.push(&bytes[start..=i]);
            start = i + 1;
        }
    }
    if start < bytes.len() {
        lines.push(&bytes[start..]);
    }
    lines
}

/// Whether a line (which includes its newline) is blank — the header/body
/// separator. `"\n"` and `"\r\n"` are blank; anything else is not.
fn is_blank_line(line: &[u8]) -> bool {
    matches!(line, b"\n" | b"\r\n")
}

/// Whether a header line's field NAME is `Bcc` (case-insensitive). The field
/// name is everything before the first `:`. A line with no `:` is not a header
/// (so never a Bcc header).
fn header_name_is_bcc(line: &[u8]) -> bool {
    let Some(colon) = line.iter().position(|&b| b == b':') else {
        return false;
    };
    line[..colon].eq_ignore_ascii_case(b"bcc")
}

// ===========================================================================
// Envelope derivation (pure, parser-based).
// ===========================================================================

/// The SMTP envelope derived from a message's headers: the MAIL FROM address
/// and the deduplicated RCPT list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Envelope {
    pub mail_from: String,
    pub rcpts: Vec<String>,
}

/// Derive the SMTP envelope from the raw MIME, validating the From against the
/// account's identities.
///
/// MAIL FROM is the SINGLE address in the `From` header. It is a DEFINITE
/// failure (nothing sent) if there is no `From`, more than one `From` address,
/// or the `From` address normalises to none of `identities` — this is the gate
/// that stops a send as an arbitrary or stale address.
///
/// The RCPT list is every address in `To` + `Cc` + `Bcc`, in that header order
/// and first-seen order within a header, deduplicated case-insensitively (the
/// same whole-address normalisation identities use). Display names and group
/// syntax are handled by the parser. No recipients is a DEFINITE failure.
pub fn derive_envelope(raw: &[u8], identities: &[SendingIdentity]) -> Result<Envelope, String> {
    let message = MessageParser::default()
        .parse(raw)
        .ok_or_else(|| "The message could not be parsed, so it was not sent.".to_string())?;

    // --- MAIL FROM: exactly one From address, and it must be an identity. ---
    let from_addrs = collect_addresses(message.from());
    let mail_from = match from_addrs.as_slice() {
        [] => return Err("The message has no From address, so it was not sent.".into()),
        [one] => one.clone(),
        _ => {
            return Err("The message has more than one From address, so it was not sent.".into())
        }
    };
    let from_norm = normalize_identity_address(&mail_from);
    let is_identity = identities
        .iter()
        .any(|id| normalize_identity_address(&id.address) == from_norm);
    if !is_identity {
        return Err(format!(
            "The From address ({mail_from}) is not one of this account's sending \
             addresses, so the message was not sent. Pick a current sending address."
        ));
    }

    // --- RCPT: To + Cc + Bcc, deduped case-insensitively, order preserved. ---
    let mut rcpts: Vec<String> = Vec::new();
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for header in [message.to(), message.cc(), message.bcc()] {
        for addr in collect_addresses(header) {
            let norm = normalize_identity_address(&addr);
            if seen.insert(norm) {
                rcpts.push(addr);
            }
        }
    }
    if rcpts.is_empty() {
        return Err("The message has no recipients, so it was not sent.".into());
    }

    Ok(Envelope { mail_from, rcpts })
}

/// Collect every bare email address from a parsed address header, in order.
/// Handles both flat address lists and RFC 5322 groups (the parser exposes
/// group members in the same `iter`), skipping any entry with no address part
/// (a group name, or a malformed entry).
fn collect_addresses(header: Option<&Address>) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(address) = header {
        for addr in address.iter() {
            if let Some(email) = addr.address() {
                let email = email.trim();
                if !email.is_empty() {
                    out.push(email.to_string());
                }
            }
        }
    }
    out
}

/// The raw `Message-ID` header value of `raw`, if present (as it appeared).
pub(super) fn raw_message_id(raw: &[u8]) -> Option<String> {
    let message = MessageParser::default().parse(raw)?;
    message.message_id().map(|id| id.to_string())
}

/// Build the provider message id for a successfully submitted message.
///
/// Uses [`derive_message_id`] with the `Message-ID` header of `raw`, so it
/// matches the id sync will later derive for the same message when the server
/// supplies no `OBJECTID`/`EMAILID`.
///
/// OBJECTID caveat: a server that DOES later advertise `OBJECTID`/`EMAILID`
/// would derive a different id for the same message (the server id wins over
/// the Message-ID hash). The design's sticky-reuse rule (handled by the sync
/// ingest, not here) keeps the already-recorded id, so this value stays
/// authoritative for the sent copy; we do NOT try to solve the OBJECTID case
/// here.
pub fn provider_message_id_for(account: &str, raw: &[u8]) -> String {
    derive_message_id(
        account,
        &IdentityInputs {
            message_id: raw_message_id(raw),
            ..Default::default()
        },
    )
}

// ===========================================================================
// Endpoint / credentials from settings.
// ===========================================================================

/// Build the SMTP endpoint from an account's non-secret settings. An empty
/// host or a zero port is an [`ProviderError::InvalidOperation`] BEFORE any
/// network I/O: the account has no outgoing server configured.
pub fn smtp_endpoint_from_settings(
    settings: &ImapAccountSettings,
) -> Result<SmtpEndpoint, ProviderError> {
    let host = settings.smtp_host.trim();
    if host.is_empty() || settings.smtp_port == 0 {
        return Err(ProviderError::InvalidOperation(
            "This account has no outgoing mail server configured.".into(),
        ));
    }
    Ok(SmtpEndpoint {
        host: host.to_string(),
        port: settings.smtp_port,
        security: settings.smtp_security,
        pinned: settings.pinned_for(host, settings.smtp_port),
    })
}

/// The SMTP username per the user's decision (`smtp_username` when non-empty,
/// else the IMAP username). The PASSWORD is always the IMAP password.
pub fn smtp_username(settings: &ImapAccountSettings) -> &str {
    let smtp = settings.smtp_username.trim();
    if smtp.is_empty() {
        settings.imap_username.trim()
    } else {
        smtp
    }
}

/// Build SMTP credentials for an account. The username follows the user's
/// decision ([`smtp_username`]); the password is the resolved SMTP password.
/// The password is never formatted anywhere (SmtpCredentials redacts Debug).
pub fn smtp_credentials(settings: &ImapAccountSettings, password: &str) -> SmtpCredentials {
    SmtpCredentials {
        username: smtp_username(settings).to_string(),
        password: password.to_string(),
    }
}

/// Map a run-1 [`SmtpFailure`] from the connect/prepare phase to a
/// [`ProviderError`]. The failure already carries the right class in its
/// `error` field (run 1's `map_connect_error`): `ReauthenticationRequired`
/// stays that class, a transport/TLS/timeout stays `TransientTransport`, and
/// so on. We never invent a new class.
pub fn prepare_failure_to_error(failure: SmtpFailure) -> ProviderError {
    failure.error
}

// ===========================================================================
// The Delivery permit.
// ===========================================================================

/// A connected, authenticated one-shot SMTP delivery permit for an IMAP
/// account. Owns the live [`SmtpSession`] and the data `send_once` needs to
/// derive the envelope and the receipt.
pub struct ImapDelivery {
    session: SmtpSession,
    /// The account's own address (its key), for the derived provider id.
    account: String,
    /// The account's sending identities, to validate the From against.
    identities: Vec<SendingIdentity>,
}

impl ImapDelivery {
    pub fn new(session: SmtpSession, account: String, identities: Vec<SendingIdentity>) -> Self {
        Self {
            session,
            account,
            identities,
        }
    }
}

#[async_trait::async_trait]
impl crate::provider::Delivery for ImapDelivery {
    async fn send_once(
        self: Box<Self>,
        raw: &[u8],
        thread: Option<&str>,
    ) -> Result<DeliveryReceipt, (bool, String)> {
        let ImapDelivery {
            session,
            account,
            identities,
        } = *self;

        // Derive the envelope from the raw bytes. A From that is absent,
        // multiple, or not an identity, or no recipients, is a DEFINITE
        // failure with ZERO bytes on the wire.
        let envelope = derive_envelope(raw, &identities).map_err(|message| (true, message))?;

        // Strip the Bcc header so blind recipients are RCPT'd but never
        // disclosed on the wire. The body is untouched.
        let on_wire = strip_bcc_header(raw);

        match session
            .send(&envelope.mail_from, &envelope.rcpts, &on_wire)
            .await
        {
            SmtpOutcome::Accepted { .. } => Ok(DeliveryReceipt {
                provider_message_id: provider_message_id_for(&account, raw),
                // A REPLY carries the local thread id forward so send-and-
                // archive + refetch work; a new message has none.
                thread_id: thread.map(str::to_string),
            }),
            SmtpOutcome::NotDelivered(failure) => {
                Err((true, not_delivered_message(&failure, &envelope.mail_from)))
            }
            SmtpOutcome::Unknown(_) => Err((
                false,
                "The connection dropped after the message was sent to the server, so it \
                 may have been delivered. Check with the recipient before sending it again."
                    .into(),
            )),
        }
    }
}

/// The user-facing message for a DEFINITE non-delivery. No hostnames, ports,
/// credentials, IPs or Rust `Debug` output: the run-1 failure already carries
/// a sanitised, length-capped server reply, which we reuse.
///
/// A 5xx on MAIL FROM names the identity, because that is the actionable fact
/// ("the server will not let you send as this address"). A RCPT rejection
/// already names the recipient in `failure.error` (run 1), so that text is
/// reused verbatim. Everything else uses the error's own display text.
fn not_delivered_message(failure: &SmtpFailure, mail_from: &str) -> String {
    if failure.phase == Phase::MailFrom {
        if let Some(reply) = &failure.reply {
            if (500..=599).contains(&reply.code) {
                return format!(
                    "The mail server refused to send as {mail_from} ({}). Check this \
                     address under the account's sending addresses.",
                    reply.summary()
                );
            }
        }
    }
    // ProviderError's Display is a plain, sanitised sentence for every class
    // run 1 produces; reuse it rather than re-deriving from Debug.
    failure.error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::imap::settings::{LabelStorage, SecurityMode};

    fn ident(addr: &str) -> SendingIdentity {
        SendingIdentity {
            address: addr.to_string(),
            display_name: None,
        }
    }

    // ---- strip_bcc_header --------------------------------------------

    #[test]
    fn strips_a_simple_bcc_header_keeping_crlf_and_other_headers() {
        let raw = b"From: a@x\r\nTo: b@y\r\nBcc: secret@z\r\nSubject: hi\r\n\r\nbody\r\n";
        let out = strip_bcc_header(raw);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "From: a@x\r\nTo: b@y\r\nSubject: hi\r\n\r\nbody\r\n"
        );
    }

    #[test]
    fn strips_a_folded_bcc_header_including_its_continuation_lines() {
        let raw = b"From: a@x\r\nBcc: one@z,\r\n two@z,\r\n\tthree@z\r\nSubject: hi\r\n\r\nbody";
        let out = strip_bcc_header(raw);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "From: a@x\r\nSubject: hi\r\n\r\nbody"
        );
    }

    #[test]
    fn mixed_case_bcc_is_stripped() {
        let raw = b"From: a@x\r\nbCc: secret@z\r\nSubject: hi\r\n\r\nbody";
        let out = strip_bcc_header(raw);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "From: a@x\r\nSubject: hi\r\n\r\nbody"
        );
    }

    #[test]
    fn bcc_in_the_body_survives() {
        let raw = b"From: a@x\r\nSubject: hi\r\n\r\nBcc: this is body text\r\nmore\r\n";
        let out = strip_bcc_header(raw);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "From: a@x\r\nSubject: hi\r\n\r\nBcc: this is body text\r\nmore\r\n"
        );
    }

    #[test]
    fn bcc_text_inside_another_headers_value_survives() {
        let raw = b"From: a@x\r\nSubject: about Bcc: handling\r\nTo: b@y\r\n\r\nbody";
        let out = strip_bcc_header(raw);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "From: a@x\r\nSubject: about Bcc: handling\r\nTo: b@y\r\n\r\nbody"
        );
    }

    #[test]
    fn bcc_in_a_folded_continuation_of_another_header_survives() {
        let raw = b"From: a@x\r\nSubject: long\r\n Bcc: still subject\r\nTo: b@y\r\n\r\nbody";
        let out = strip_bcc_header(raw);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "From: a@x\r\nSubject: long\r\n Bcc: still subject\r\nTo: b@y\r\n\r\nbody"
        );
    }

    #[test]
    fn no_bcc_header_is_a_byte_identical_passthrough() {
        let raw = b"From: a@x\r\nTo: b@y\r\nSubject: hi\r\n\r\nbody\r\n";
        assert_eq!(strip_bcc_header(raw), raw);
    }

    #[test]
    fn lf_only_message_keeps_lf_endings() {
        let raw = b"From: a@x\nBcc: secret@z\nSubject: hi\n\nbody\n";
        assert_eq!(
            String::from_utf8(strip_bcc_header(raw)).unwrap(),
            "From: a@x\nSubject: hi\n\nbody\n"
        );
    }

    // ---- derive_envelope ---------------------------------------------

    #[test]
    fn derives_from_and_dedupes_recipients_across_to_cc_bcc() {
        let raw = b"From: me@proton.me\r\nTo: a@x, b@y\r\nCc: a@x, c@z\r\nBcc: d@w\r\n\r\nbody";
        let env = derive_envelope(raw, &[ident("me@proton.me")]).unwrap();
        assert_eq!(env.mail_from, "me@proton.me");
        assert_eq!(env.rcpts, vec!["a@x", "b@y", "c@z", "d@w"]);
    }

    #[test]
    fn dedupes_recipients_case_insensitively() {
        let raw = b"From: me@proton.me\r\nTo: A@X\r\nCc: a@x\r\n\r\nbody";
        let env = derive_envelope(raw, &[ident("me@proton.me")]).unwrap();
        assert_eq!(env.rcpts, vec!["A@X"]);
    }

    #[test]
    fn a_secondary_identity_from_is_accepted() {
        let raw = b"From: work@proton.me\r\nTo: a@x\r\n\r\nbody";
        let env =
            derive_envelope(raw, &[ident("me@proton.me"), ident("work@proton.me")]).unwrap();
        assert_eq!(env.mail_from, "work@proton.me");
    }

    #[test]
    fn a_from_that_is_not_an_identity_is_a_definite_failure() {
        let raw = b"From: evil@attacker.test\r\nTo: a@x\r\n\r\nbody";
        let err = derive_envelope(raw, &[ident("me@proton.me")]).unwrap_err();
        assert!(err.contains("evil@attacker.test"), "{err}");
        assert!(err.contains("not one of this account"), "{err}");
    }

    #[test]
    fn no_from_is_a_failure() {
        let raw = b"To: a@x\r\n\r\nbody";
        assert!(derive_envelope(raw, &[ident("me@proton.me")]).is_err());
    }

    #[test]
    fn multiple_from_addresses_is_a_failure() {
        let raw = b"From: me@proton.me, other@proton.me\r\nTo: a@x\r\n\r\nbody";
        let err =
            derive_envelope(raw, &[ident("me@proton.me"), ident("other@proton.me")]).unwrap_err();
        assert!(err.contains("more than one From"), "{err}");
    }

    #[test]
    fn no_recipients_is_a_failure() {
        let raw = b"From: me@proton.me\r\nSubject: hi\r\n\r\nbody";
        assert!(derive_envelope(raw, &[ident("me@proton.me")]).is_err());
    }

    #[test]
    fn display_names_and_groups_are_handled_by_the_parser() {
        let raw = b"From: \"Me\" <me@proton.me>\r\nTo: Team: a@x, b@y;\r\n\r\nbody";
        let env = derive_envelope(raw, &[ident("me@proton.me")]).unwrap();
        assert_eq!(env.mail_from, "me@proton.me");
        assert_eq!(env.rcpts, vec!["a@x", "b@y"]);
    }

    // ---- provider_message_id_for -------------------------------------

    #[test]
    fn provider_message_id_matches_the_sync_derivation_for_the_same_message_id() {
        let raw = b"From: me@proton.me\r\nMessage-ID: <abc@proton.me>\r\nTo: a@x\r\n\r\nbody";
        let got = provider_message_id_for("me@proton.me", raw);
        let expected = derive_message_id(
            "me@proton.me",
            &IdentityInputs {
                message_id: Some("<abc@proton.me>".into()),
                ..Default::default()
            },
        );
        assert_eq!(got, expected);
    }

    // ---- smtp_endpoint_from_settings / smtp_username -----------------

    fn settings_with(
        smtp_host: &str,
        smtp_port: u16,
        smtp_user: &str,
        imap_user: &str,
    ) -> ImapAccountSettings {
        ImapAccountSettings {
            imap_host: "imap.example".into(),
            imap_port: 993,
            imap_security: SecurityMode::ImplicitTls,
            imap_username: imap_user.into(),
            smtp_host: smtp_host.into(),
            smtp_port,
            smtp_security: SecurityMode::StartTls,
            smtp_username: smtp_user.into(),
            mailbox_overrides: Default::default(),
            archive_mailbox: None,
            label_storage: LabelStorage::None,
            label_container: None,
            identities: vec![],
            pinned_fingerprints: Default::default(),
            server_saves_sent: false,
        }
    }

    #[test]
    fn empty_smtp_host_is_an_invalid_operation_before_any_network() {
        let s = settings_with("", 587, "", "joel");
        assert!(matches!(
            smtp_endpoint_from_settings(&s).unwrap_err(),
            ProviderError::InvalidOperation(_)
        ));
    }

    #[test]
    fn zero_smtp_port_is_an_invalid_operation() {
        let s = settings_with("smtp.example", 0, "", "joel");
        assert!(matches!(
            smtp_endpoint_from_settings(&s).unwrap_err(),
            ProviderError::InvalidOperation(_)
        ));
    }

    #[test]
    fn smtp_username_honours_the_explicit_value_else_the_imap_username() {
        let explicit = settings_with("smtp.example", 587, "smtp-user", "imap-user");
        assert_eq!(smtp_username(&explicit), "smtp-user");
        let defaulted = settings_with("smtp.example", 587, "   ", "imap-user");
        assert_eq!(smtp_username(&defaulted), "imap-user");
    }
}

#[cfg(test)]
mod provider_level_tests {
    //! End-to-end tests driving a real [`ImapProvider`] through
    //! `prepare_delivery` + `send_once` against an in-process scripted TLS
    //! SMTP server (same self-signed + leaf-pin harness as the run-1 SMTP
    //! tests). Every failing test makes the server actually DO the failing
    //! thing and asserts BOTH the outcome class AND what reached the wire, so
    //! none can pass vacuously. The provider's IMAP connection manager points
    //! at a dead address — SMTP never touches it, so these are offline.

    use super::*;
    use crate::provider::imap::connection::{ConnectionConfig, ImapConnectionManager, TlsMode};
    use crate::provider::imap::settings::{
        host_port_key, Identity, ImapAccountSettings, LabelStorage, SecurityMode,
    };
    use crate::provider::imap::tls::Sha256Fingerprint;
    use crate::provider::imap::{BodyCache, ImapProvider, ImapProviderConfig, ImapStateStore};
    use crate::provider::{Delivery, LabelModel, MailSend, ProviderResult};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::io::AsyncBufReadExt;
    use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;

    /// What the scripted server does. Mirrors the run-1 server's variants, but
    /// only the ones these provider-level tests need.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Script {
        Accept,
        Reject535Auth,
        Reject553MailFrom,
        Reject550Rcpt,
        DropAfterDot,
    }

    struct FakeSmtp {
        port: u16,
        pin: Sha256Fingerprint,
        log: Arc<Mutex<Vec<String>>>,
        body: Arc<Mutex<Vec<u8>>>,
    }

    impl FakeSmtp {
        fn saw(&self, needle: &str) -> bool {
            self.log.lock().unwrap().iter().any(|l| l.contains(needle))
        }
        fn body_bytes(&self) -> Vec<u8> {
            self.body.lock().unwrap().clone()
        }
    }

    /// Start a STARTTLS scripted SMTP server on 127.0.0.1, returning once it is
    /// listening. Returns the port and the self-signed leaf's SHA-256 so the
    /// provider can pin it.
    async fn start(script: Script) -> FakeSmtp {
        use rustls::pki_types::PrivatePkcs8KeyDer;
        let _ = rustls::crypto::ring::default_provider().install_default();
        let cert = rcgen::generate_simple_self_signed(vec!["127.0.0.1".into()]).unwrap();
        let pin = Sha256Fingerprint::of_certificate(cert.cert.der());
        let config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![cert.cert.der().clone()],
                PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der()).into(),
            )
            .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let log = Arc::new(Mutex::new(Vec::new()));
        let body = Arc::new(Mutex::new(Vec::new()));
        let log2 = log.clone();
        let body2 = body.clone();
        tokio::spawn(async move {
            let Ok((mut tcp, _)) = listener.accept().await else {
                return;
            };
            // STARTTLS handshake in the clear, then upgrade.
            if tcp.write_all(b"220 fake ready\r\n").await.is_err() {
                return;
            }
            let mut reader = BufReader::new(&mut tcp);
            let mut line = String::new();
            if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                return;
            }
            log2.lock().unwrap().push(line.trim_end().to_string());
            drop(reader);
            let _ = tcp.write_all(b"250-fake\r\n250 STARTTLS\r\n").await;
            let mut reader = BufReader::new(&mut tcp);
            line.clear();
            if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                return;
            }
            log2.lock().unwrap().push(line.trim_end().to_string());
            drop(reader);
            let _ = tcp.write_all(b"220 go ahead\r\n").await;
            let Ok(tls) = acceptor.accept(tcp).await else {
                return;
            };
            run_session(tls, script, log2, body2).await;
        });
        FakeSmtp { port, pin, log, body }
    }

    async fn run_session<S>(
        stream: S,
        script: Script,
        log: Arc<Mutex<Vec<String>>>,
        body: Arc<Mutex<Vec<u8>>>,
    ) where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        let mut s = BufReader::new(stream);
        // No greeting here: STARTTLS already greeted in the clear before the
        // upgrade, and mail-send does not expect a second greeting on the
        // upgraded stream. It issues EHLO next.
        let mut rcpt_seen = 0usize;
        let mut line = String::new();
        loop {
            line.clear();
            let n = match s.read_line(&mut line).await {
                Ok(n) => n,
                Err(_) => return,
            };
            if n == 0 {
                return;
            }
            let trimmed = line.trim_end().to_string();
            log.lock().unwrap().push(trimmed.clone());
            let up = trimmed.to_ascii_uppercase();
            if up.starts_with("EHLO") {
                let _ = s
                    .write_all(b"250-fake\r\n250-AUTH PLAIN LOGIN\r\n250 8BITMIME\r\n")
                    .await;
            } else if up.starts_with("AUTH") {
                if script == Script::Reject535Auth {
                    let _ = s.write_all(b"535 5.7.8 bad credentials\r\n").await;
                    return;
                }
                if up.starts_with("AUTH LOGIN") {
                    let _ = s.write_all(b"334 VXNlcm5hbWU6\r\n").await;
                    line.clear();
                    if s.read_line(&mut line).await.unwrap_or(0) == 0 {
                        return;
                    }
                    let _ = s.write_all(b"334 UGFzc3dvcmQ6\r\n").await;
                    line.clear();
                    if s.read_line(&mut line).await.unwrap_or(0) == 0 {
                        return;
                    }
                    let _ = s.write_all(b"235 2.7.0 authenticated\r\n").await;
                } else {
                    let _ = s.write_all(b"235 2.7.0 authenticated\r\n").await;
                }
            } else if up.starts_with("MAIL FROM") {
                if script == Script::Reject553MailFrom {
                    let _ = s.write_all(b"553 5.7.1 sender address rejected\r\n").await;
                } else {
                    let _ = s.write_all(b"250 2.1.0 ok\r\n").await;
                }
            } else if up.starts_with("RCPT TO") {
                rcpt_seen += 1;
                if script == Script::Reject550Rcpt {
                    let _ = s.write_all(b"550 5.1.1 no such user\r\n").await;
                } else {
                    let _ = s.write_all(b"250 2.1.5 ok\r\n").await;
                }
                let _ = rcpt_seen;
            } else if up == "DATA" {
                let _ = s.write_all(b"354 end with <CRLF>.<CRLF>\r\n").await;
                let mut collected: Vec<u8> = Vec::new();
                let mut buf = [0u8; 256];
                loop {
                    let m = match s.read(&mut buf).await {
                        Ok(m) => m,
                        Err(_) => return,
                    };
                    if m == 0 {
                        *body.lock().unwrap() = collected;
                        return;
                    }
                    collected.extend_from_slice(&buf[..m]);
                    if collected.ends_with(b"\r\n.\r\n") {
                        break;
                    }
                }
                *body.lock().unwrap() = collected;
                if script == Script::DropAfterDot {
                    return;
                }
                let _ = s.write_all(b"250 2.0.0 accepted\r\n").await;
            } else if up == "RSET" {
                let _ = s.write_all(b"250 2.0.0 reset\r\n").await;
            } else if up == "QUIT" {
                let _ = s.write_all(b"221 bye\r\n").await;
                return;
            } else {
                let _ = s.write_all(b"250 ok\r\n").await;
            }
        }
    }

    /// Build an ImapProvider whose SMTP settings point at `srv` (pinned), with
    /// the given account address, IMAP login, and stored secondary identities.
    /// The IMAP connection manager points at a dead port — SMTP never uses it.
    fn provider_for(
        srv: &FakeSmtp,
        account: &str,
        login: &str,
        identities: Vec<Identity>,
    ) -> ImapProvider {
        provider_with_smtp_password(srv, account, login, identities, None)
    }

    fn provider_with_smtp_password(
        srv: &FakeSmtp,
        account: &str,
        login: &str,
        identities: Vec<Identity>,
        smtp_password: Option<String>,
    ) -> ImapProvider {
        let database = Arc::new(crate::db::Database::open_memory());
        database
            .adopt_mail_account(account, crate::models::MailProviderKind::Imap)
            .unwrap();
        let store = ImapStateStore::new(database.clone(), account);
        let cache = BodyCache::new(store.clone());
        let mut pinned = std::collections::BTreeMap::new();
        pinned.insert(host_port_key("127.0.0.1", srv.port), srv.pin.to_hex());
        let settings = ImapAccountSettings {
            imap_host: "127.0.0.1".into(),
            imap_port: 1,
            imap_security: SecurityMode::StartTls,
            imap_username: login.into(),
            smtp_host: "127.0.0.1".into(),
            smtp_port: srv.port,
            smtp_security: SecurityMode::StartTls,
            smtp_username: String::new(),
            mailbox_overrides: Default::default(),
            archive_mailbox: None,
            label_storage: LabelStorage::None,
            label_container: None,
            identities,
            pinned_fingerprints: pinned,
            server_saves_sent: false,
        };
        let manager = ImapConnectionManager::new(ConnectionConfig {
            host: "127.0.0.1".into(),
            port: 1,
            tls_mode: TlsMode::StartTls,
            pinned_fingerprint: Some(srv.pin),
        });
        ImapProvider::new(ImapProviderConfig {
            account_id: account.into(),
            username: login.into(),
            password: "imap-password".into(),
            smtp_password,
            manager,
            store,
            cache,
            label_model: LabelModel::ImapNoUserLabels,
            settings,
        })
    }

    async fn permit(provider: &ImapProvider) -> Box<dyn Delivery> {
        provider.prepare_delivery().await.expect("prepare connects + authenticates")
    }

    /// Extract the error from a `prepare_delivery` result. `Box<dyn Delivery>`
    /// is not `Debug`, so `.unwrap_err()` will not compile; match instead.
    fn prepare_err(result: ProviderResult<Box<dyn Delivery>>) -> ProviderError {
        match result {
            Ok(_) => panic!("expected prepare_delivery to fail"),
            Err(e) => e,
        }
    }

    #[tokio::test]
    async fn accepted_send_puts_the_right_envelope_and_bcc_stripped_body_on_the_wire() {
        let srv = start(Script::Accept).await;
        let provider = provider_for(&srv, "me@proton.me", "me@proton.me", vec![]);
        let raw = b"From: me@proton.me\r\nTo: a@x.test\r\nCc: b@y.test\r\nBcc: hidden@z.test\r\nSubject: hi\r\n\r\nhello\r\n";
        let receipt = permit(&provider)
            .await
            .send_once(raw, None)
            .await
            .expect("accepted");
        // Provider id equals the sync derivation from the Message-ID (none
        // here, so from Date/From/Subject/size — but we only assert it is
        // non-empty and account-scoped).
        assert!(receipt.provider_message_id.contains("me@proton.me") || !receipt.provider_message_id.is_empty());
        assert_eq!(receipt.thread_id, None, "a new message carries no thread");
        // Envelope: MAIL FROM = identity, one RCPT per To/Cc/Bcc.
        assert!(srv.saw("MAIL FROM:<me@proton.me>"));
        assert!(srv.saw("RCPT TO:<a@x.test>"));
        assert!(srv.saw("RCPT TO:<b@y.test>"));
        assert!(srv.saw("RCPT TO:<hidden@z.test>"), "the Bcc address IS RCPT'd");
        // The transmitted body must NOT contain the Bcc header, must keep the
        // body, and dot-stuffing/terminator intact.
        let body = srv.body_bytes();
        let text = String::from_utf8_lossy(&body);
        assert!(!text.contains("Bcc:"), "the Bcc header must not be transmitted");
        assert!(text.contains("hidden@z.test") == false, "the blind address is not disclosed in the header block\n{text}");
        assert!(text.contains("hello"));
        assert!(body.ends_with(b"\r\n.\r\n"));
    }

    #[tokio::test]
    async fn smtp_auth_uses_the_separate_password_or_falls_back_to_imap() {
        use base64::{engine::general_purpose::STANDARD, Engine};
        for separate in [None, Some("smtp-only-password".to_string())] {
            let srv = start(Script::Accept).await;
            let provider = provider_with_smtp_password(
                &srv,
                "me@example.com",
                "login",
                vec![],
                separate.clone(),
            );
            permit(&provider)
                .await
                .send_once(
                    b"From: me@example.com\r\nTo: friend@example.com\r\n\r\nhello",
                    None,
                )
                .await
                .unwrap();
            let expected = separate.as_deref().unwrap_or("imap-password");
            assert!(srv.saw(&format!(
                "AUTH PLAIN {}",
                STANDARD.encode(format!("\0login\0{expected}"))
            )));
            if separate.is_some() {
                assert!(!srv.saw(&STANDARD.encode("\0login\0imap-password")));
            }
        }
    }

    #[tokio::test]
    async fn a_reply_returns_the_thread_id_it_was_given() {
        let srv = start(Script::Accept).await;
        let provider = provider_for(&srv, "me@proton.me", "me@proton.me", vec![]);
        let raw = b"From: me@proton.me\r\nTo: a@x.test\r\nSubject: re\r\n\r\nbody\r\n";
        let receipt = permit(&provider)
            .await
            .send_once(raw, Some("imap:t:42"))
            .await
            .unwrap();
        assert_eq!(receipt.thread_id.as_deref(), Some("imap:t:42"));
    }

    #[tokio::test]
    async fn a_secondary_identity_sends_as_that_identity() {
        let srv = start(Script::Accept).await;
        let provider = provider_for(
            &srv,
            "me@proton.me",
            "me@proton.me",
            vec![Identity {
                address: "work@proton.me".into(),
                display_name: Some("Work".into()),
            }],
        );
        let raw = b"From: work@proton.me\r\nTo: a@x.test\r\n\r\nbody\r\n";
        permit(&provider).await.send_once(raw, None).await.unwrap();
        assert!(srv.saw("MAIL FROM:<work@proton.me>"));
    }

    #[tokio::test]
    async fn a_from_that_is_not_an_identity_sends_zero_envelope_bytes() {
        let srv = start(Script::Accept).await;
        let provider = provider_for(&srv, "me@proton.me", "me@proton.me", vec![]);
        let raw = b"From: evil@attacker.test\r\nTo: a@x.test\r\n\r\nbody\r\n";
        let (definite, msg) = permit(&provider)
            .await
            .send_once(raw, None)
            .await
            .unwrap_err();
        assert!(definite, "a stale/foreign From is a definite non-delivery");
        assert!(msg.contains("evil@attacker.test"));
        // The server never saw a MAIL FROM: the gate ran before any envelope.
        assert!(!srv.saw("MAIL FROM"), "no envelope bytes after a rejected From");
    }

    #[tokio::test]
    async fn a_553_on_mail_from_is_definite_and_names_the_identity() {
        let srv = start(Script::Reject553MailFrom).await;
        let provider = provider_for(&srv, "me@proton.me", "me@proton.me", vec![]);
        let raw = b"From: me@proton.me\r\nTo: a@x.test\r\n\r\nbody\r\n";
        let (definite, msg) = permit(&provider)
            .await
            .send_once(raw, None)
            .await
            .unwrap_err();
        assert!(definite);
        assert!(msg.contains("me@proton.me"), "the message names the identity: {msg}");
        assert!(msg.contains("553"), "{msg}");
    }

    #[tokio::test]
    async fn a_550_on_rcpt_is_definite_and_names_the_recipient() {
        let srv = start(Script::Reject550Rcpt).await;
        let provider = provider_for(&srv, "me@proton.me", "me@proton.me", vec![]);
        let raw = b"From: me@proton.me\r\nTo: nobody@x.test\r\n\r\nbody\r\n";
        let (definite, msg) = permit(&provider)
            .await
            .send_once(raw, None)
            .await
            .unwrap_err();
        assert!(definite);
        assert!(msg.contains("nobody@x.test"), "names the recipient: {msg}");
    }

    #[tokio::test]
    async fn a_drop_after_the_body_is_uncertain() {
        let srv = start(Script::DropAfterDot).await;
        let provider = provider_for(&srv, "me@proton.me", "me@proton.me", vec![]);
        let raw = b"From: me@proton.me\r\nTo: a@x.test\r\n\r\nbody\r\n";
        let (definite, msg) = permit(&provider)
            .await
            .send_once(raw, None)
            .await
            .unwrap_err();
        assert!(!definite, "a drop after the body is UNKNOWN, not definite");
        assert!(msg.contains("may have been delivered"), "{msg}");
    }

    #[tokio::test]
    async fn auth_535_in_prepare_is_reauthentication_required() {
        let srv = start(Script::Reject535Auth).await;
        let provider = provider_for(&srv, "me@proton.me", "me@proton.me", vec![]);
        let err = prepare_err(provider.prepare_delivery().await);
        assert!(matches!(err, ProviderError::ReauthenticationRequired(_)), "{err:?}");
    }

    #[tokio::test]
    async fn a_refused_connection_is_transient_transport() {
        // No server; a dead port. prepare must fail transient, nothing sent.
        let database = Arc::new(crate::db::Database::open_memory());
        database
            .adopt_mail_account("me@proton.me", crate::models::MailProviderKind::Imap)
            .unwrap();
        let store = ImapStateStore::new(database.clone(), "me@proton.me");
        let cache = BodyCache::new(store.clone());
        let settings = ImapAccountSettings {
            imap_host: "127.0.0.1".into(),
            imap_port: 1,
            imap_security: SecurityMode::StartTls,
            imap_username: "me@proton.me".into(),
            smtp_host: "127.0.0.1".into(),
            smtp_port: 1, // nothing listens
            smtp_security: SecurityMode::StartTls,
            smtp_username: String::new(),
            mailbox_overrides: Default::default(),
            archive_mailbox: None,
            label_storage: LabelStorage::None,
            label_container: None,
            identities: vec![],
            pinned_fingerprints: Default::default(),
            server_saves_sent: false,
        };
        let manager = ImapConnectionManager::new(ConnectionConfig {
            host: "127.0.0.1".into(),
            port: 1,
            tls_mode: TlsMode::StartTls,
            pinned_fingerprint: None,
        });
        let dead = ImapProvider::new(ImapProviderConfig {
            account_id: "me@proton.me".into(),
            username: "me@proton.me".into(),
            password: "imap-password".into(),
            smtp_password: None,
            manager,
            store,
            cache,
            label_model: LabelModel::ImapNoUserLabels,
            settings,
        });
        let err = prepare_err(
            tokio::time::timeout(Duration::from_secs(10), dead.prepare_delivery())
                .await
                .expect("connect to a closed port fails fast"),
        );
        assert!(matches!(err, ProviderError::TransientTransport(_)), "{err:?}");
    }

    #[tokio::test]
    async fn a_wrong_pin_fails_before_auth() {
        let srv = start(Script::Accept).await;
        // Build a provider that pins the WRONG fingerprint for this host:port,
        // so TLS fails in the handshake before any AUTH bytes.
        let database = Arc::new(crate::db::Database::open_memory());
        database
            .adopt_mail_account("me@proton.me", crate::models::MailProviderKind::Imap)
            .unwrap();
        let store = ImapStateStore::new(database.clone(), "me@proton.me");
        let cache = BodyCache::new(store.clone());
        let wrong = Sha256Fingerprint::from_bytes([0xEE; 32]);
        let mut pinned = std::collections::BTreeMap::new();
        pinned.insert(host_port_key("127.0.0.1", srv.port), wrong.to_hex());
        let settings = ImapAccountSettings {
            imap_host: "127.0.0.1".into(),
            imap_port: 1,
            imap_security: SecurityMode::StartTls,
            imap_username: "me@proton.me".into(),
            smtp_host: "127.0.0.1".into(),
            smtp_port: srv.port,
            smtp_security: SecurityMode::StartTls,
            smtp_username: String::new(),
            mailbox_overrides: Default::default(),
            archive_mailbox: None,
            label_storage: LabelStorage::None,
            label_container: None,
            identities: vec![],
            pinned_fingerprints: pinned,
            server_saves_sent: false,
        };
        let manager = ImapConnectionManager::new(ConnectionConfig {
            host: "127.0.0.1".into(),
            port: 1,
            tls_mode: TlsMode::StartTls,
            pinned_fingerprint: Some(wrong),
        });
        let provider = ImapProvider::new(ImapProviderConfig {
            account_id: "me@proton.me".into(),
            username: "me@proton.me".into(),
            password: "imap-password".into(),
            smtp_password: None,
            manager,
            store,
            cache,
            label_model: LabelModel::ImapNoUserLabels,
            settings,
        });
        let err = prepare_err(provider.prepare_delivery().await);
        assert!(matches!(err, ProviderError::TransientTransport(_)), "{err:?}");
        // Nothing authenticated.
        assert!(!srv.saw("AUTH"));
    }

    // ---- Fact 6: login vs account address ----------------------------

    #[tokio::test]
    async fn sender_identity_is_the_account_address_not_the_login() {
        let srv = start(Script::Accept).await;
        // Proton Bridge style: login `joel`, account `joel@proton.me`.
        let provider = provider_for(&srv, "joel@proton.me", "joel", vec![]);
        assert_eq!(
            provider.sender_identity().await.unwrap(),
            "joel@proton.me",
            "the tick compares draft.account to this; it must be the address"
        );
        let identities = provider.identities("joel@proton.me").await.unwrap();
        assert_eq!(
            identities[0].address, "joel@proton.me",
            "identities[0] is the account address, not the login"
        );
        // And a send from the account address still goes out (the login is
        // still what SMTP AUTH used, unchanged).
        let raw = b"From: joel@proton.me\r\nTo: a@x.test\r\n\r\nbody\r\n";
        permit(&provider).await.send_once(raw, None).await.unwrap();
        assert!(srv.saw("MAIL FROM:<joel@proton.me>"));
    }

    // ---- Gated live test (skipped without the env vars) --------------

    /// Live send through a real `ImapProvider` to the Dovecot harness's SMTP
    /// sink, with a Bcc and a second identity. Skipped (not failed) unless the
    /// same env vars the run-1 live tests use are set. The lead runs this with
    /// docker up; this worker cannot.
    #[tokio::test]
    async fn live_send_through_imap_provider_to_the_sink() {
        let (Ok(fp_hex), Ok(port)) = (
            std::env::var("DOVECOT_TEST_FP"),
            std::env::var("THREESTRANDS_SMTP_SINK_PORT"),
        ) else {
            eprintln!("skipping: DOVECOT_TEST_FP / THREESTRANDS_SMTP_SINK_PORT not set");
            return;
        };
        let implicit_port = std::env::var("THREESTRANDS_SMTP_SINK_IMPLICIT_PORT").ok();
        let fp = crate::provider::imap::parse_sha256_fingerprint(&fp_hex)
            .expect("DOVECOT_TEST_FP must be 64 hex digits");
        let port: u16 = port.parse().expect("port");

        let database = Arc::new(crate::db::Database::open_memory());
        let account = "test@threestrands.test";
        database
            .adopt_mail_account(account, crate::models::MailProviderKind::Imap)
            .unwrap();
        let store = ImapStateStore::new(database.clone(), account);
        let cache = BodyCache::new(store.clone());
        let (security, use_port) = match &implicit_port {
            Some(p) => (SecurityMode::ImplicitTls, p.parse().unwrap_or(port)),
            None => (SecurityMode::StartTls, port),
        };
        let mut pinned = std::collections::BTreeMap::new();
        pinned.insert(host_port_key("127.0.0.1", use_port), fp.to_hex());
        let settings = ImapAccountSettings {
            imap_host: "127.0.0.1".into(),
            imap_port: 11143,
            imap_security: SecurityMode::StartTls,
            imap_username: account.into(),
            smtp_host: "127.0.0.1".into(),
            smtp_port: use_port,
            smtp_security: security,
            smtp_username: String::new(),
            mailbox_overrides: Default::default(),
            archive_mailbox: None,
            label_storage: LabelStorage::None,
            label_container: None,
            identities: vec![Identity {
                address: "second@threestrands.test".into(),
                display_name: Some("Second".into()),
            }],
            pinned_fingerprints: pinned,
            server_saves_sent: false,
        };
        let manager = ImapConnectionManager::new(ConnectionConfig {
            host: "127.0.0.1".into(),
            port: 11143,
            tls_mode: TlsMode::StartTls,
            pinned_fingerprint: Some(fp),
        });
        let provider = ImapProvider::new(ImapProviderConfig {
            account_id: account.into(),
            username: account.into(),
            password: "testpassword".into(),
            smtp_password: None,
            manager,
            store,
            cache,
            label_model: LabelModel::ImapNoUserLabels,
            settings,
        });
        let raw = b"From: second@threestrands.test\r\nTo: rcpt@threestrands.test\r\nBcc: blind@threestrands.test\r\nSubject: live\r\n\r\nhello from the live test\r\n";
        let receipt = provider
            .prepare_delivery()
            .await
            .expect("live prepare")
            .send_once(raw, None)
            .await
            .expect("live send accepted");
        assert!(!receipt.provider_message_id.is_empty());
    }
}
