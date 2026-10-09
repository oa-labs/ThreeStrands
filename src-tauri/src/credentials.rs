//! The keychain credential envelope.
//!
//! Every secret this app stores in the OS keychain is written as this tagged
//! JSON value rather than a bare provider-specific payload, so a single
//! keychain entry can eventually hold OAuth tokens for one provider and, say,
//! an IMAP password for another, and a reader can always tell which it has
//! without guessing from context.
//!
//! Entries written before this envelope existed are bare `Tokens` JSON.
//! [`StoredCredential::decode`] falls back to reading those directly as
//! [`StoredCredential::GoogleOAuth`], so upgrading needs no keychain
//! migration pass of its own — the next token refresh calls
//! [`StoredCredential::encode`] and rewrites the entry tagged, and the fleet
//! converges one save at a time.

use serde::{Deserialize, Serialize};

use crate::auth::Tokens;

/// An IMAP/SMTP password credential, stored in the keychain under the same
/// tagged envelope as every other secret. The IMAP password is always
/// present; the SMTP password is `None` when the same password authenticates
/// both submission and retrieval, which is the common case. Only the secrets
/// live here — the non-secret server settings (hosts, ports, security mode,
/// usernames) belong in the `imap_account_settings` table added with the IMAP
/// provider, never in the keychain and never where a password could leak.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImapPassword {
    pub imap: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub smtp: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum StoredCredential {
    #[serde(rename = "google_oauth")]
    GoogleOAuth(Tokens),
    /// An IMAP account's password(s). Added in Phase 1 slice 2 ahead of the
    /// IMAP provider (phase 2) that reads it; stored exactly like
    /// [`Self::GoogleOAuth`] so the keychain envelope needs no change to hold
    /// a non-OAuth credential.
    #[serde(rename = "imap_password")]
    ImapPassword(ImapPassword),
}

impl StoredCredential {
    pub fn encode(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|error| error.to_string())
    }

    /// Reads a stored credential, accepting both the tagged envelope and a
    /// bare `Tokens` payload from before the envelope existed. Tries the
    /// tagged shape first: a bare `Tokens` object has no `kind` field, so it
    /// can only ever fail to parse as `Self`, never succeed with the wrong
    /// variant.
    pub fn decode(value: &str) -> Result<Self, String> {
        if let Ok(tagged) = serde_json::from_str::<Self>(value) {
            return Ok(tagged);
        }
        serde_json::from_str::<Tokens>(value)
            .map(Self::GoogleOAuth)
            .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens() -> Tokens {
        Tokens {
            access_token: "at".into(),
            refresh_token: Some("rt".into()),
            expires_at: 1234,
        }
    }

    #[test]
    fn a_bare_pre_envelope_payload_still_decodes() {
        let legacy = serde_json::to_string(&tokens()).unwrap();
        let StoredCredential::GoogleOAuth(decoded) = StoredCredential::decode(&legacy).unwrap()
        else {
            panic!("expected a GoogleOAuth credential");
        };
        assert_eq!(decoded.access_token, "at");
        assert_eq!(decoded.refresh_token.as_deref(), Some("rt"));
    }

    #[test]
    fn an_encoded_credential_round_trips_tagged() {
        let encoded = StoredCredential::GoogleOAuth(tokens()).encode().unwrap();
        assert!(
            encoded.contains("\"kind\":\"google_oauth\""),
            "expected a tagged envelope, got: {encoded}"
        );
        let StoredCredential::GoogleOAuth(decoded) = StoredCredential::decode(&encoded).unwrap()
        else {
            panic!("expected a GoogleOAuth credential");
        };
        assert_eq!(decoded.access_token, "at");
    }

    #[test]
    fn an_imap_password_credential_round_trips_tagged() {
        let encoded = StoredCredential::ImapPassword(ImapPassword {
            imap: "imap-secret".into(),
            smtp: Some("smtp-secret".into()),
        })
        .encode()
        .unwrap();
        assert!(
            encoded.contains("\"kind\":\"imap_password\""),
            "expected a tagged envelope, got: {encoded}"
        );
        let StoredCredential::ImapPassword(decoded) = StoredCredential::decode(&encoded).unwrap()
        else {
            panic!("expected an ImapPassword credential");
        };
        assert_eq!(decoded.imap, "imap-secret");
        assert_eq!(decoded.smtp.as_deref(), Some("smtp-secret"));
    }

    #[test]
    fn an_imap_password_omits_an_absent_smtp_secret() {
        let encoded = StoredCredential::ImapPassword(ImapPassword {
            imap: "imap-secret".into(),
            smtp: None,
        })
        .encode()
        .unwrap();
        // Omitted, not serialized as null, so a shared-password account
        // never records an empty SMTP secret.
        assert!(!encoded.contains("smtp"), "expected no smtp key, got: {encoded}");
        let StoredCredential::ImapPassword(decoded) = StoredCredential::decode(&encoded).unwrap()
        else {
            panic!("expected an ImapPassword credential");
        };
        assert_eq!(decoded.imap, "imap-secret");
        assert_eq!(decoded.smtp, None);
    }

    #[test]
    fn a_bare_pre_envelope_payload_is_still_read_as_google_oauth_not_imap() {
        // The bare-Tokens fallback must stay pinned to GoogleOAuth: a legacy
        // entry predates IMAP entirely, so it can never be an IMAP password.
        let legacy = serde_json::to_string(&tokens()).unwrap();
        assert!(matches!(
            StoredCredential::decode(&legacy).unwrap(),
            StoredCredential::GoogleOAuth(_)
        ));
    }
}
