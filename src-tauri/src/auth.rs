use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use keyring::Entry;
use rand::{rngs::OsRng, RngCore};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    time::{timeout, Instant},
};
use tokio_util::sync::CancellationToken;
use url::Url;

const SERVICE: &str = "app.dispatch.mail";
const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const PROFILE_URL: &str = "https://gmail.googleapis.com/gmail/v1/users/me/profile";
const SCOPES: &str = "openid email https://www.googleapis.com/auth/gmail.modify https://www.googleapis.com/auth/gmail.labels";
const OAUTH_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const OAUTH_REQUEST_TIMEOUT: Duration = Duration::from_secs(45);

/// Keychain keys used before an account's real Gmail address is known.
/// `LEGACY_KEY` names a pre-upgrade install's one connected account, still
/// stored under the fixed key used before multi-account support existed.
/// `PENDING_KEY` names an in-progress "add account" flow. Both are replaced
/// by the real address the moment it's learned, via `GoogleAuth::rekey_to`.
const LEGACY_KEY: &str = "default";
const PENDING_KEY: &str = "pending";
/// Returned when a newer sign-in attempt superseded this one.
const CANCELED: &str = "Sign-in was canceled by a newer attempt.";

#[derive(Debug, thiserror::Error)]
pub enum AccessTokenError {
    #[error("{0}")]
    ReauthenticationRequired(String),
    #[error("{0}")]
    Transient(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: u64,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: u64,
}

#[derive(Deserialize)]
struct Profile {
    #[serde(rename = "emailAddress")]
    email_address: String,
}

/// The app's shared Google OAuth client credentials. One Google Cloud OAuth
/// client is used for every connected account; only the token and consent
/// are per-account, so this is what constructs a [`GoogleAuth`] for each one.
#[derive(Clone)]
pub struct GoogleAuthConfig {
    client: Client,
    client_id: String,
    client_secret: String,
}

impl GoogleAuthConfig {
    pub fn from_environment() -> Result<Self, String> {
        let client_id = std::env::var("DISPATCH_GOOGLE_CLIENT_ID")
            .ok()
            .or_else(|| option_env!("DISPATCH_GOOGLE_CLIENT_ID").map(str::to_owned))
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                "Google OAuth is not configured. Set DISPATCH_GOOGLE_CLIENT_ID to an installed-app client ID."
                    .to_string()
            })?;
        let client_secret = std::env::var("DISPATCH_GOOGLE_CLIENT_SECRET")
            .ok()
            .or_else(|| option_env!("DISPATCH_GOOGLE_CLIENT_SECRET").map(str::to_owned))
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                "Google OAuth is not configured. Set DISPATCH_GOOGLE_CLIENT_SECRET to the value from the Desktop app credential."
                    .to_string()
            })?;
        Ok(Self {
            client: build_oauth_client(OAUTH_CONNECT_TIMEOUT, OAUTH_REQUEST_TIMEOUT)?,
            client_id,
            client_secret,
        })
    }

    /// The account representing a pre-upgrade install's single connected
    /// identity (or a brand-new "Continue with Google" flow before
    /// multi-account UI existed), before it has learned its real Gmail
    /// address.
    pub fn legacy_account(&self) -> GoogleAuth {
        self.keyed(LEGACY_KEY)
    }

    /// A fresh account for an in-progress "add account" flow. `authorize()`
    /// rekeys it to the account's real address once sign-in completes.
    pub fn pending_account(&self) -> GoogleAuth {
        self.keyed(PENDING_KEY)
    }

    /// An account already known by its real Gmail address, e.g. to
    /// reconnect or remove it.
    pub fn account(&self, email: &str) -> GoogleAuth {
        self.keyed(email)
    }

    fn keyed(&self, key: &str) -> GoogleAuth {
        GoogleAuth {
            client: self.client.clone(),
            client_id: self.client_id.clone(),
            client_secret: self.client_secret.clone(),
            key: Arc::new(Mutex::new(key.to_string())),
            token_cache: Arc::new(Mutex::new(None)),
            available_cache: Arc::new(Mutex::new(None)),
        }
    }
}

#[derive(Clone)]
pub struct GoogleAuth {
    client: Client,
    client_id: String,
    client_secret: String,
    key: Arc<Mutex<String>>,
    token_cache: Arc<Mutex<Option<Tokens>>>,
    available_cache: Arc<Mutex<Option<bool>>>,
}

impl GoogleAuth {
    /// The Gmail address this instance represents, once known; otherwise the
    /// placeholder key (`"default"`/`"pending"`) it was constructed with.
    pub fn key(&self) -> String {
        self.key.lock().unwrap().clone()
    }

    pub fn available(&self) -> bool {
        let cached = *self.available_cache.lock().unwrap();
        if let Some(value) = cached {
            return value;
        }
        let value = self
            .entry()
            .and_then(|entry| entry.get_password().map_err(|error| error.to_string()))
            .is_ok();
        *self.available_cache.lock().unwrap() = Some(value);
        value
    }

    /// Runs the interactive PKCE flow and learns which Gmail address just
    /// authorized, so the user never has to type an email. If this instance
    /// already represents a real address, the authorized account must match
    /// it — reconnecting one account can never silently adopt another
    /// account's tokens. Returns the authorized email.
    ///
    /// `cancel` lets a caller abandon this attempt from the outside — e.g.
    /// because the user started a newer "add account"/"reconnect" flow
    /// before finishing (or closing) the browser tab this one opened. Without
    /// it, an abandoned flow would sit waiting on the loopback listener for
    /// the full timeout, silently blocking any retry that shares its slot.
    pub async fn authorize(&self, cancel: &CancellationToken) -> Result<String, String> {
        let tokens = self.run_pkce_flow(cancel).await?;
        let email = fetch_email(&self.client, &tokens.access_token).await?;
        self.accept_identity(&email)?;
        self.save(&tokens)?;
        Ok(email)
    }

    /// Confirms this instance represents `email`, rekeying its keychain entry
    /// from a placeholder if this is the first time it's been identified.
    /// Errors if it already represents a different real address.
    pub fn accept_identity(&self, email: &str) -> Result<(), String> {
        let current = self.key();
        if current != LEGACY_KEY && current != PENDING_KEY && current != email {
            return Err(format!(
                "Signed in as {email}, but this reconnects {current}. Choose {current} in the browser and try again."
            ));
        }
        self.rekey_to(email)
    }

    async fn run_pkce_flow(&self, cancel: &CancellationToken) -> Result<Tokens, String> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.map_err(display)?;
        let redirect_uri = format!(
            "http://127.0.0.1:{}/oauth/callback",
            listener.local_addr().map_err(display)?.port()
        );
        let verifier = random_urlsafe(64);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let state = random_urlsafe(32);
        let mut authorization = Url::parse(AUTH_URL).map_err(display)?;
        authorization
            .query_pairs_mut()
            .append_pair("client_id", &self.client_id)
            .append_pair("redirect_uri", &redirect_uri)
            .append_pair("response_type", "code")
            .append_pair("scope", SCOPES)
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &state)
            .append_pair("access_type", "offline")
            .append_pair("prompt", "consent");
        open::that(authorization.as_str()).map_err(display)?;

        let (mut stream, target) = self.accept_callback(&listener, cancel).await?;
        let callback = Url::parse(&format!("http://localhost{target}")).map_err(display)?;
        let values = callback
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();
        let result = if values.get("state").map(|value| value.as_ref()) != Some(state.as_str()) {
            Err("OAuth state did not match; sign-in was rejected".to_string())
        } else if let Some(error) = values.get("error") {
            Err(format!("Google authorization failed: {error}"))
        } else {
            let code = values
                .get("code")
                .ok_or_else(|| "OAuth callback did not contain a code".to_string())?;
            self.exchange_code(code, &verifier, &redirect_uri).await
        };
        let (status, body) = if result.is_ok() {
            ("200 OK", "Dispatch is connected. You can close this tab.")
        } else {
            (
                "400 Bad Request",
                "Dispatch could not complete sign-in. Return to the app.",
            )
        };
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes()).await;
        result
    }

    /// Accepts loopback connections until one is actually the browser's
    /// redirect to `/oauth/callback`, instead of trusting whichever
    /// connection happens to land first. Browsers routinely open extra
    /// connections against a page they just navigated to — a favicon
    /// request, a speculative preconnect — and treating one of those as
    /// *the* callback made sign-in either fail outright or, worse, sit
    /// waiting on a follow-up read that never comes until it times out. Any
    /// non-matching connection gets a quick 404 and is dropped so the real
    /// one still gets a chance within the overall deadline.
    async fn accept_callback(
        &self,
        listener: &TcpListener,
        cancel: &CancellationToken,
    ) -> Result<(tokio::net::TcpStream, String), String> {
        let deadline = Instant::now() + Duration::from_secs(180);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err("OAuth callback timed out".to_string());
            }
            let (mut stream, _) = tokio::select! {
                _ = cancel.cancelled() => return Err(CANCELED.to_string()),
                result = timeout(remaining, listener.accept()) => {
                    result.map_err(|_| "OAuth callback timed out".to_string())?.map_err(display)?
                }
            };
            let mut request = vec![0_u8; 16 * 1024];
            let read = tokio::select! {
                _ = cancel.cancelled() => return Err(CANCELED.to_string()),
                result = timeout(Duration::from_secs(5), stream.read(&mut request)) => result,
            };
            let Ok(Ok(count)) = read else {
                // Incomplete or silent connection (e.g. a preconnect that
                // never sends a request) — not the callback; keep waiting.
                continue;
            };
            let target = String::from_utf8_lossy(&request[..count])
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1).map(str::to_string));
            if let Some(target) = target.filter(|path| path.starts_with("/oauth/callback")) {
                return Ok((stream, target));
            }
            let _ = stream
                .write_all(b"HTTP/1.1 404 Not Found\r\nConnection: close\r\n\r\n")
                .await;
        }
    }

    async fn exchange_code(
        &self,
        code: &str,
        verifier: &str,
        redirect_uri: &str,
    ) -> Result<Tokens, String> {
        let response = self
            .client
            .post(TOKEN_URL)
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("code", code),
                ("code_verifier", verifier),
                ("grant_type", "authorization_code"),
                ("redirect_uri", redirect_uri),
            ])
            .send()
            .await
            .map_err(display)?;
        let response = checked(response).await?;
        let token: TokenResponse = response.json().await.map_err(display)?;
        Ok(Tokens {
            access_token: token.access_token,
            refresh_token: token.refresh_token,
            expires_at: now() + token.expires_in.saturating_sub(60),
        })
    }

    pub async fn access_token(&self) -> Result<String, AccessTokenError> {
        let mut tokens = self
            .load()
            .map_err(AccessTokenError::ReauthenticationRequired)?;
        if tokens.expires_at > now() {
            return Ok(tokens.access_token);
        }
        let refresh_token = tokens.refresh_token.as_deref().ok_or_else(|| {
            AccessTokenError::ReauthenticationRequired(
                "Google session expired; reconnect the account".to_string(),
            )
        })?;
        let response = self
            .client
            .post(TOKEN_URL)
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("refresh_token", refresh_token),
                ("grant_type", "refresh_token"),
            ])
            .send()
            .await
            .map_err(|error| AccessTokenError::Transient(error.to_string()))?;
        let response = checked_refresh(response).await?;
        let refreshed: TokenResponse = response
            .json()
            .await
            .map_err(|error| AccessTokenError::Transient(error.to_string()))?;
        tokens.access_token = refreshed.access_token;
        tokens.expires_at = now() + refreshed.expires_in.saturating_sub(60);
        self.save(&tokens).map_err(AccessTokenError::Transient)?;
        Ok(tokens.access_token)
    }

    pub fn disconnect(&self) -> Result<(), String> {
        match self.entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {
                *self.token_cache.lock().unwrap() = None;
                *self.available_cache.lock().unwrap() = Some(false);
                Ok(())
            }
            Err(error) => Err(error.to_string()),
        }
    }

    fn load(&self) -> Result<Tokens, String> {
        let cached = self.token_cache.lock().unwrap().clone();
        if let Some(tokens) = cached {
            return Ok(tokens);
        }
        let value = self.entry()?.get_password().map_err(display)?;
        let tokens: Tokens = serde_json::from_str(&value).map_err(display)?;
        *self.token_cache.lock().unwrap() = Some(tokens.clone());
        Ok(tokens)
    }

    fn save(&self, tokens: &Tokens) -> Result<(), String> {
        let value = serde_json::to_string(tokens).map_err(display)?;
        self.entry()?.set_password(&value).map_err(display)?;
        *self.token_cache.lock().unwrap() = Some(tokens.clone());
        *self.available_cache.lock().unwrap() = Some(true);
        Ok(())
    }

    /// Moves this account's keychain entry to `email`'s key if it isn't
    /// there already. A cheap no-op once already rekeyed.
    fn rekey_to(&self, email: &str) -> Result<(), String> {
        let mut key = self.key.lock().unwrap();
        if *key == email {
            return Ok(());
        }
        if let Ok(old_entry) = Entry::new(SERVICE, key.as_str()) {
            if let Ok(secret) = old_entry.get_password() {
                if let Ok(new_entry) = Entry::new(SERVICE, email) {
                    new_entry.set_password(&secret).map_err(display)?;
                }
                let _ = old_entry.delete_credential();
            }
        }
        *key = email.to_string();
        // The cached tokens/availability were read under the old key; drop
        // them so the next check/load reads through the new one.
        *self.token_cache.lock().unwrap() = None;
        *self.available_cache.lock().unwrap() = None;
        Ok(())
    }

    fn entry(&self) -> Result<Entry, String> {
        Entry::new(SERVICE, &self.key()).map_err(display)
    }
}

async fn fetch_email(client: &Client, access_token: &str) -> Result<String, String> {
    let response = client
        .get(PROFILE_URL)
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(display)?;
    let response = checked(response).await?;
    let profile: Profile = response.json().await.map_err(display)?;
    Ok(profile.email_address)
}

async fn checked(response: reqwest::Response) -> Result<reqwest::Response, String> {
    if response.status().is_success() {
        Ok(response)
    } else {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        Err(format!("Google OAuth returned {status}: {body}"))
    }
}

async fn checked_refresh(
    response: reqwest::Response,
) -> Result<reqwest::Response, AccessTokenError> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    let message = format!("Google OAuth returned {status}: {body}");
    if classify_refresh_failure(&body) == RefreshFailureKind::ReauthenticationRequired {
        Err(AccessTokenError::ReauthenticationRequired(message))
    } else {
        // Endpoint outages, throttling, malformed upstream responses, and
        // configuration errors must not revoke an otherwise valid local
        // account. Only Google's explicit invalid_grant signal proves that
        // this account's refresh grant is permanently unusable.
        Err(AccessTokenError::Transient(message))
    }
}

#[derive(Debug, PartialEq)]
enum RefreshFailureKind {
    ReauthenticationRequired,
    Transient,
}

fn classify_refresh_failure(body: &str) -> RefreshFailureKind {
    if oauth_error_code(body).as_deref() == Some("invalid_grant") {
        RefreshFailureKind::ReauthenticationRequired
    } else {
        RefreshFailureKind::Transient
    }
}

fn oauth_error_code(body: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()?
        .get("error")?
        .as_str()
        .map(str::to_owned)
}

fn build_oauth_client(
    connect_timeout: Duration,
    request_timeout: Duration,
) -> Result<Client, String> {
    Client::builder()
        .connect_timeout(connect_timeout)
        .timeout(request_timeout)
        .build()
        .map_err(display)
}

fn random_urlsafe(bytes: usize) -> String {
    let mut value = vec![0_u8; bytes];
    OsRng.fill_bytes(&mut value);
    URL_SAFE_NO_PAD.encode(value)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn display(error: impl std::fmt::Display) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> GoogleAuthConfig {
        GoogleAuthConfig {
            client: build_oauth_client(OAUTH_CONNECT_TIMEOUT, OAUTH_REQUEST_TIMEOUT).unwrap(),
            client_id: "test-client-id".into(),
            client_secret: "test-client-secret".into(),
        }
    }

    // Deliberately does not exercise the accepting path: that would rekey
    // and touch the OS keychain, which isn't available in a sandboxed test
    // environment. The rejection path never reaches the keychain.
    #[test]
    fn accept_identity_rejects_a_different_already_known_account() {
        let auth = config().account("work@example.com");
        assert!(auth.accept_identity("personal@example.com").is_err());
        assert_eq!(auth.key(), "work@example.com");
    }

    #[test]
    fn accept_identity_is_a_no_op_when_the_identity_already_matches() {
        let auth = config().account("work@example.com");
        assert!(auth.accept_identity("work@example.com").is_ok());
    }

    #[tokio::test]
    async fn oauth_client_times_out_a_stalled_request() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(1)).await;
        });
        let client = build_oauth_client(Duration::from_secs(1), Duration::from_millis(25)).unwrap();

        let error = client
            .get(format!("http://{address}"))
            .send()
            .await
            .unwrap_err();

        assert!(error.is_timeout(), "unexpected request error: {error}");
        server.abort();
    }

    #[test]
    fn only_invalid_grant_is_a_permanent_refresh_failure() {
        assert_eq!(
            classify_refresh_failure(
                r#"{"error":"invalid_grant","error_description":"revoked"}"#
            ),
            RefreshFailureKind::ReauthenticationRequired
        );
        assert_eq!(
            classify_refresh_failure(r#"{"error":"temporarily_unavailable"}"#),
            RefreshFailureKind::Transient
        );
        assert_eq!(
            classify_refresh_failure("<html>upstream failure</html>"),
            RefreshFailureKind::Transient
        );
    }
}
