use std::sync::Arc;

use axum::{
    extract::{FromRequestParts, Path, Query, State},
    http::{header::AUTHORIZATION, request::Parts, StatusCode},
    response::{IntoResponse, Redirect},
    Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{Duration as ChronoDuration, Utc};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::FromRow;
use threestrands_sync_protocol::{AccountProfile, Entitlement};
use url::Url;
use uuid::Uuid;

use crate::{ApiError, AppState};

const GOOGLE_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const GOOGLE_USERINFO_URL: &str = "https://openidconnect.googleapis.com/v1/userinfo";
const GOOGLE_TOKENINFO_URL: &str = "https://oauth2.googleapis.com/tokeninfo";
const GOOGLE_ISSUER: &str = "https://accounts.google.com";

#[derive(Debug, Clone)]
pub struct AuthContext {
    pub user_id: Uuid,
    pub device_id: Uuid,
    pub session_id: Uuid,
}

impl FromRequestParts<Arc<AppState>> for AuthContext {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &Arc<AppState>) -> Result<Self, Self::Rejection> {
        let token = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .ok_or_else(ApiError::unauthorized)?;
        let row = sqlx::query_as::<_, SessionRow>(
            "SELECT id, user_id, device_id FROM sessions
             WHERE access_hash=$1 AND access_expires_at > now() AND revoked_at IS NULL",
        )
        .bind(hash(token))
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(ApiError::unauthorized)?;
        sqlx::query("UPDATE devices SET last_seen_at=now() WHERE user_id=$1 AND id=$2")
            .bind(row.user_id)
            .bind(row.device_id)
            .execute(&state.pool)
            .await?;
        Ok(Self { user_id: row.user_id, device_id: row.device_id, session_id: row.id })
    }
}

#[derive(FromRow)]
struct SessionRow {
    id: Uuid,
    user_id: Uuid,
    device_id: Uuid,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartRequest {
    redirect_uri: String,
    code_challenge: String,
    state: String,
    device_name: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartResponse {
    authorization_url: String,
}

pub async fn google_start(
    State(state): State<Arc<AppState>>,
    Json(request): Json<StartRequest>,
) -> Result<Json<StartResponse>, ApiError> {
    validate_loopback(&request.redirect_uri)?;
    if request.code_challenge.len() < 43
        || request.code_challenge.len() > 128
        || request.state.is_empty()
        || request.state.len() > 200
        || request.device_name.trim().is_empty()
        || request.device_name.len() > 200
    {
        return Err(ApiError::bad_request("Invalid sign-in request"));
    }
    let oauth_state = random_token(32);
    let flow_id = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO auth_flows(oauth_state_hash, desktop_state, redirect_uri, code_challenge, device_name, expires_at)
         VALUES($1,$2,$3,$4,$5,now()+interval '10 minutes') RETURNING id",
    )
    .bind(hash(&oauth_state))
    .bind(&request.state)
    .bind(&request.redirect_uri)
    .bind(&request.code_challenge)
    .bind(request.device_name.trim())
    .fetch_one(&state.pool)
    .await?;

    let callback = format!("{}/v1/auth/google/callback", state.public_base_url);
    let mut url = Url::parse(GOOGLE_AUTH_URL).map_err(|_| ApiError::bad_request("Invalid OAuth configuration"))?;
    url.query_pairs_mut()
        .append_pair("client_id", &state.google_client_id)
        .append_pair("redirect_uri", &callback)
        .append_pair("response_type", "code")
        .append_pair("scope", "openid email profile")
        .append_pair("state", &oauth_state)
        .append_pair("nonce", &flow_id.to_string())
        .append_pair("prompt", "select_account");
    Ok(Json(StartResponse { authorization_url: url.to_string() }))
}

#[derive(Deserialize)]
pub struct CallbackQuery {
    code: Option<String>,
    state: String,
    error: Option<String>,
}

#[derive(FromRow)]
struct FlowRow {
    id: Uuid,
    desktop_state: String,
    redirect_uri: String,
}

pub async fn google_callback(
    State(state): State<Arc<AppState>>,
    Query(query): Query<CallbackQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let flow = sqlx::query_as::<_, FlowRow>(
        "SELECT id, desktop_state, redirect_uri FROM auth_flows
         WHERE oauth_state_hash=$1 AND expires_at > now() AND completed_at IS NULL",
    )
    .bind(hash(&query.state))
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(|| ApiError::bad_request("The sign-in request expired"))?;
    if let Some(error) = query.error {
        return Ok(Redirect::to(&callback_url(&flow.redirect_uri, &flow.desktop_state, None, Some(&error))?));
    }
    let code = query.code.ok_or_else(|| ApiError::bad_request("Google returned no authorization code"))?;
    let token: GoogleToken = state
        .http
        .post(GOOGLE_TOKEN_URL)
        .form(&[
            ("client_id", state.google_client_id.as_str()),
            ("client_secret", state.google_client_secret.as_str()),
            ("code", code.as_str()),
            ("grant_type", "authorization_code"),
            ("redirect_uri", &format!("{}/v1/auth/google/callback", state.public_base_url)),
        ])
        .send()
        .await?
        .error_for_status()
        .map_err(|_| ApiError::unauthorized())?
        .json()
        .await?;
    let profile: GoogleProfile = state
        .http
        .get(GOOGLE_USERINFO_URL)
        .bearer_auth(token.access_token)
        .send()
        .await?
        .error_for_status()
        .map_err(|_| ApiError::unauthorized())?
        .json()
        .await?;
    let claims: GoogleTokenClaims = state.http.get(GOOGLE_TOKENINFO_URL)
        .query(&[("id_token", token.id_token.as_str())]).send().await?
        .error_for_status().map_err(|_| ApiError::unauthorized())?.json().await?;
    let expected_nonce = flow.id.to_string();
    if claims.aud != state.google_client_id
        || !matches!(claims.iss.as_str(), "https://accounts.google.com" | "accounts.google.com")
        || claims.nonce.as_deref() != Some(expected_nonce.as_str())
        || claims.sub != profile.sub
        || claims.email != profile.email
        || !claims.email_verified
        || !profile.email_verified
    {
        return Err(ApiError::forbidden("Google email is not verified"));
    }

    let mut tx = state.pool.begin().await?;
    let existing = sqlx::query_scalar::<_, Uuid>(
        "SELECT user_id FROM auth_identities WHERE issuer=$1 AND subject=$2",
    )
    .bind(GOOGLE_ISSUER)
    .bind(&profile.sub)
    .fetch_optional(&mut *tx)
    .await?;
    let user_id = match existing {
        Some(id) => {
            sqlx::query("UPDATE users SET email=$1, display_name=$2, avatar_url=$3 WHERE id=$4")
                .bind(&profile.email)
                .bind(&profile.name)
                .bind(&profile.picture)
                .bind(id)
                .execute(&mut *tx)
                .await?;
            id
        }
        None => {
            let id = sqlx::query_scalar::<_, Uuid>(
                "INSERT INTO users(email,display_name,avatar_url) VALUES($1,$2,$3) RETURNING id",
            )
            .bind(&profile.email)
            .bind(&profile.name)
            .bind(&profile.picture)
            .fetch_one(&mut *tx)
            .await?;
            sqlx::query(
                "INSERT INTO auth_identities(issuer,subject,user_id,email) VALUES($1,$2,$3,$4)",
            )
            .bind(GOOGLE_ISSUER)
            .bind(&profile.sub)
            .bind(id)
            .bind(&profile.email)
            .execute(&mut *tx)
            .await?;
            id
        }
    };
    let desktop_code = random_token(32);
    sqlx::query(
        "INSERT INTO one_time_codes(code_hash,flow_id,user_id,expires_at)
         VALUES($1,$2,$3,now()+interval '2 minutes')",
    )
    .bind(hash(&desktop_code))
    .bind(flow.id)
    .bind(user_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE auth_flows SET completed_at=now() WHERE id=$1")
        .bind(flow.id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Redirect::to(&callback_url(
        &flow.redirect_uri,
        &flow.desktop_state,
        Some(&desktop_code),
        None,
    )?))
}

#[derive(Deserialize)]
struct GoogleToken {
    access_token: String,
    id_token: String,
}

#[derive(Deserialize)]
struct GoogleTokenClaims {
    aud: String,
    iss: String,
    sub: String,
    email: String,
    email_verified: bool,
    nonce: Option<String>,
}

#[derive(Deserialize)]
struct GoogleProfile {
    sub: String,
    email: String,
    #[serde(default)]
    email_verified: bool,
    name: Option<String>,
    picture: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExchangeRequest {
    code: String,
    code_verifier: String,
    device_id: Uuid,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenResponse {
    access_token: String,
    expires_in: i64,
    refresh_token: String,
}

#[derive(FromRow)]
struct CodeRow {
    user_id: Uuid,
    code_challenge: String,
    device_name: String,
}

pub async fn exchange_code(
    State(state): State<Arc<AppState>>,
    Json(request): Json<ExchangeRequest>,
) -> Result<Json<TokenResponse>, ApiError> {
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query_as::<_, CodeRow>(
        "SELECT c.user_id,f.code_challenge,f.device_name
         FROM one_time_codes c JOIN auth_flows f ON f.id=c.flow_id
         WHERE c.code_hash=$1 AND c.expires_at > now() AND c.consumed_at IS NULL FOR UPDATE",
    )
    .bind(hash(&request.code))
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(ApiError::unauthorized)?;
    let actual = URL_SAFE_NO_PAD.encode(Sha256::digest(request.code_verifier.as_bytes()));
    if actual != row.code_challenge {
        return Err(ApiError::unauthorized());
    }
    sqlx::query("UPDATE one_time_codes SET consumed_at=now() WHERE code_hash=$1")
        .bind(hash(&request.code))
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO devices(id,user_id,name) VALUES($1,$2,$3)
         ON CONFLICT(user_id,id) DO UPDATE SET name=excluded.name,last_seen_at=now(),revoked_at=NULL",
    )
    .bind(request.device_id)
    .bind(row.user_id)
    .bind(row.device_name)
    .execute(&mut *tx)
    .await?;
    let response = create_session(&mut tx, row.user_id, request.device_id).await?;
    tx.commit().await?;
    Ok(Json(response))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RefreshRequest {
    refresh_token: String,
}

#[derive(FromRow)]
struct RefreshRow {
    id: Uuid,
    user_id: Uuid,
    device_id: Uuid,
    replaced_by: Option<Uuid>,
}

pub async fn refresh(
    State(state): State<Arc<AppState>>,
    Json(request): Json<RefreshRequest>,
) -> Result<Json<TokenResponse>, ApiError> {
    let mut tx = state.pool.begin().await?;
    let row = sqlx::query_as::<_, RefreshRow>(
        "SELECT id,user_id,device_id,replaced_by FROM sessions
         WHERE refresh_hash=$1 AND refresh_expires_at > now() FOR UPDATE",
    )
    .bind(hash(&request.refresh_token))
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(ApiError::unauthorized)?;
    if row.replaced_by.is_some() || sqlx::query_scalar::<_, bool>("SELECT revoked_at IS NOT NULL FROM sessions WHERE id=$1")
        .bind(row.id).fetch_one(&mut *tx).await?
    {
        sqlx::query("UPDATE sessions SET revoked_at=now() WHERE user_id=$1 AND device_id=$2")
            .bind(row.user_id).bind(row.device_id).execute(&mut *tx).await?;
        tx.commit().await?;
        return Err(ApiError::unauthorized());
    }
    let response = create_session(&mut tx, row.user_id, row.device_id).await?;
    let replacement = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM sessions WHERE refresh_hash=$1",
    ).bind(hash(&response.refresh_token)).fetch_one(&mut *tx).await?;
    sqlx::query("UPDATE sessions SET revoked_at=now(),replaced_by=$1 WHERE id=$2")
        .bind(replacement).bind(row.id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(response))
}

pub async fn revoke(
    State(state): State<Arc<AppState>>,
    auth: AuthContext,
) -> Result<StatusCode, ApiError> {
    sqlx::query("UPDATE sessions SET revoked_at=now() WHERE id=$1 AND user_id=$2")
        .bind(auth.session_id).bind(auth.user_id).execute(&state.pool).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(FromRow)]
struct ProfileRow {
    id: Uuid,
    email: String,
    display_name: Option<String>,
    avatar_url: Option<String>,
}

#[derive(FromRow)]
struct EntitlementRow {
    feature: String,
    source: String,
    expires_at: Option<chrono::DateTime<Utc>>,
}

pub async fn me(
    State(state): State<Arc<AppState>>,
    auth: AuthContext,
) -> Result<Json<AccountProfile>, ApiError> {
    let profile = sqlx::query_as::<_, ProfileRow>("SELECT id,email,display_name,avatar_url FROM users WHERE id=$1")
        .bind(auth.user_id).fetch_one(&state.pool).await?;
    let entitlements = sqlx::query_as::<_, EntitlementRow>(
        "SELECT feature,source,expires_at FROM entitlements
         WHERE user_id=$1 AND (expires_at IS NULL OR expires_at > now())",
    ).bind(auth.user_id).fetch_all(&state.pool).await?;
    Ok(Json(AccountProfile {
        id: profile.id.to_string(), email: profile.email, display_name: profile.display_name,
        avatar_url: profile.avatar_url,
        entitlements: entitlements.into_iter().map(|value| Entitlement {
            feature: value.feature, source: value.source,
            expires_at: value.expires_at.map(|value| value.to_rfc3339()),
        }).collect(),
    }))
}

#[derive(Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct DeviceResponse {
    id: Uuid,
    name: String,
    created_at: chrono::DateTime<Utc>,
    last_seen_at: chrono::DateTime<Utc>,
    current: bool,
}

pub async fn devices(
    State(state): State<Arc<AppState>>,
    auth: AuthContext,
) -> Result<Json<Vec<DeviceResponse>>, ApiError> {
    let rows = sqlx::query_as::<_, DeviceDbRow>(
        "SELECT id,name,created_at,last_seen_at FROM devices WHERE user_id=$1 AND revoked_at IS NULL ORDER BY last_seen_at DESC",
    ).bind(auth.user_id).fetch_all(&state.pool).await?;
    Ok(Json(rows.into_iter().map(|row| DeviceResponse {
        current: row.id == auth.device_id, id: row.id, name: row.name,
        created_at: row.created_at, last_seen_at: row.last_seen_at,
    }).collect()))
}

#[derive(FromRow)]
struct DeviceDbRow {
    id: Uuid,
    name: String,
    created_at: chrono::DateTime<Utc>,
    last_seen_at: chrono::DateTime<Utc>,
}

pub async fn revoke_device(
    State(state): State<Arc<AppState>>,
    auth: AuthContext,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let mut tx = state.pool.begin().await?;
    let changed = sqlx::query("UPDATE devices SET revoked_at=now() WHERE user_id=$1 AND id=$2 AND revoked_at IS NULL")
        .bind(auth.user_id).bind(id).execute(&mut *tx).await?.rows_affected();
    if changed == 0 { return Err(ApiError::not_found()); }
    sqlx::query("UPDATE sessions SET revoked_at=now() WHERE user_id=$1 AND device_id=$2")
        .bind(auth.user_id).bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete_account(
    State(state): State<Arc<AppState>>,
    auth: AuthContext,
) -> Result<StatusCode, ApiError> {
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(auth.user_id).execute(&state.pool).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn create_session(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user_id: Uuid,
    device_id: Uuid,
) -> Result<TokenResponse, ApiError> {
    let access = random_token(32);
    let refresh = random_token(48);
    sqlx::query(
        "INSERT INTO sessions(user_id,device_id,access_hash,access_expires_at,refresh_hash,refresh_expires_at)
         VALUES($1,$2,$3,now()+interval '15 minutes',$4,now()+interval '30 days')",
    )
    .bind(user_id).bind(device_id).bind(hash(&access)).bind(hash(&refresh))
    .execute(&mut **tx).await?;
    Ok(TokenResponse { access_token: access, expires_in: ChronoDuration::minutes(15).num_seconds(), refresh_token: refresh })
}

fn callback_url(base: &str, state: &str, code: Option<&str>, error: Option<&str>) -> Result<String, ApiError> {
    let mut url = Url::parse(base).map_err(|_| ApiError::bad_request("Invalid loopback callback"))?;
    let mut pairs = url.query_pairs_mut();
    pairs.append_pair("state", state);
    if let Some(code) = code { pairs.append_pair("code", code); }
    if let Some(error) = error { pairs.append_pair("error", error); }
    drop(pairs);
    Ok(url.to_string())
}

fn validate_loopback(value: &str) -> Result<(), ApiError> {
    let url = Url::parse(value).map_err(|_| ApiError::bad_request("Invalid loopback callback"))?;
    if url.scheme() != "http"
        || !matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]" | "::1"))
        || url.port().is_none()
        || url.path() != "/account/callback"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ApiError::bad_request("Only an exact loopback callback is allowed"));
    }
    Ok(())
}

fn random_token(bytes: usize) -> String {
    let mut value = vec![0; bytes];
    OsRng.fill_bytes(&mut value);
    URL_SAFE_NO_PAD.encode(value)
}

fn hash(value: &str) -> Vec<u8> {
    Sha256::digest(value.as_bytes()).to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_narrow_loopback_callbacks() {
        assert!(validate_loopback("http://127.0.0.1:43821/account/callback").is_ok());
        assert!(validate_loopback("https://127.0.0.1:43821/account/callback").is_err());
        assert!(validate_loopback("http://example.com:43821/account/callback").is_err());
        assert!(validate_loopback("http://127.0.0.1:43821/other").is_err());
    }
}
