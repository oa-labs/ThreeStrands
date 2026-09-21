mod auth;
mod sync;

use std::{env, sync::Arc};

use axum::{
    extract::DefaultBodyLimit,
    http::StatusCode,
    routing::{delete, get, post},
    Json, Router,
};
use serde::Serialize;
use sqlx::{postgres::PgPoolOptions, PgPool};
use tower::ServiceBuilder;
use tower_http::{request_id::MakeRequestUuid, trace::TraceLayer, ServiceBuilderExt};

pub use auth::AuthContext;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub http: reqwest::Client,
    pub google_client_id: String,
    pub google_client_secret: String,
    pub public_base_url: String,
}

impl AppState {
    pub async fn from_environment() -> anyhow::Result<Self> {
        let database_url = required("DATABASE_URL")?;
        let pool = PgPoolOptions::new().max_connections(20).connect(&database_url).await?;
        Ok(Self {
            pool,
            http: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .timeout(std::time::Duration::from_secs(30))
                .build()?,
            google_client_id: required("GOOGLE_OIDC_CLIENT_ID")?,
            google_client_secret: required("GOOGLE_OIDC_CLIENT_SECRET")?,
            public_base_url: required("PUBLIC_BASE_URL")?.trim_end_matches('/').to_string(),
        })
    }
}

fn required(key: &str) -> anyhow::Result<String> {
    env::var(key).map_err(|_| anyhow::anyhow!("{key} is required"))
}

pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .route("/v1/auth/google/start", post(auth::google_start))
        .route("/v1/auth/google/callback", get(auth::google_callback))
        .route("/v1/auth/token", post(auth::exchange_code))
        .route("/v1/auth/refresh", post(auth::refresh))
        .route("/v1/auth/revoke", post(auth::revoke))
        .route("/v1/me", get(auth::me).delete(auth::delete_account))
        .route("/v1/devices", get(auth::devices))
        .route("/v1/devices/{id}", delete(auth::revoke_device))
        .route("/v1/sync", post(sync::synchronize))
        .route("/v1/conflicts", get(sync::list_conflicts))
        .route("/v1/conflicts/{id}/resolve", post(sync::resolve_conflict))
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024))
        .layer(
            ServiceBuilder::new()
                .set_x_request_id(MakeRequestUuid)
                .propagate_x_request_id()
                .layer(TraceLayer::new_for_http()),
        )
        .with_state(Arc::new(state))
}

#[derive(Serialize)]
struct Health<'a> {
    status: &'a str,
}

async fn live() -> Json<Health<'static>> {
    Json(Health { status: "ok" })
}

async fn ready(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
) -> Result<Json<Health<'static>>, ApiError> {
    sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(&state.pool)
        .await?;
    Ok(Json(Health { status: "ready" }))
}

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self { status: StatusCode::BAD_REQUEST, message: message.into() }
    }
    pub fn unauthorized() -> Self {
        Self { status: StatusCode::UNAUTHORIZED, message: "Authentication required".into() }
    }
    pub fn forbidden(message: impl Into<String>) -> Self {
        Self { status: StatusCode::FORBIDDEN, message: message.into() }
    }
    pub fn not_found() -> Self {
        Self { status: StatusCode::NOT_FOUND, message: "Not found".into() }
    }
    pub fn conflict(message: impl Into<String>) -> Self {
        Self { status: StatusCode::CONFLICT, message: message.into() }
    }
}

impl<E> From<E> for ApiError
where
    E: std::error::Error,
{
    fn from(error: E) -> Self {
        tracing::error!(error = %error, "request failed");
        Self { status: StatusCode::INTERNAL_SERVER_ERROR, message: "Internal server error".into() }
    }
}

impl axum::response::IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        (self.status, Json(serde_json::json!({ "error": self.message }))).into_response()
    }
}
