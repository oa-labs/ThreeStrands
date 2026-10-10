//! RFC 5530 IMAP response-code mapping onto [`ProviderError`].
//!
//! `docs/imap-design.md` ("Sync" / "Errors") fixes the table:
//!
//! | IMAP condition                                   | `ProviderError`              |
//! |--------------------------------------------------|------------------------------|
//! | `AUTHENTICATIONFAILED`                           | `ReauthenticationRequired`   |
//! | `UNAVAILABLE`, `INUSE`, dropped conn, timeout    | `TransientTransport`         |
//! | `OVERQUOTA`                                      | `PermanentClientRejection`   |
//! | `TRYCREATE`                                      | (caller) create + retry once |
//! | `BAD` (and other `No`/`Bad` without a code)      | `InvalidOperation`           |
//!
//! Only an explicit authentication failure pauses the account, matching the
//! Gmail rule that only `invalid_grant` forces a reconnect
//! ([`ProviderError::requires_reauthentication`]).
//!
//! `TRYCREATE` is a two-step action, not a single mapped error: the caller
//! creates the target mailbox and retries once. It is modelled here as its own
//! [`ImapFailure::TryCreate`] variant so the connection layer can branch on it;
//! the create-and-retry hook lands with the mailbox/mutation slices (there is
//! no mailbox-create path in Slice 1), so for now it maps to a
//! `TransientTransport` carrying the retry intent, with a `// Slice 3` note.

use async_imap::imap_proto::{Response, ResponseCode};

use crate::provider::ProviderError;

/// A classified IMAP failure, independent of the concrete `async-imap`/IO
/// error type, so the connection layer and the error table are testable
/// without a live server. [`classify_status_text`] builds one from what we can
/// observe at the wire; [`ImapFailure::into_provider_error`] applies the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImapFailure {
    /// `AUTHENTICATIONFAILED` — credentials no longer work.
    AuthenticationFailed(String),
    /// `UNAVAILABLE` / `INUSE` — the server is temporarily refusing, retry.
    ServerUnavailable(String),
    /// A dropped connection, a read/write IO error, or a timeout.
    TransportDropped(String),
    /// `OVERQUOTA` — a durable client-side condition the user must resolve.
    OverQuota(String),
    /// `TRYCREATE` — the target mailbox is absent; create it and retry once.
    TryCreate(String),
    /// A `BAD` response or any other protocol-level rejection with no code we
    /// map specially.
    Bad(String),
}

impl ImapFailure {
    /// Apply the RFC 5530 table.
    pub fn into_provider_error(self) -> ProviderError {
        match self {
            ImapFailure::AuthenticationFailed(m) => ProviderError::ReauthenticationRequired(m),
            ImapFailure::ServerUnavailable(m) | ImapFailure::TransportDropped(m) => {
                ProviderError::TransientTransport(m)
            }
            ImapFailure::OverQuota(m) => ProviderError::PermanentClientRejection(format!(
                "the mail server reports the account is over its storage quota: {m}"
            )),
            // Slice 3 (mailbox discovery / mutations) owns the create-and-retry
            // hook; there is no CREATE path in Slice 1, so a TRYCREATE here is
            // surfaced as retryable transport with the intent recorded. The
            // connection layer can match `ImapFailure::TryCreate` directly
            // before this fallback once that path exists.
            ImapFailure::TryCreate(m) => ProviderError::TransientTransport(format!(
                "mailbox does not exist (TRYCREATE); create-and-retry lands in Slice 3: {m}"
            )),
            ImapFailure::Bad(m) => ProviderError::InvalidOperation(m),
        }
    }
}

/// Classify an RFC 5530 response code (as `async-imap` parses it) plus the
/// human-readable status text into an [`ImapFailure`]. Recognized typed codes
/// take precedence over diagnostic text. Codes imap-proto does not recognize
/// remain at the start of the information field, including their brackets.
pub fn classify_response_code(code: Option<&ResponseCode<'_>>, text: &str) -> ImapFailure {
    let text = text.trim();
    match code {
        Some(ResponseCode::TryCreate) => ImapFailure::TryCreate(text.to_string()),
        Some(_) => ImapFailure::Bad(text.to_string()),
        None => {
            let code = text
                .strip_prefix('[')
                .and_then(|rest| rest.split_once(']'))
                .map(|(code, _)| code.to_ascii_uppercase());
            match code.as_deref() {
                Some("AUTHENTICATIONFAILED") => ImapFailure::AuthenticationFailed(text.to_string()),
                Some("OVERQUOTA") => ImapFailure::OverQuota(text.to_string()),
                Some("TRYCREATE") => ImapFailure::TryCreate(text.to_string()),
                Some("UNAVAILABLE" | "INUSE") => ImapFailure::ServerUnavailable(text.to_string()),
                _ => ImapFailure::Bad(text.to_string()),
            }
        }
    }
}

/// Classify bracketed response text or a complete IMAP status line. Only the
/// response-code position is significant; mailbox names and diagnostic text
/// must never determine authentication or retry policy.
pub fn classify_status_text(text: &str) -> ImapFailure {
    let text = text.trim();
    if text.starts_with('[') {
        return classify_response_code(None, text);
    }
    let wire = format!("{text}\r\n");
    if let Ok(([], Response::Done { outcome, .. } | Response::Data { outcome, .. })) =
        Response::parse(wire.as_bytes())
    {
        return classify_response_code(
            outcome.code.as_ref(),
            outcome.information.as_deref().unwrap_or_default(),
        );
    }
    ImapFailure::Bad(text.to_string())
}

/// async-imap 0.12 erases parsed outcomes into two debug-string formats:
/// `check_status_ok` uses `code: ..., info: ...`, while SELECT/FETCH parsers
/// use `outcome: Outcome { code: ..., information: ... }`. Inspect only their
/// leading fields. Unknown codes survive at the start of the quoted
/// information; recognized typed codes must not fall back to that text.
fn classify_async_imap_status_text(text: &str) -> ImapFailure {
    let fields = text
        .strip_prefix("code: ")
        .map(|fields| (fields, "None, info: Some(\""))
        .or_else(|| {
            text.strip_prefix("outcome: Outcome { code: ")
                .map(|fields| (fields, "None, information: Some(\""))
        });
    if let Some((fields, information_prefix)) = fields {
        if fields.starts_with("Some(TryCreate), ") {
            return ImapFailure::TryCreate(text.to_string());
        }
        if let Some(information) = fields.strip_prefix(information_prefix) {
            return classify_response_code(None, information);
        }
        return ImapFailure::Bad(text.to_string());
    }
    classify_status_text(text)
}

/// Map an `async-imap` error directly to a [`ProviderError`], classifying IO /
/// connection-drop kinds as transient and parsing any protocol status text
/// through [`classify_status_text`]. This is the single entry point the
/// connection layer calls.
pub fn map_imap_error(err: &async_imap::error::Error) -> ProviderError {
    use async_imap::error::Error as E;
    let failure = match err {
        // A `No`/`Bad` tagged response carries the server's status text, where
        // the RFC 5530 code word lives.
        E::No(text) | E::Bad(text) => classify_async_imap_status_text(text),
        // Connection-level failures: dropped socket or IO error. All transient
        // transport.
        E::ConnectionLost => ImapFailure::TransportDropped("connection lost".to_string()),
        E::Io(io) => ImapFailure::TransportDropped(format!("io error: {io}")),
        // Anything else (parse errors, protocol validation) is a client-side
        // invalid operation we should not retry blindly.
        other => ImapFailure::Bad(format!("{other}")),
    };
    failure.into_provider_error()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authentication_failed_requires_reauthentication_and_pauses_the_account() {
        let err = ImapFailure::AuthenticationFailed("[AUTHENTICATIONFAILED] bad creds".into())
            .into_provider_error();
        assert!(matches!(err, ProviderError::ReauthenticationRequired(_)));
        assert!(err.requires_reauthentication());
        // A wrong password is not fixed by retrying the mutation.
        assert!(!err.retry_mutation());
    }

    #[test]
    fn unavailable_inuse_drop_and_timeout_are_transient_transport() {
        for failure in [
            ImapFailure::ServerUnavailable("[UNAVAILABLE] try later".into()),
            ImapFailure::ServerUnavailable("[INUSE] too many connections".into()),
            ImapFailure::TransportDropped("connection lost".into()),
            ImapFailure::TransportDropped("io error: timed out".into()),
        ] {
            let err = failure.into_provider_error();
            assert!(
                matches!(err, ProviderError::TransientTransport(_)),
                "expected TransientTransport, got {err:?}"
            );
            assert!(!err.requires_reauthentication());
        }
    }

    #[test]
    fn overquota_is_a_readable_permanent_client_rejection() {
        let err = ImapFailure::OverQuota("[OVERQUOTA] mailbox full".into()).into_provider_error();
        match err {
            ProviderError::PermanentClientRejection(msg) => {
                assert!(msg.contains("over its storage quota"), "{msg}");
            }
            other => panic!("expected PermanentClientRejection, got {other:?}"),
        }
    }

    #[test]
    fn bad_is_an_invalid_operation() {
        let err = ImapFailure::Bad("[BAD] command unrecognised".into()).into_provider_error();
        assert!(matches!(err, ProviderError::InvalidOperation(_)));
    }

    #[test]
    fn trycreate_is_recognised_and_carries_the_slice_3_retry_intent() {
        // Classification picks the TryCreate variant out of the status text...
        let failure = classify_status_text("[TRYCREATE] Mailbox doesn't exist: Archive");
        assert_eq!(
            failure,
            ImapFailure::TryCreate("[TRYCREATE] Mailbox doesn't exist: Archive".into())
        );
        // ...and until the Slice 3 create-and-retry hook exists it maps to a
        // retryable transport error that names the deferral.
        let err = failure.into_provider_error();
        match err {
            ProviderError::TransientTransport(msg) => {
                assert!(msg.contains("TRYCREATE"), "{msg}");
                assert!(msg.contains("Slice 3"), "{msg}");
            }
            other => panic!("expected TransientTransport, got {other:?}"),
        }
    }

    #[test]
    fn status_text_classification_is_server_neutral_and_case_insensitive() {
        assert!(matches!(
            classify_status_text("a01 no [authenticationfailed] Invalid credentials"),
            ImapFailure::AuthenticationFailed(_)
        ));
        assert!(matches!(
            classify_status_text("* BYE [UNAVAILABLE] Server shutting down"),
            ImapFailure::ServerUnavailable(_)
        ));
        // No recognised code => Bad / InvalidOperation.
        assert!(matches!(
            classify_status_text("a02 NO Something unexpected"),
            ImapFailure::Bad(_)
        ));
    }

    #[test]
    fn async_imap_connection_loss_maps_to_transient_transport() {
        let err = map_imap_error(&async_imap::error::Error::ConnectionLost);
        assert!(matches!(err, ProviderError::TransientTransport(_)));
    }

    #[test]
    fn async_imap_no_response_text_is_classified_through_the_table() {
        let err = map_imap_error(&async_imap::error::Error::No(
            "[AUTHENTICATIONFAILED] nope".into(),
        ));
        assert!(matches!(err, ProviderError::ReauthenticationRequired(_)));
    }

    #[test]
    fn diagnostic_text_and_partial_code_names_do_not_change_error_policy() {
        for text in [
            "[NONEXISTENT] Unknown mailbox: INUSE",
            "[NONEXISTENT] Unknown mailbox: AUTHENTICATIONFAILED",
            "Unknown mailbox: OVERQUOTA",
            "Mailbox doesn't exist: [TRYCREATE]",
            "[UNAVAILABLE-LATER] try again",
            "[AUTHENTICATIONFAILED_EXTRA] unrelated code",
            "[AUTHENTICATIONFAILED missing closing bracket",
        ] {
            assert!(
                matches!(classify_status_text(text), ImapFailure::Bad(_)),
                "diagnostic text must not be interpreted as a response code: {text}"
            );
        }
    }

    #[test]
    fn typed_response_codes_take_precedence_over_diagnostic_text() {
        assert!(matches!(
            classify_response_code(Some(&ResponseCode::TryCreate), "AUTHENTICATIONFAILED"),
            ImapFailure::TryCreate(_)
        ));
        assert!(matches!(
            classify_response_code(
                Some(&ResponseCode::Alert),
                "[AUTHENTICATIONFAILED] diagnostic"
            ),
            ImapFailure::Bad(_)
        ));
        assert!(matches!(
            classify_status_text("a1 NO [UNAVAILABLE] backend AUTHENTICATIONFAILED"),
            ImapFailure::ServerUnavailable(_)
        ));
    }
}
