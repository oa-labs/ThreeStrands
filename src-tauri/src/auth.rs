use std::sync::{Mutex, OnceLock};
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
    time::timeout,
};
use url::Url;

const SERVICE: &str = "app.dispatch.mail";
const TOKEN_KEY: &str = "google-oauth-default";
const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const SCOPES: &str = "openid email https://www.googleapis.com/auth/gmail.modify https://www.googleapis.com/auth/gmail.labels";

// There is only ever one Google account per app instance (fixed SERVICE/TOKEN_KEY above),
// so an in-memory cache can live at process scope. This is what keeps `access_token()` and
// `available()` from hitting the OS keychain on every Gmail API call and every poll tick.
static TOKEN_CACHE: OnceLock<Mutex<Option<Tokens>>> = OnceLock::new();
static AVAILABLE_CACHE: OnceLock<Mutex<Option<bool>>> = OnceLock::new();

fn token_cache() -> &'static Mutex<Option<Tokens>> {
    TOKEN_CACHE.get_or_init(|| Mutex::new(None))
}

fn available_cache() -> &'static Mutex<Option<bool>> {
    AVAILABLE_CACHE.get_or_init(|| Mutex::new(None))
}

#[derive(Clone)]
pub struct GoogleAuth {
    client: Client,
    client_id: String,
    client_secret: String,
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

impl GoogleAuth {
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
            client: Client::new(),
            client_id,
            client_secret,
        })
    }

    pub fn available() -> bool {
        let cached = *available_cache().lock().unwrap();
        if let Some(value) = cached {
            return value;
        }
        let value = Self::entry()
            .and_then(|entry| entry.get_password().map_err(|error| error.to_string()))
            .is_ok();
        *available_cache().lock().unwrap() = Some(value);
        value
    }

    pub async fn authorize(&self) -> Result<(), String> {
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

        let (mut stream, _) = timeout(Duration::from_secs(300), listener.accept())
            .await
            .map_err(|_| "OAuth callback timed out".to_string())?
            .map_err(display)?;
        let mut request = vec![0_u8; 16 * 1024];
        let count = timeout(Duration::from_secs(10), stream.read(&mut request))
            .await
            .map_err(|_| "OAuth callback was incomplete".to_string())?
            .map_err(display)?;
        let first_line = String::from_utf8_lossy(&request[..count])
            .lines()
            .next()
            .ok_or_else(|| "Invalid OAuth callback".to_string())?
            .to_string();
        let target = first_line
            .split_whitespace()
            .nth(1)
            .ok_or_else(|| "Invalid OAuth callback".to_string())?;
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

    async fn exchange_code(
        &self,
        code: &str,
        verifier: &str,
        redirect_uri: &str,
    ) -> Result<(), String> {
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
        self.save(&Tokens {
            access_token: token.access_token,
            refresh_token: token.refresh_token,
            expires_at: now() + token.expires_in.saturating_sub(60),
        })
    }

    pub async fn access_token(&self) -> Result<String, String> {
        let mut tokens = self.load()?;
        if tokens.expires_at > now() {
            return Ok(tokens.access_token);
        }
        let refresh_token = tokens
            .refresh_token
            .as_deref()
            .ok_or_else(|| "Google session expired; reconnect the account".to_string())?;
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
            .map_err(display)?;
        let response = checked(response).await?;
        let refreshed: TokenResponse = response.json().await.map_err(display)?;
        tokens.access_token = refreshed.access_token;
        tokens.expires_at = now() + refreshed.expires_in.saturating_sub(60);
        self.save(&tokens)?;
        Ok(tokens.access_token)
    }

    pub fn disconnect() -> Result<(), String> {
        match Self::entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {
                *token_cache().lock().unwrap() = None;
                *available_cache().lock().unwrap() = Some(false);
                Ok(())
            }
            Err(error) => Err(error.to_string()),
        }
    }

    fn load(&self) -> Result<Tokens, String> {
        let cached = token_cache().lock().unwrap().clone();
        if let Some(tokens) = cached {
            return Ok(tokens);
        }
        let value = Self::entry()?.get_password().map_err(display)?;
        let tokens: Tokens = serde_json::from_str(&value).map_err(display)?;
        *token_cache().lock().unwrap() = Some(tokens.clone());
        Ok(tokens)
    }

    fn save(&self, tokens: &Tokens) -> Result<(), String> {
        let value = serde_json::to_string(tokens).map_err(display)?;
        Self::entry()?.set_password(&value).map_err(display)?;
        *token_cache().lock().unwrap() = Some(tokens.clone());
        *available_cache().lock().unwrap() = Some(true);
        Ok(())
    }

    fn entry() -> Result<Entry, String> {
        Entry::new(SERVICE, TOKEN_KEY).map_err(display)
    }
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
