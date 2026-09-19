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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum StoredCredential {
    #[serde(rename = "google_oauth")]
    GoogleOAuth(Tokens),
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
        let StoredCredential::GoogleOAuth(decoded) = StoredCredential::decode(&legacy).unwrap();
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
        let StoredCredential::GoogleOAuth(decoded) = StoredCredential::decode(&encoded).unwrap();
        assert_eq!(decoded.access_token, "at");
    }
}
