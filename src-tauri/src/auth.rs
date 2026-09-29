use std::borrow::Cow;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use keyring::Entry;
use oauth2::{
    basic::{BasicClient, BasicErrorResponse},
    AuthType, AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointNotSet,
    EndpointSet, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, RefreshToken, Scope,
    TokenResponse, TokenUrl,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    time::{timeout, Instant},
};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::error_text::display;

const MAIL_SERVICE: &str = "app.threestrands.mail";
const CALENDAR_SERVICE: &str = "app.threestrands.calendar";
const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const PROFILE_URL: &str = "https://gmail.googleapis.com/gmail/v1/users/me/profile";
const USERINFO_URL: &str = "https://openidconnect.googleapis.com/v1/userinfo";
const MAIL_SCOPES: &str = "openid email https://www.googleapis.com/auth/gmail.modify https://www.googleapis.com/auth/gmail.labels";
const CALENDAR_SCOPES: &str = "openid email https://www.googleapis.com/auth/calendar.readonly https://www.googleapis.com/auth/calendar.events";
const OAUTH_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const OAUTH_REQUEST_TIMEOUT: Duration = Duration::from_secs(45);

/// Keychain keys used before an account's real Gmail address is known.
/// `LEGACY_KEY` names a pre-upgrade install's one connected account, still
/// stored under the fixed key used before multi-account support existed.
/// `PENDING_KEY` names an in-progress "add account" flow. Both are replaced
/// by the real address the moment it's learned, via `GoogleAuth::rekey_to`.
pub(crate) const LEGACY_KEY: &str = "default";
const PENDING_KEY: &str = "pending";
/// Returned when a newer sign-in attempt superseded this one.
const CANCELED: &str = "Sign-in was canceled by a newer attempt.";

type GoogleOAuthClient =
    BasicClient<EndpointSet, EndpointNotSet, EndpointNotSet, EndpointNotSet, EndpointSet>;

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
struct Profile {
    #[serde(rename = "emailAddress", alias = "email")]
    email_address: String,
}

/// The OAuth endpoints and scope one authorization flow runs against.
///
/// Separated from [`GoogleAuth`] itself because a single Google Cloud OAuth
/// client already authorizes two different flows under it — full Gmail
/// access and Calendar access — that share `auth_url`/`token_url`
/// but differ in `scopes`/`profile_url`. A future non-Google provider would
/// differ in every field instead.
#[derive(Clone)]
pub struct OAuthEndpoints {
    pub auth_url: String,
    pub token_url: String,
    pub scopes: String,
    pub profile_url: String,
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
        let client_id = std::env::var("THREESTRANDS_GOOGLE_CLIENT_ID")
            .ok()
            .or_else(|| option_env!("THREESTRANDS_GOOGLE_CLIENT_ID").map(str::to_owned))
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                "Google OAuth is not configured. Set THREESTRANDS_GOOGLE_CLIENT_ID to an installed-app client ID."
                    .to_string()
            })?;
        let client_secret = std::env::var("THREESTRANDS_GOOGLE_CLIENT_SECRET")
            .ok()
            .or_else(|| option_env!("THREESTRANDS_GOOGLE_CLIENT_SECRET").map(str::to_owned))
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                "Google OAuth is not configured. Set THREESTRANDS_GOOGLE_CLIENT_SECRET to the value from the Desktop app credential."
                    .to_string()
        })?;
        Ok(Self {
            client: build_http_client(OAUTH_CONNECT_TIMEOUT, OAUTH_REQUEST_TIMEOUT)?,
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
        self.keyed_for(key, MAIL_SERVICE, Self::mail_endpoints())
    }

    fn mail_endpoints() -> OAuthEndpoints {
        OAuthEndpoints {
            auth_url: AUTH_URL.to_string(),
            token_url: TOKEN_URL.to_string(),
            scopes: MAIL_SCOPES.to_string(),
            profile_url: PROFILE_URL.to_string(),
        }
    }

    /// Calendar access is deliberately authorized and stored separately from
    /// Gmail. Connecting it can never broaden an existing mail refresh token.
    pub fn pending_calendar_account(&self) -> GoogleAuth {
        self.keyed_for(PENDING_KEY, CALENDAR_SERVICE, Self::calendar_endpoints())
    }

    pub fn calendar_account(&self, email: &str) -> GoogleAuth {
        self.keyed_for(email, CALENDAR_SERVICE, Self::calendar_endpoints())
    }

    fn calendar_endpoints() -> OAuthEndpoints {
        OAuthEndpoints {
            auth_url: AUTH_URL.to_string(),
            token_url: TOKEN_URL.to_string(),
            scopes: CALENDAR_SCOPES.to_string(),
            profile_url: USERINFO_URL.to_string(),
        }
    }

    fn keyed_for(&self, key: &str, service: &str, endpoints: OAuthEndpoints) -> GoogleAuth {
        let oauth_client = build_oauth_client(&self.client_id, &self.client_secret, &endpoints)
            .expect("static OAuth endpoints must be valid URLs");
        GoogleAuth {
            client: self.client.clone(),
            oauth_client,
            service: service.to_string(),
            endpoints,
            key: Arc::new(Mutex::new(key.to_string())),
            token_cache: Arc::new(Mutex::new(None)),
            available_cache: Arc::new(Mutex::new(None)),
        }
    }
}

#[derive(Clone)]
pub struct GoogleAuth {
    client: Client,
    oauth_client: GoogleOAuthClient,
    service: String,
    endpoints: OAuthEndpoints,
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
        let email = fetch_email(
            &self.client,
            &self.endpoints.profile_url,
            &tokens.access_token,
        )
        .await?;
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
        let redirect_uri = RedirectUrl::new(redirect_uri).map_err(display)?;
        let (authorization_url, state, pkce_verifier) = authorization_request(
            &self.oauth_client,
            &self.endpoints.scopes,
            redirect_uri.clone(),
        );
        open::that(authorization_url.as_str()).map_err(display)?;

        let (mut stream, target) = self.accept_callback(&listener, cancel).await?;
        let result = match callback_code(&target, &state) {
            Ok(code) => self.exchange_code(&code, pkce_verifier, redirect_uri).await,
            Err(error) => Err(error),
        };
        let (status, body) = if result.is_ok() {
            (
                "200 OK",
                "ThreeStrands is connected. You can close this tab.",
            )
        } else {
            (
                "400 Bad Request",
                "ThreeStrands could not complete sign-in. Return to the app.",
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
        verifier: PkceCodeVerifier,
        redirect_uri: RedirectUrl,
    ) -> Result<Tokens, String> {
        let token = self
            .oauth_client
            .exchange_code(AuthorizationCode::new(code.to_string()))
            .set_pkce_verifier(verifier)
            .set_redirect_uri(Cow::Owned(redirect_uri))
            .request_async(&self.client)
            .await
            .map_err(|error| format!("Google OAuth code exchange failed: {error}"))?;
        tokens_from_response(&token)
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
        let refreshed = self
            .oauth_client
            .exchange_refresh_token(&RefreshToken::new(refresh_token.to_string()))
            .request_async(&self.client)
            .await
            .map_err(map_refresh_error)?;
        let refreshed = tokens_from_response(&refreshed).map_err(AccessTokenError::Transient)?;
        tokens.access_token = refreshed.access_token;
        if refreshed.refresh_token.is_some() {
            tokens.refresh_token = refreshed.refresh_token;
        }
        tokens.expires_at = refreshed.expires_at;
        self.save(&tokens).map_err(AccessTokenError::Transient)?;
        Ok(tokens.access_token)
    }

    /// Marks `rejected` expired so the next [`Self::access_token`] refreshes
    /// it. A no-op once another request has already replaced it, so several
    /// concurrent rejections of one stale token cost a single refresh.
    pub fn expire_access_token(&self, rejected: &str) {
        if let Some(tokens) = self.token_cache.lock().unwrap().as_mut() {
            if tokens.access_token == rejected {
                tokens.expires_at = 0;
            }
        }
    }

    /// An account whose tokens live only in memory, refreshing against
    /// `token_url`, so tests never reach the OS keychain.
    #[cfg(test)]
    pub(crate) fn in_memory_for_test(token_url: &str, tokens: Tokens) -> Self {
        let mut endpoints = GoogleAuthConfig::mail_endpoints();
        endpoints.token_url = token_url.to_string();
        let auth = GoogleAuthConfig {
            client: build_http_client(OAUTH_CONNECT_TIMEOUT, OAUTH_REQUEST_TIMEOUT).unwrap(),
            client_id: "test-client-id".into(),
            client_secret: "test-client-secret".into(),
        }
        .keyed_for("test@example.com", MAIL_SERVICE, endpoints);
        *auth.token_cache.lock().unwrap() = Some(tokens);
        auth
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
        let crate::credentials::StoredCredential::GoogleOAuth(tokens) =
            crate::credentials::StoredCredential::decode(&value)?;
        *self.token_cache.lock().unwrap() = Some(tokens.clone());
        Ok(tokens)
    }

    fn save(&self, tokens: &Tokens) -> Result<(), String> {
        let value = crate::credentials::StoredCredential::GoogleOAuth(tokens.clone()).encode()?;
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
        if let Ok(old_entry) = Entry::new(&self.service, key.as_str()) {
            if let Ok(secret) = old_entry.get_password() {
                if let Ok(new_entry) = Entry::new(&self.service, email) {
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
        Entry::new(&self.service, &self.key()).map_err(display)
    }
}

async fn fetch_email(
    client: &Client,
    profile_url: &str,
    access_token: &str,
) -> Result<String, String> {
    let response = client
        .get(profile_url)
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

fn map_refresh_error<E>(error: oauth2::RequestTokenError<E, BasicErrorResponse>) -> AccessTokenError
where
    E: std::error::Error + 'static,
{
    if let oauth2::RequestTokenError::ServerResponse(response) = &error {
        if response.error().as_ref() == "invalid_grant" {
            return AccessTokenError::ReauthenticationRequired(format!(
                "Google OAuth returned error: {response}"
            ));
        }
    }
    // Endpoint outages, throttling, malformed upstream responses, and
    // configuration errors must not revoke an otherwise valid local
    // account. Only Google's explicit invalid_grant signal proves that
    // this account's refresh grant is permanently unusable.
    AccessTokenError::Transient(format!("Google OAuth refresh failed: {error}"))
}

fn tokens_from_response(response: &impl TokenResponse) -> Result<Tokens, String> {
    let expires_in = response
        .expires_in()
        .ok_or_else(|| "Google OAuth token response omitted expires_in".to_string())?
        .as_secs();
    Ok(Tokens {
        access_token: response.access_token().secret().to_string(),
        refresh_token: response
            .refresh_token()
            .map(|token| token.secret().to_string()),
        expires_at: now() + expires_in.saturating_sub(60),
    })
}

fn authorization_request(
    client: &GoogleOAuthClient,
    scopes: &str,
    redirect_uri: RedirectUrl,
) -> (Url, CsrfToken, PkceCodeVerifier) {
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let mut request = client
        .authorize_url(CsrfToken::new_random)
        .set_pkce_challenge(challenge)
        .set_redirect_uri(Cow::Owned(redirect_uri));
    for scope in scopes.split_ascii_whitespace() {
        request = request.add_scope(Scope::new(scope.to_string()));
    }
    let (url, state) = request
        .add_extra_param("access_type", "offline")
        .add_extra_param("prompt", "consent")
        .url();
    (url, state, verifier)
}

fn callback_code(target: &str, expected_state: &CsrfToken) -> Result<String, String> {
    let callback = Url::parse(&format!("http://localhost{target}")).map_err(display)?;
    let values = callback
        .query_pairs()
        .collect::<std::collections::HashMap<_, _>>();
    if values.get("state").map(|value| value.as_ref()) != Some(expected_state.secret()) {
        return Err("OAuth state did not match; sign-in was rejected".to_string());
    }
    if let Some(error) = values.get("error") {
        return Err(format!("Google authorization failed: {error}"));
    }
    values
        .get("code")
        .map(|code| code.to_string())
        .ok_or_else(|| "OAuth callback did not contain a code".to_string())
}

fn build_oauth_client(
    client_id: &str,
    client_secret: &str,
    endpoints: &OAuthEndpoints,
) -> Result<GoogleOAuthClient, String> {
    let auth_url = AuthUrl::new(endpoints.auth_url.clone()).map_err(display)?;
    let token_url = TokenUrl::new(endpoints.token_url.clone()).map_err(display)?;
    Ok(BasicClient::new(ClientId::new(client_id.to_string()))
        .set_client_secret(ClientSecret::new(client_secret.to_string()))
        .set_auth_type(AuthType::RequestBody)
        .set_auth_uri(auth_url)
        .set_token_uri(token_url))
}

fn build_http_client(
    connect_timeout: Duration,
    request_timeout: Duration,
) -> Result<Client, String> {
    Client::builder()
        .connect_timeout(connect_timeout)
        .timeout(request_timeout)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(display)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        extract::{Form, State},
        routing::post,
        Json, Router,
    };
    use serde_json::{json, Value};
    use std::collections::HashMap;

    fn config() -> GoogleAuthConfig {
        GoogleAuthConfig {
            client: build_http_client(OAUTH_CONNECT_TIMEOUT, OAUTH_REQUEST_TIMEOUT).unwrap(),
            client_id: "test-client-id".into(),
            client_secret: "test-client-secret".into(),
        }
    }

    async fn capture_token_request(
        State(requests): State<Arc<Mutex<Vec<HashMap<String, String>>>>>,
        Form(form): Form<HashMap<String, String>>,
    ) -> Json<Value> {
        requests.lock().unwrap().push(form);
        Json(json!({
            "access_token": "exchanged-access",
            "refresh_token": "exchanged-refresh",
            "token_type": "Bearer",
            "expires_in": 3600
        }))
    }

    async fn error_token_response(
        State((status, code)): State<(axum::http::StatusCode, String)>,
    ) -> (axum::http::StatusCode, Json<Value>) {
        (status, Json(json!({ "error": code })))
    }

    async fn start_token_server(app: Router) -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{address}/token"), task)
    }

    fn auth_with_token_url(token_url: &str) -> GoogleAuth {
        let mut endpoints = GoogleAuthConfig::mail_endpoints();
        endpoints.token_url = token_url.to_string();
        GoogleAuthConfig {
            client: build_http_client(OAUTH_CONNECT_TIMEOUT, OAUTH_REQUEST_TIMEOUT).unwrap(),
            client_id: "test-client-id".into(),
            client_secret: "test-client-secret".into(),
        }
        .keyed_for("test@example.com", MAIL_SERVICE, endpoints)
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

    #[test]
    fn authorization_request_uses_pkce_scopes_and_google_consent_parameters() {
        let auth = config().account("work@example.com");
        let redirect = RedirectUrl::new("http://127.0.0.1:43210/oauth/callback".into()).unwrap();
        let (url, state, verifier) =
            authorization_request(&auth.oauth_client, &auth.endpoints.scopes, redirect);
        let query = url.query_pairs().collect::<HashMap<_, _>>();

        assert_eq!(query.get("response_type").map(|v| v.as_ref()), Some("code"));
        assert_eq!(
            query.get("client_id").map(|v| v.as_ref()),
            Some("test-client-id")
        );
        assert_eq!(
            query.get("state").map(|v| v.as_ref()),
            Some(state.secret().as_str())
        );
        assert_eq!(
            query.get("code_challenge_method").map(|v| v.as_ref()),
            Some("S256")
        );
        assert!(!query.get("code_challenge").unwrap().is_empty());
        assert_eq!(
            query.get("access_type").map(|v| v.as_ref()),
            Some("offline")
        );
        assert_eq!(query.get("prompt").map(|v| v.as_ref()), Some("consent"));
        assert_eq!(query.get("scope").map(|v| v.as_ref()), Some(MAIL_SCOPES));
        assert!(verifier.secret().len() >= 43);
    }

    #[test]
    fn callback_requires_matching_state_and_an_authorization_code() {
        let expected = CsrfToken::new("expected-state".into());
        assert_eq!(
            callback_code("/oauth/callback?state=expected-state&code=abc", &expected).unwrap(),
            "abc"
        );
        assert!(
            callback_code("/oauth/callback?state=wrong&code=abc", &expected)
                .unwrap_err()
                .contains("state did not match")
        );
        assert!(callback_code(
            "/oauth/callback?state=expected-state&error=access_denied",
            &expected
        )
        .unwrap_err()
        .contains("access_denied"));
        assert!(
            callback_code("/oauth/callback?state=expected-state", &expected)
                .unwrap_err()
                .contains("did not contain a code")
        );
    }

    #[tokio::test]
    async fn oauth2_exchanges_code_with_pkce_redirect_and_parses_tokens() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route("/token", post(capture_token_request))
            .with_state(requests.clone());
        let (token_url, server) = start_token_server(app).await;
        let auth = auth_with_token_url(&token_url);
        let redirect = RedirectUrl::new("http://127.0.0.1:43210/oauth/callback".into()).unwrap();

        let tokens = auth
            .exchange_code(
                "authorization-code",
                PkceCodeVerifier::new("test-verifier-value".into()),
                redirect,
            )
            .await
            .unwrap();

        let requests = requests.lock().unwrap();
        let request = &requests[0];
        assert_eq!(
            request.get("grant_type").map(String::as_str),
            Some("authorization_code")
        );
        assert_eq!(
            request.get("code").map(String::as_str),
            Some("authorization-code")
        );
        assert_eq!(
            request.get("code_verifier").map(String::as_str),
            Some("test-verifier-value")
        );
        assert_eq!(
            request.get("redirect_uri").map(String::as_str),
            Some("http://127.0.0.1:43210/oauth/callback")
        );
        assert_eq!(
            request.get("client_id").map(String::as_str),
            Some("test-client-id")
        );
        assert_eq!(tokens.access_token, "exchanged-access");
        assert_eq!(tokens.refresh_token.as_deref(), Some("exchanged-refresh"));
        assert!(tokens.expires_at > now());
        server.abort();
    }

    #[tokio::test]
    async fn refresh_errors_preserve_reauthentication_vs_transient_classification() {
        for (status, error, expects_reauth) in [
            (axum::http::StatusCode::BAD_REQUEST, "invalid_grant", true),
            (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "temporarily_unavailable",
                false,
            ),
        ] {
            let app = Router::new()
                .route("/token", post(error_token_response))
                .with_state((status, error.to_string()));
            let (token_url, server) = start_token_server(app).await;
            let auth = GoogleAuth::in_memory_for_test(
                &token_url,
                Tokens {
                    access_token: "expired-access".into(),
                    refresh_token: Some("refresh-secret".into()),
                    expires_at: 0,
                },
            );

            let result = auth.access_token().await;
            assert!(
                matches!(result, Err(AccessTokenError::ReauthenticationRequired(_)))
                    == expects_reauth,
                "unexpected refresh classification: {result:?}"
            );
            server.abort();
        }
    }

    #[tokio::test]
    async fn oauth_client_times_out_a_stalled_request() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(1)).await;
        });
        let client = build_http_client(Duration::from_secs(1), Duration::from_millis(25)).unwrap();

        let error = client
            .get(format!("http://{address}"))
            .send()
            .await
            .unwrap_err();

        assert!(error.is_timeout(), "unexpected request error: {error}");
        server.abort();
    }

    #[test]
    fn expiring_a_rejected_access_token_spares_its_replacement() {
        let auth = GoogleAuth::in_memory_for_test(
            TOKEN_URL,
            Tokens {
                access_token: "current".into(),
                refresh_token: Some("refresh".into()),
                expires_at: u64::MAX,
            },
        );

        auth.expire_access_token("already-replaced");
        assert_eq!(auth.load().unwrap().expires_at, u64::MAX);

        auth.expire_access_token("current");
        assert_eq!(auth.load().unwrap().expires_at, 0);
        assert_eq!(auth.load().unwrap().access_token, "current");
    }

    #[test]
    fn only_invalid_grant_is_a_permanent_refresh_failure() {
        let invalid_grant = oauth2::RequestTokenError::<std::io::Error, _>::ServerResponse(
            BasicErrorResponse::new(
                oauth2::basic::BasicErrorResponseType::InvalidGrant,
                Some("revoked".to_string()),
                None,
            ),
        );
        assert!(matches!(
            map_refresh_error(invalid_grant),
            AccessTokenError::ReauthenticationRequired(_)
        ));

        let temporary = oauth2::RequestTokenError::<std::io::Error, _>::ServerResponse(
            BasicErrorResponse::new(
                oauth2::basic::BasicErrorResponseType::Extension(
                    "temporarily_unavailable".to_string(),
                ),
                None,
                None,
            ),
        );
        assert!(matches!(
            map_refresh_error(temporary),
            AccessTokenError::Transient(_)
        ));
    }

    #[test]
    fn calendar_authorization_can_create_events_and_is_separate_from_mail() {
        let config = config();
        let mail = config.account("work@example.com");
        let calendar = config.calendar_account("work@example.com");

        assert_eq!(mail.service, MAIL_SERVICE);
        assert!(mail.endpoints.scopes.contains("gmail.modify"));
        assert_eq!(calendar.service, CALENDAR_SERVICE);
        assert!(calendar.endpoints.scopes.contains("calendar.readonly"));
        assert!(calendar.endpoints.scopes.contains("calendar.events"));
        assert!(!calendar.endpoints.scopes.contains("gmail."));
    }

    // `.available()`/`.authorize()`/`.disconnect()` touch the OS keychain or
    // network and aren't exercised here for the same reason
    // `accept_identity_rejects_a_different_already_known_account` isn't —
    // see its comment. `.key()` and `.provider()` are pure, so they cover
    // that `AccountAuth` actually dispatches to its wrapped credential
    // rather than, say, always returning a fresh unkeyed one.
    #[test]
    fn account_auth_key_and_provider_delegate_to_the_wrapped_credential() {
        let auth = AccountAuth::Google(config().account("work@example.com"));
        assert_eq!(auth.key(), "work@example.com");
        assert!(auth.provider().capabilities().server_search);
    }
}

/// An account's credential handle, abstracting over which provider it
/// authenticates through. `SyncService`, `ConnectedAccount`, and
/// `Correspondence` hold this rather than a concrete credential type, so
/// none of them has to change shape when a second provider exists — they
/// gain a variant here and a new arm in `provider()` instead.
///
/// Every method below is a thin dispatch to the wrapped credential; this
/// type carries no state of its own.
#[derive(Clone)]
pub enum AccountAuth {
    Google(GoogleAuth),
}

impl AccountAuth {
    /// This account's id, once known — see the equivalent method on the
    /// wrapped credential for what that means before then.
    pub fn key(&self) -> String {
        match self {
            Self::Google(auth) => auth.key(),
        }
    }

    pub fn available(&self) -> bool {
        match self {
            Self::Google(auth) => auth.available(),
        }
    }

    pub fn disconnect(&self) -> Result<(), String> {
        match self {
            Self::Google(auth) => auth.disconnect(),
        }
    }

    pub fn accept_identity(&self, email: &str) -> Result<(), String> {
        match self {
            Self::Google(auth) => auth.accept_identity(email),
        }
    }

    pub async fn authorize(&self, cancel: &CancellationToken) -> Result<String, String> {
        match self {
            Self::Google(auth) => auth.authorize(cancel).await,
        }
    }

    /// The mail backend this credential authorizes access to.
    pub fn provider(&self) -> std::sync::Arc<dyn crate::provider::MailProvider> {
        match self {
            Self::Google(auth) => {
                std::sync::Arc::new(crate::provider::gmail::GmailClient::new(auth.clone()))
            }
        }
    }
}
