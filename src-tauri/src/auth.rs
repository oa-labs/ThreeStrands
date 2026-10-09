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

use crate::{credentials::StoredCredential, error_text::display, models::MailProviderKind};

const MAIL_SERVICE: &str = "app.threestrands.mail";
const CALENDAR_SERVICE: &str = "app.threestrands.calendar";
const GOOGLE_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const GMAIL_PROFILE_URL: &str = "https://gmail.googleapis.com/gmail/v1/users/me/profile";
const GOOGLE_USERINFO_URL: &str = "https://openidconnect.googleapis.com/v1/userinfo";
const GMAIL_SCOPES: &str = "openid email https://www.googleapis.com/auth/gmail.modify https://www.googleapis.com/auth/gmail.labels";
const GOOGLE_CALENDAR_SCOPES: &str = "openid email https://www.googleapis.com/auth/calendar.readonly https://www.googleapis.com/auth/calendar.events";
const OAUTH_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const OAUTH_REQUEST_TIMEOUT: Duration = Duration::from_secs(45);

/// Keychain keys used before an account's real address is known.
/// `LEGACY_KEY` names a pre-upgrade install's one connected account, still
/// stored under the fixed key used before multi-account support existed.
/// `PENDING_KEY` names an in-progress "add account" flow. Both are replaced
/// by the real address the moment it's learned, via `OAuthCredential::rekey_to`.
pub(crate) const LEGACY_KEY: &str = "default";
const PENDING_KEY: &str = "pending";
/// Returned when a newer sign-in attempt superseded this one.
const CANCELED: &str = "Sign-in was canceled by a newer attempt.";

type OAuthClient =
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
/// Separated from [`OAuthCredential`] itself because a single OAuth app
/// already authorizes two different flows under it — full mail access and
/// calendar access — that share `auth_url`/`token_url` but differ in
/// `scopes`/`profile_url`. Another identity service differs in every field.
#[derive(Clone)]
pub struct OAuthEndpoints {
    pub auth_url: String,
    pub token_url: String,
    pub scopes: String,
    pub profile_url: String,
}

/// The identity service an [`OAuthCredential`] signs in through.
///
/// Everything about the interactive flow — PKCE, the loopback redirect,
/// refresh, and keychain storage — is shared by every service; this type
/// describes only what differs between them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OAuthProvider {
    Google,
}

impl OAuthProvider {
    /// How this service is named in user-facing errors.
    fn label(self) -> &'static str {
        match self {
            Self::Google => "Google",
        }
    }

    /// Why no app is configured for this service, phrased for whoever
    /// builds or launches the app.
    pub fn not_configured(self) -> String {
        match self {
            Self::Google => "Google OAuth is not configured. Set THREESTRANDS_GOOGLE_CLIENT_ID and \
                 THREESTRANDS_GOOGLE_CLIENT_SECRET from a Desktop app credential."
                .into(),
        }
    }

    fn mail_endpoints(self) -> OAuthEndpoints {
        match self {
            Self::Google => OAuthEndpoints {
                auth_url: GOOGLE_AUTH_URL.to_string(),
                token_url: GOOGLE_TOKEN_URL.to_string(),
                scopes: GMAIL_SCOPES.to_string(),
                profile_url: GMAIL_PROFILE_URL.to_string(),
            },
        }
    }

    fn calendar_endpoints(self) -> OAuthEndpoints {
        match self {
            Self::Google => OAuthEndpoints {
                auth_url: GOOGLE_AUTH_URL.to_string(),
                token_url: GOOGLE_TOKEN_URL.to_string(),
                scopes: GOOGLE_CALENDAR_SCOPES.to_string(),
                profile_url: GOOGLE_USERINFO_URL.to_string(),
            },
        }
    }

    /// Authorization-URL parameters beyond the standard PKCE request.
    /// Google only issues a refresh token for `access_type=offline`, and
    /// only re-issues one on reconnect when `prompt=consent` forces it.
    fn extra_auth_params(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::Google => &[("access_type", "offline"), ("prompt", "consent")],
        }
    }

    fn wrap(self, tokens: Tokens) -> StoredCredential {
        match self {
            Self::Google => StoredCredential::GoogleOAuth(tokens),
        }
    }

    /// Unwraps a stored credential, which must have been written for this
    /// service: a token minted by one identity service is never presented
    /// to another, and a non-OAuth credential (e.g. an IMAP password) is
    /// never presented to an OAuth provider.
    fn unwrap(self, credential: StoredCredential) -> Result<Tokens, String> {
        match (self, credential) {
            (Self::Google, StoredCredential::GoogleOAuth(tokens)) => Ok(tokens),
            (Self::Google, StoredCredential::ImapPassword(_)) => Err(format!(
                "{} expected an OAuth token but found an IMAP password credential",
                self.label()
            )),
        }
    }
}

/// One identity service's shared OAuth app registration. A single app is
/// used for every account connected through that service; only the token
/// and consent are per-account, so this is what constructs an
/// [`OAuthCredential`] for each one.
#[derive(Clone)]
pub struct OAuthApp {
    provider: OAuthProvider,
    client: Client,
    client_id: String,
    /// `None` for a public client that authenticates with PKCE alone.
    client_secret: Option<String>,
}

impl OAuthApp {
    pub fn google_from_environment() -> Result<Self, String> {
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
            provider: OAuthProvider::Google,
            client: build_http_client(OAUTH_CONNECT_TIMEOUT, OAUTH_REQUEST_TIMEOUT)?,
            client_id,
            client_secret: Some(client_secret),
        })
    }

    /// A mail credential stored under `key`: an account's real address, or
    /// a placeholder (`LEGACY_KEY`/`PENDING_KEY`) that `authorize()` rekeys
    /// to the real address once sign-in completes.
    fn mail_credential(&self, key: &str) -> OAuthCredential {
        self.keyed_for(key, MAIL_SERVICE, self.provider.mail_endpoints())
    }

    /// Calendar access is deliberately authorized and stored separately from
    /// mail. Connecting it can never broaden an existing mail refresh token.
    pub fn pending_calendar_account(&self) -> OAuthCredential {
        self.keyed_for(PENDING_KEY, CALENDAR_SERVICE, self.provider.calendar_endpoints())
    }

    pub fn calendar_account(&self, email: &str) -> OAuthCredential {
        self.keyed_for(email, CALENDAR_SERVICE, self.provider.calendar_endpoints())
    }

    fn keyed_for(&self, key: &str, service: &str, endpoints: OAuthEndpoints) -> OAuthCredential {
        let oauth_client = build_oauth_client(
            &self.client_id,
            self.client_secret.as_deref(),
            &endpoints,
        )
        .expect("static OAuth endpoints must be valid URLs");
        OAuthCredential {
            provider: self.provider,
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

/// Every OAuth app this build is configured with. A service whose app isn't
/// configured simply can't be connected; the others are unaffected.
#[derive(Clone, Default)]
pub struct AuthConfig {
    google: Option<OAuthApp>,
}

impl AuthConfig {
    pub fn from_environment() -> Self {
        Self {
            google: OAuthApp::google_from_environment().ok(),
        }
    }

    /// A config with a Google app whose credentials only ever reach test
    /// stand-ins.
    #[cfg(test)]
    pub(crate) fn google_for_test() -> Self {
        Self {
            google: Some(OAuthApp {
                provider: OAuthProvider::Google,
                client: build_http_client(OAUTH_CONNECT_TIMEOUT, OAUTH_REQUEST_TIMEOUT).unwrap(),
                client_id: "test-client-id".into(),
                client_secret: Some("test-client-secret".into()),
            }),
        }
    }

    /// The Google app, which authorizes both Gmail and Google Calendar.
    pub fn google(&self) -> Result<&OAuthApp, String> {
        self.google
            .as_ref()
            .ok_or_else(|| OAuthProvider::Google.not_configured())
    }

    /// The identity service a mail provider signs in through. Only OAuth
    /// providers have one; an IMAP account authenticates with a stored
    /// password and never reaches this.
    fn mail_app(&self, provider: MailProviderKind) -> Result<&OAuthApp, String> {
        match provider {
            MailProviderKind::Gmail => self.google(),
            MailProviderKind::Imap => Err(
                "IMAP accounts authenticate with a stored password, not an OAuth app".to_string(),
            ),
        }
    }

    /// The credential for a mail account stored under `key` — its real
    /// address, or the `LEGACY_KEY` placeholder.
    pub fn mail_account(&self, provider: MailProviderKind, key: &str) -> Result<AccountAuth, String> {
        Ok(match provider {
            MailProviderKind::Gmail => {
                AccountAuth::Gmail(self.mail_app(provider)?.mail_credential(key))
            }
            MailProviderKind::Imap => AccountAuth::Imap(ImapCredential::for_account(key)),
        })
    }

    /// A fresh credential for an in-progress "add account" flow.
    pub fn pending_mail_account(&self, provider: MailProviderKind) -> Result<AccountAuth, String> {
        self.mail_account(provider, PENDING_KEY)
    }
}

/// One account's OAuth credential for one service (mail or calendar),
/// persisted in the OS keychain under the account's address.
#[derive(Clone)]
pub struct OAuthCredential {
    provider: OAuthProvider,
    client: Client,
    oauth_client: OAuthClient,
    service: String,
    endpoints: OAuthEndpoints,
    key: Arc<Mutex<String>>,
    token_cache: Arc<Mutex<Option<Tokens>>>,
    available_cache: Arc<Mutex<Option<bool>>>,
}

impl OAuthCredential {
    /// The address this instance represents, once known; otherwise the
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

    /// Runs the interactive PKCE flow and learns which address just
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
            self.provider,
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
            self.provider,
            &self.endpoints.scopes,
            redirect_uri.clone(),
        );
        open::that(authorization_url.as_str()).map_err(display)?;

        let (mut stream, target) = self.accept_callback(&listener, cancel).await?;
        let result = match callback_code(&target, &state, self.provider) {
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
            .request_async(&OAuthHttp(self.client.clone()))
            .await
            .map_err(|error| {
                format!("{} OAuth code exchange failed: {error}", self.provider.label())
            })?;
        tokens_from_response(&token, self.provider)
    }

    pub async fn access_token(&self) -> Result<String, AccessTokenError> {
        let mut tokens = self
            .load()
            .map_err(AccessTokenError::ReauthenticationRequired)?;
        if tokens.expires_at > now() {
            return Ok(tokens.access_token);
        }
        let refresh_token = tokens.refresh_token.as_deref().ok_or_else(|| {
            AccessTokenError::ReauthenticationRequired(format!(
                "{} session expired; reconnect the account",
                self.provider.label()
            ))
        })?;
        let refreshed = self
            .oauth_client
            .exchange_refresh_token(&RefreshToken::new(refresh_token.to_string()))
            .request_async(&OAuthHttp(self.client.clone()))
            .await
            .map_err(|error| map_refresh_error(error, self.provider))?;
        let refreshed = tokens_from_response(&refreshed, self.provider)
            .map_err(AccessTokenError::Transient)?;
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
        let mut endpoints = OAuthProvider::Google.mail_endpoints();
        endpoints.token_url = token_url.to_string();
        let auth = OAuthApp {
            provider: OAuthProvider::Google,
            client: build_http_client(OAUTH_CONNECT_TIMEOUT, OAUTH_REQUEST_TIMEOUT).unwrap(),
            client_id: "test-client-id".into(),
            client_secret: Some("test-client-secret".into()),
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
        let tokens = self.provider.unwrap(StoredCredential::decode(&value)?)?;
        *self.token_cache.lock().unwrap() = Some(tokens.clone());
        Ok(tokens)
    }

    fn save(&self, tokens: &Tokens) -> Result<(), String> {
        let value = self.provider.wrap(tokens.clone()).encode()?;
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
    provider: OAuthProvider,
    profile_url: &str,
    access_token: &str,
) -> Result<String, String> {
    let response = client
        .get(profile_url)
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(display)?;
    let response = checked(response, provider).await?;
    let profile: Profile = response.json().await.map_err(display)?;
    Ok(profile.email_address)
}

async fn checked(
    response: reqwest::Response,
    provider: OAuthProvider,
) -> Result<reqwest::Response, String> {
    if response.status().is_success() {
        Ok(response)
    } else {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        Err(format!("{} OAuth returned {status}: {body}", provider.label()))
    }
}

fn map_refresh_error<E>(
    error: oauth2::RequestTokenError<E, BasicErrorResponse>,
    provider: OAuthProvider,
) -> AccessTokenError
where
    E: std::error::Error + 'static,
{
    if let oauth2::RequestTokenError::ServerResponse(response) = &error {
        if response.error().as_ref() == "invalid_grant" {
            return AccessTokenError::ReauthenticationRequired(format!(
                "{} OAuth returned error: {response}",
                provider.label()
            ));
        }
    }
    // Endpoint outages, throttling, malformed upstream responses, and
    // configuration errors must not revoke an otherwise valid local
    // account. Only the standard invalid_grant signal (RFC 6749 §5.2)
    // proves that this account's refresh grant is permanently unusable.
    AccessTokenError::Transient(format!(
        "{} OAuth refresh failed: {error}",
        provider.label()
    ))
}

fn tokens_from_response(
    response: &impl TokenResponse,
    provider: OAuthProvider,
) -> Result<Tokens, String> {
    let expires_in = response
        .expires_in()
        .ok_or_else(|| {
            format!(
                "{} OAuth token response omitted expires_in",
                provider.label()
            )
        })?
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
    client: &OAuthClient,
    provider: OAuthProvider,
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
    for (name, value) in provider.extra_auth_params() {
        request = request.add_extra_param(*name, *value);
    }
    let (url, state) = request.url();
    (url, state, verifier)
}

fn callback_code(
    target: &str,
    expected_state: &CsrfToken,
    provider: OAuthProvider,
) -> Result<String, String> {
    let callback = Url::parse(&format!("http://localhost{target}")).map_err(display)?;
    let values = callback
        .query_pairs()
        .collect::<std::collections::HashMap<_, _>>();
    if values.get("state").map(|value| value.as_ref()) != Some(expected_state.secret()) {
        return Err("OAuth state did not match; sign-in was rejected".to_string());
    }
    if let Some(error) = values.get("error") {
        return Err(format!("{} authorization failed: {error}", provider.label()));
    }
    values
        .get("code")
        .map(|code| code.to_string())
        .ok_or_else(|| "OAuth callback did not contain a code".to_string())
}

fn build_oauth_client(
    client_id: &str,
    client_secret: Option<&str>,
    endpoints: &OAuthEndpoints,
) -> Result<OAuthClient, String> {
    let auth_url = AuthUrl::new(endpoints.auth_url.clone()).map_err(display)?;
    let token_url = TokenUrl::new(endpoints.token_url.clone()).map_err(display)?;
    let client = BasicClient::new(ClientId::new(client_id.to_string()));
    let client = match client_secret {
        Some(secret) => client.set_client_secret(ClientSecret::new(secret.to_string())),
        None => client,
    };
    Ok(client
        .set_auth_type(AuthType::RequestBody)
        .set_auth_uri(auth_url)
        .set_token_uri(token_url))
}

/// oauth2's HTTP client seam over the app's own `reqwest` client. A named
/// type returning a boxed `Send` future (rather than a closure) keeps token
/// requests usable from Tauri's `Send` command futures.
struct OAuthHttp(Client);

impl<'c> oauth2::AsyncHttpClient<'c> for OAuthHttp {
    type Error = oauth2::HttpClientError<reqwest::Error>;
    type Future = std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<oauth2::HttpResponse, Self::Error>> + Send + 'c,
        >,
    >;

    fn call(&'c self, request: oauth2::HttpRequest) -> Self::Future {
        // `reqwest::Client` is a cheap handle; owning it keeps the future
        // free of borrows.
        let client = self.0.clone();
        Box::pin(async move { send_oauth_request(&client, request).await })
    }
}

/// Sends one oauth2 token request, mirroring the adapter oauth2 ships for
/// the reqwest version it pins.
async fn send_oauth_request(
    client: &Client,
    request: oauth2::HttpRequest,
) -> Result<oauth2::HttpResponse, oauth2::HttpClientError<reqwest::Error>> {
    let response = client
        .execute(request.try_into().map_err(Box::new)?)
        .await
        .map_err(Box::new)?;
    let mut builder = oauth2::http::Response::builder()
        .status(response.status())
        .version(response.version());
    for (name, value) in response.headers() {
        builder = builder.header(name, value);
    }
    builder
        .body(response.bytes().await.map_err(Box::new)?.to_vec())
        .map_err(oauth2::HttpClientError::Http)
}

fn build_http_client(
    connect_timeout: Duration,
    request_timeout: Duration,
) -> Result<Client, String> {
    crate::http_client::builder()
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

    fn google_app() -> OAuthApp {
        config().google().unwrap().clone()
    }

    fn config() -> AuthConfig {
        AuthConfig::google_for_test()
    }

    fn gmail_credential(key: &str) -> OAuthCredential {
        google_app().mail_credential(key)
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

    fn auth_with_token_url(token_url: &str) -> OAuthCredential {
        let mut endpoints = OAuthProvider::Google.mail_endpoints();
        endpoints.token_url = token_url.to_string();
        google_app().keyed_for("test@example.com", MAIL_SERVICE, endpoints)
    }

    // Deliberately does not exercise the accepting path: that would rekey
    // and touch the OS keychain, which isn't available in a sandboxed test
    // environment. The rejection path never reaches the keychain.
    #[test]
    fn accept_identity_rejects_a_different_already_known_account() {
        let auth = gmail_credential("work@example.com");
        assert!(auth.accept_identity("personal@example.com").is_err());
        assert_eq!(auth.key(), "work@example.com");
    }

    #[test]
    fn accept_identity_is_a_no_op_when_the_identity_already_matches() {
        let auth = gmail_credential("work@example.com");
        assert!(auth.accept_identity("work@example.com").is_ok());
    }

    #[test]
    fn authorization_request_uses_pkce_scopes_and_google_consent_parameters() {
        let auth = gmail_credential("work@example.com");
        let redirect = RedirectUrl::new("http://127.0.0.1:43210/oauth/callback".into()).unwrap();
        let (url, state, verifier) = authorization_request(
            &auth.oauth_client,
            auth.provider,
            &auth.endpoints.scopes,
            redirect,
        );
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
        assert_eq!(query.get("scope").map(|v| v.as_ref()), Some(GMAIL_SCOPES));
        assert!(verifier.secret().len() >= 43);
    }

    #[test]
    fn callback_requires_matching_state_and_an_authorization_code() {
        let expected = CsrfToken::new("expected-state".into());
        let google = OAuthProvider::Google;
        assert_eq!(
            callback_code("/oauth/callback?state=expected-state&code=abc", &expected, google).unwrap(),
            "abc"
        );
        assert!(
            callback_code("/oauth/callback?state=wrong&code=abc", &expected, google)
                .unwrap_err()
                .contains("state did not match")
        );
        assert!(callback_code(
            "/oauth/callback?state=expected-state&error=access_denied",
            &expected,
            google
        )
        .unwrap_err()
        .contains("access_denied"));
        assert!(
            callback_code("/oauth/callback?state=expected-state", &expected, google)
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
            let auth = OAuthCredential::in_memory_for_test(
                &token_url,
                Tokens {
                    access_token: "expired-access".into(),
                    refresh_token: Some("refresh-secret".into()),
                    expires_at: 0,
                },
            );

            let result = auth.access_token().await;
            if expects_reauth {
                assert!(
                    matches!(result, Err(AccessTokenError::ReauthenticationRequired(_))),
                    "unexpected refresh classification: {result:?}"
                );
            } else {
                assert!(
                    matches!(result, Err(AccessTokenError::Transient(_))),
                    "unexpected refresh classification: {result:?}"
                );
            }
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
        let auth = OAuthCredential::in_memory_for_test(
            GOOGLE_TOKEN_URL,
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
            map_refresh_error(invalid_grant, OAuthProvider::Google),
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
            map_refresh_error(temporary, OAuthProvider::Google),
            AccessTokenError::Transient(_)
        ));
    }

    #[test]
    fn calendar_authorization_can_create_events_and_is_separate_from_mail() {
        let app = google_app();
        let mail = app.mail_credential("work@example.com");
        let calendar = app.calendar_account("work@example.com");

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
        let auth = config()
            .mail_account(MailProviderKind::Gmail, "work@example.com")
            .unwrap();
        assert_eq!(auth.key(), "work@example.com");
        assert_eq!(auth.mail_provider(), MailProviderKind::Gmail);
        assert!(auth.provider().capabilities().server_search);
    }

    #[test]
    fn a_pending_mail_account_signs_in_through_its_providers_mail_flow() {
        let auth = config()
            .pending_mail_account(MailProviderKind::Gmail)
            .unwrap();
        assert_eq!(auth.key(), PENDING_KEY);
        let AccountAuth::Gmail(credential) = &auth else {
            panic!("expected a Gmail account");
        };
        assert_eq!(credential.service, MAIL_SERVICE);
        assert_eq!(credential.endpoints.scopes, GMAIL_SCOPES);
    }

    #[test]
    fn an_imap_account_is_representable_without_an_oauth_credential() {
        let auth = config()
            .mail_account(MailProviderKind::Imap, "person@example.com")
            .unwrap();
        assert_eq!(auth.mail_provider(), MailProviderKind::Imap);
        assert_eq!(auth.key(), "person@example.com");
        // It has no OAuth credential, and asking for one is a clear error
        // rather than a panic — the interactive browser flow is not its path.
        assert!(auth.credential().is_none());
        assert!(auth.require_oauth_credential().is_err());
        assert!(matches!(auth, AccountAuth::Imap(_)));
    }

    #[test]
    fn a_gmail_account_still_exposes_its_oauth_credential_unchanged() {
        let auth = config()
            .mail_account(MailProviderKind::Gmail, "work@example.com")
            .unwrap();
        // Gmail's credential() is byte-for-byte the same handle as before,
        // now simply wrapped in Some.
        let credential = auth.credential().expect("Gmail always has a credential");
        assert_eq!(credential.key(), "work@example.com");
        assert_eq!(credential.service, MAIL_SERVICE);
        assert!(auth.require_oauth_credential().is_ok());
    }

    #[test]
    fn an_imap_provider_has_no_oauth_app() {
        // IMAP never reaches the OAuth app resolution; asking for one is a
        // clear error, not a silent Gmail fallback.
        let config = AuthConfig::default();
        assert!(config
            .mail_account(MailProviderKind::Imap, "person@example.com")
            .is_ok());
    }

    #[test]
    fn an_unconfigured_provider_cannot_connect_accounts() {
        let config = AuthConfig::default();
        let error = config
            .mail_account(MailProviderKind::Gmail, "work@example.com")
            .err()
            .unwrap();
        assert!(error.contains("THREESTRANDS_GOOGLE_CLIENT_ID"), "{error}");
        assert!(config.google().is_err());
    }

    #[test]
    fn a_stored_credential_is_only_presented_to_the_service_that_minted_it() {
        let tokens = Tokens {
            access_token: "at".into(),
            refresh_token: None,
            expires_at: 1,
        };
        let stored = OAuthProvider::Google.wrap(tokens);
        let decoded = StoredCredential::decode(&stored.encode().unwrap()).unwrap();
        assert_eq!(
            OAuthProvider::Google.unwrap(decoded).unwrap().access_token,
            "at"
        );
    }

    #[tokio::test]
    async fn a_public_client_omits_the_client_secret_from_token_requests() {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route("/token", post(capture_token_request))
            .with_state(requests.clone());
        let (token_url, server) = start_token_server(app).await;
        let mut endpoints = OAuthProvider::Google.mail_endpoints();
        endpoints.token_url = token_url;
        let public = OAuthApp {
            client_secret: None,
            ..google_app()
        }
        .keyed_for("test@example.com", MAIL_SERVICE, endpoints);
        let redirect = RedirectUrl::new("http://127.0.0.1:43210/oauth/callback".into()).unwrap();

        public
            .exchange_code(
                "authorization-code",
                PkceCodeVerifier::new("test-verifier-value".into()),
                redirect,
            )
            .await
            .unwrap();

        let requests = requests.lock().unwrap();
        assert_eq!(
            requests[0].get("client_id").map(String::as_str),
            Some("test-client-id")
        );
        assert!(!requests[0].contains_key("client_secret"));
        server.abort();
    }
}

/// An account's credential handle, abstracting over which mail provider it
/// authenticates through. `SyncService`, `ConnectedAccount`, and
/// `Correspondence` hold this rather than a concrete credential type, so
/// none of them has to change shape when a provider is added — it gains a
/// variant here and a new arm in `provider()` instead.
///
/// Every method below is a thin dispatch to the wrapped credential; this
/// type carries no state of its own.
#[derive(Clone)]
// `Gmail(OAuthCredential)` is ~880 bytes against `Imap(ImapCredential)`'s ~32
// in slice 2, which trips `large_enum_variant`. The asymmetry is temporary:
// `ImapCredential` is deliberately thin here and grows as the IMAP provider
// lands (server settings, pinned-cert handle). Boxing the hot Gmail variant
// to chase a transient gap would add an allocation and a deref to every
// Gmail credential access and churn every `AccountAuth::Gmail(_)` site, which
// this slice must keep byte-for-byte. Revisit once the Imap variant fills in.
#[allow(clippy::large_enum_variant)]
pub enum AccountAuth {
    Gmail(OAuthCredential),
    /// A password-authenticated IMAP/SMTP account. Added in Phase 1 slice 2
    /// so the auth model can *represent* a non-OAuth account; the IMAP
    /// `MailProvider` implementation and the "test and save" setup command
    /// that constructs this variant in production land in later phases.
    Imap(ImapCredential),
}

impl AccountAuth {
    /// The mail provider this account was connected through, as recorded
    /// in `accounts.provider`.
    pub fn mail_provider(&self) -> MailProviderKind {
        match self {
            Self::Gmail(_) => MailProviderKind::Gmail,
            Self::Imap(_) => MailProviderKind::Imap,
        }
    }

    /// The OAuth credential behind this account, when it has one — e.g. for
    /// the interactive sign-in flow. `None` for a non-OAuth account such as
    /// IMAP, which authenticates with a stored password and never runs the
    /// browser flow. Gmail always returns `Some`, exactly the credential it
    /// returned before this became optional.
    pub fn credential(&self) -> Option<&OAuthCredential> {
        match self {
            Self::Gmail(credential) => Some(credential),
            Self::Imap(_) => None,
        }
    }

    /// The OAuth credential this account must have to run the interactive
    /// browser sign-in, or a clear error for an account that authenticates
    /// another way. Gmail always succeeds; IMAP uses "test and save" instead.
    pub fn require_oauth_credential(&self) -> Result<&OAuthCredential, String> {
        self.credential().ok_or_else(|| {
            format!(
                "{} accounts do not use interactive OAuth sign-in",
                self.mail_provider().as_str()
            )
        })
    }

    /// This account's id, once known — see [`OAuthCredential::key`] for
    /// what that means before then.
    pub fn key(&self) -> String {
        match self {
            Self::Gmail(credential) => credential.key(),
            Self::Imap(credential) => credential.key(),
        }
    }

    pub fn available(&self) -> bool {
        match self {
            Self::Gmail(credential) => credential.available(),
            Self::Imap(credential) => credential.available(),
        }
    }

    pub fn disconnect(&self) -> Result<(), String> {
        match self {
            Self::Gmail(credential) => credential.disconnect(),
            Self::Imap(credential) => credential.disconnect(),
        }
    }

    pub fn accept_identity(&self, email: &str) -> Result<(), String> {
        match self {
            Self::Gmail(credential) => credential.accept_identity(email),
            // An IMAP account's address is entered by the user at setup, not
            // learned from an OAuth profile, so there is nothing to rekey.
            Self::Imap(_) => Ok(()),
        }
    }

    /// The mail backend this credential authorizes access to.
    pub fn provider(&self) -> std::sync::Arc<dyn crate::provider::MailProvider> {
        match self {
            Self::Gmail(credential) => std::sync::Arc::new(
                crate::provider::gmail::GmailClient::new(credential.clone()),
            ),
            // The IMAP provider lands in phase 2. No production path
            // constructs `AccountAuth::Imap` yet (there is no IMAP "test and
            // save" setup command), so this is unreachable in slice 2 rather
            // than a stub client that could silently misbehave.
            Self::Imap(_) => unreachable!(
                "the IMAP MailProvider lands in phase 2; no AccountAuth::Imap is constructed yet"
            ),
        }
    }
}

/// A password-authenticated IMAP/SMTP account's credential handle, persisted
/// in the OS keychain as a [`StoredCredential::ImapPassword`] under the
/// account's address — the same keychain, the same tagged envelope, and the
/// same encryption-at-rest as every OAuth credential.
///
/// Slice 2 carries only what representability needs: which keychain entry the
/// account's password lives in, so the account can report whether it is
/// connected and be disconnected. Reading the password to actually drive an
/// IMAP session is the IMAP provider's job in a later phase.
#[derive(Clone)]
pub struct ImapCredential {
    service: String,
    key: Arc<Mutex<String>>,
}

impl ImapCredential {
    /// A handle to the IMAP account stored under `key` (its address).
    pub fn for_account(key: &str) -> Self {
        Self {
            service: MAIL_SERVICE.to_string(),
            key: Arc::new(Mutex::new(key.to_string())),
        }
    }

    /// The address this account is stored under.
    pub fn key(&self) -> String {
        self.key.lock().unwrap().clone()
    }

    /// Whether a password is stored for this account. Mirrors
    /// [`OAuthCredential::available`]: an entry that reads back is connected.
    pub fn available(&self) -> bool {
        self.entry()
            .and_then(|entry| entry.get_password().map_err(|error| error.to_string()))
            .is_ok()
    }

    /// Stores (or replaces) this account's password credential, encrypted at
    /// rest by the OS keychain exactly like an OAuth token.
    ///
    /// No caller in slice 2 — the IMAP "test and save" setup command that
    /// writes a password (phase 2) is its first reader. The dead-code allow
    /// is scoped to the two store/retrieval methods and removed with that
    /// caller, the same way slice 1 scoped the unread `ProviderCapabilities`
    /// flags.
    #[allow(dead_code)]
    pub fn save(&self, password: &crate::credentials::ImapPassword) -> Result<(), String> {
        let value = StoredCredential::ImapPassword(password.clone()).encode()?;
        self.entry()?.set_password(&value).map_err(display)
    }

    /// Reads this account's stored password credential.
    #[allow(dead_code)]
    pub fn load(&self) -> Result<crate::credentials::ImapPassword, String> {
        let value = self.entry()?.get_password().map_err(display)?;
        match StoredCredential::decode(&value)? {
            StoredCredential::ImapPassword(password) => Ok(password),
            StoredCredential::GoogleOAuth(_) => {
                Err("expected an IMAP password but found an OAuth token credential".to_string())
            }
        }
    }

    pub fn disconnect(&self) -> Result<(), String> {
        match self.entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    }

    fn entry(&self) -> Result<Entry, String> {
        Entry::new(&self.service, &self.key()).map_err(display)
    }
}
