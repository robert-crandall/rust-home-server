use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration as StdDuration;

use argon2::Argon2;
use argon2::password_hash::rand_core::{OsRng, RngCore};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use axum::extract::{FromRef, FromRequestParts, Request};
use axum::http::header::{AUTHORIZATION, COOKIE, HOST, ORIGIN, SET_COOKIE};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, Method};
use axum::middleware::Next;
use axum::response::Response;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgPool};
use subtle::ConstantTimeEq;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use url::Url;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::routes::{ApiError, AppState};

pub const SESSION_COOKIE_NAME: &str = "rust_home_template_session";
pub const SESSION_TTL_DAYS: i64 = 30;
pub const SESSION_RENEW_THRESHOLD_DAYS: i64 = 15;
const MIN_PASSWORD_LEN: usize = 8;
const MAX_PASSWORD_LEN: usize = 128;
const MAX_EMAIL_LEN: usize = 254;
const MAX_NAME_LEN: usize = 120;
const FIRST_USER_ADVISORY_LOCK_KEY: i64 = 0x7363_686f_6c61_7231;
const MAX_CONCURRENT_PASSWORD_JOBS: usize = 4;
const PASSWORD_QUEUE_TIMEOUT: StdDuration = StdDuration::from_secs(5);

static PASSWORD_SEMAPHORE: LazyLock<Arc<Semaphore>> =
    LazyLock::new(|| Arc::new(Semaphore::new(MAX_CONCURRENT_PASSWORD_JOBS)));

static DUMMY_PASSWORD_HASH: LazyLock<String> = LazyLock::new(|| {
    hash_password_sync("rust-home-template-dummy-timing-equalizer")
        .expect("dummy Argon2id password hash must build")
});

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: Uuid,
    pub email: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ApiToken {
    pub id: Uuid,
    pub name: String,
    pub last4: String,
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CreatedApiToken {
    pub id: Uuid,
    pub name: String,
    pub token: String,
    pub last4: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default)]
pub struct RenewedCookieSlot(Arc<Mutex<Option<HeaderValue>>>);

#[derive(Debug, Clone)]
pub struct SessionUser(pub User);

#[derive(Debug, Clone)]
pub struct AuthenticatedUser(pub User);

fn hash_password_sync(password: &str) -> Result<String, ApiError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|err| {
            tracing::error!(?err, "failed to hash password");
            ApiError::internal("failed to process credentials")
        })
}

async fn acquire_password_permit() -> Result<OwnedSemaphorePermit, ApiError> {
    tokio::time::timeout(
        PASSWORD_QUEUE_TIMEOUT,
        Arc::clone(&PASSWORD_SEMAPHORE).acquire_owned(),
    )
    .await
    .map_err(|_| ApiError::too_many_requests("authentication busy, please try again"))?
    .map_err(|_| ApiError::internal("failed to process credentials"))
}

pub async fn hash_password(password: String) -> Result<String, ApiError> {
    let permit = acquire_password_permit().await?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        hash_password_sync(&password)
    })
    .await
    .map_err(|err| {
        tracing::error!(?err, "password hash task panicked");
        ApiError::internal("failed to process credentials")
    })?
}

pub async fn verify_password(
    password: String,
    password_hash: Option<String>,
) -> Result<bool, ApiError> {
    let permit = acquire_password_permit().await?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let candidate = password_hash.as_deref().unwrap_or(&DUMMY_PASSWORD_HASH);
        let parsed = PasswordHash::new(candidate).map_err(|err| {
            tracing::error!(?err, "invalid stored password hash");
            ApiError::internal("failed to verify credentials")
        })?;
        let matched = Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok();
        Ok(matched && password_hash.is_some())
    })
    .await
    .map_err(|err| {
        tracing::error!(?err, "password verify task panicked");
        ApiError::internal("failed to verify credentials")
    })?
}

pub fn normalize_and_validate_email(raw: &str) -> Result<String, ApiError> {
    let email = raw.trim().to_ascii_lowercase();
    if email.is_empty() || email.len() > MAX_EMAIL_LEN {
        return Err(ApiError::bad_request("a valid email address is required"));
    }
    let Some((local, domain)) = email.split_once('@') else {
        return Err(ApiError::bad_request("a valid email address is required"));
    };
    if local.is_empty()
        || domain.is_empty()
        || domain.contains('@')
        || domain.starts_with('.')
        || domain.ends_with('.')
        || email.chars().any(char::is_whitespace)
    {
        return Err(ApiError::bad_request("a valid email address is required"));
    }
    Ok(email)
}

pub fn validate_password(password: &str) -> Result<(), ApiError> {
    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(ApiError::bad_request(
            "password must be at least 8 characters",
        ));
    }
    if password.len() > MAX_PASSWORD_LEN {
        return Err(ApiError::bad_request("password must be at most 128 bytes"));
    }
    Ok(())
}

pub fn normalize_display_name(raw: Option<&str>, fallback_email: &str) -> Result<String, ApiError> {
    let trimmed = raw.map(str::trim).unwrap_or("");
    if trimmed.chars().count() > MAX_NAME_LEN {
        return Err(ApiError::bad_request("name must be at most 120 characters"));
    }
    if trimmed.is_empty() {
        let local = fallback_email.split('@').next().unwrap_or("User");
        return Ok(local.to_string());
    }
    Ok(trimmed.to_string())
}

pub fn random_hex_secret() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

pub fn sha256_bytes(input: &str) -> Vec<u8> {
    Sha256::digest(input.as_bytes()).to_vec()
}

pub fn build_session_cookie(token: &str, secure: bool) -> HeaderValue {
    let max_age = SESSION_TTL_DAYS * 24 * 60 * 60;
    let secure_attr = if secure { "; Secure" } else { "" };
    let value = format!(
        "{SESSION_COOKIE_NAME}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{secure_attr}"
    );
    HeaderValue::from_str(&value).expect("valid Set-Cookie header")
}

pub fn build_clear_session_cookie(secure: bool) -> HeaderValue {
    let secure_attr = if secure { "; Secure" } else { "" };
    let value =
        format!("{SESSION_COOKIE_NAME}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0{secure_attr}");
    HeaderValue::from_str(&value).expect("valid clear Set-Cookie header")
}

pub fn extract_session_cookie(headers: &HeaderMap) -> Option<String> {
    for header in headers.get_all(COOKIE) {
        let Ok(raw) = header.to_str() else {
            continue;
        };
        for pair in raw.split(';') {
            let Some((name, value)) = pair.trim().split_once('=') else {
                continue;
            };
            if name == SESSION_COOKIE_NAME && !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

pub async fn create_session(pool: &PgPool, user_id: Uuid) -> Result<String, ApiError> {
    let raw_token = random_hex_secret();
    let token_hash = sha256_bytes(&raw_token);
    let expires_at = Utc::now() + Duration::days(SESSION_TTL_DAYS);

    sqlx::query("INSERT INTO sessions (token_hash, user_id, expires_at) VALUES ($1, $2, $3)")
        .bind(token_hash)
        .bind(user_id)
        .bind(expires_at)
        .execute(pool)
        .await
        .map_err(|err| {
            tracing::error!(?err, "failed to create session");
            ApiError::internal("failed to create session")
        })?;

    Ok(raw_token)
}

pub async fn register_user_with_session(
    pool: &PgPool,
    allow_open_registration: bool,
    email: &str,
    password_hash: &str,
    name: &str,
) -> Result<(User, String), ApiError> {
    let mut tx = pool.begin().await.map_err(|err| {
        tracing::error!(?err, "failed to begin registration transaction");
        ApiError::internal("failed to create account")
    })?;

    if !allow_open_registration {
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(FIRST_USER_ADVISORY_LOCK_KEY)
            .execute(&mut *tx)
            .await
            .map_err(|err| {
                tracing::error!(?err, "failed to acquire registration lock");
                ApiError::internal("failed to create account")
            })?;

        let any_user: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users)")
            .fetch_one(&mut *tx)
            .await
            .map_err(|err| {
                tracing::error!(?err, "failed to check existing users");
                ApiError::internal("failed to create account")
            })?;

        if any_user {
            return Err(ApiError::forbidden("registration is closed"));
        }
    }

    let user = sqlx::query_as::<_, User>(
        "INSERT INTO users (email, password_hash, name)
         VALUES ($1, $2, $3)
         RETURNING id, email, name, created_at",
    )
    .bind(email)
    .bind(password_hash)
    .bind(name)
    .fetch_one(&mut *tx)
    .await
    .map_err(|err| {
        if let sqlx::Error::Database(db_err) = &err
            && db_err.code().as_deref() == Some("23505")
        {
            return ApiError::conflict("an account with that email already exists");
        }
        tracing::error!(?err, "failed to insert user");
        ApiError::internal("failed to create account")
    })?;

    let raw_token = random_hex_secret();
    let token_hash = sha256_bytes(&raw_token);
    let expires_at = Utc::now() + Duration::days(SESSION_TTL_DAYS);

    sqlx::query("INSERT INTO sessions (token_hash, user_id, expires_at) VALUES ($1, $2, $3)")
        .bind(token_hash)
        .bind(user.id)
        .bind(expires_at)
        .execute(&mut *tx)
        .await
        .map_err(|err| {
            tracing::error!(?err, "failed to insert initial session");
            ApiError::internal("failed to create session")
        })?;

    tx.commit().await.map_err(|err| {
        tracing::error!(?err, "failed to commit registration");
        ApiError::internal("failed to create account")
    })?;

    Ok((user, raw_token))
}

#[derive(FromRow)]
struct SessionLookupRow {
    id: Uuid,
    email: String,
    name: String,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
}

async fn authenticate_session_cookie(
    state: &AppState,
    parts: &mut Parts,
) -> Result<User, ApiError> {
    let Some(raw_token) = extract_session_cookie(&parts.headers) else {
        return Err(ApiError::unauthorized("authentication required"));
    };
    let token_hash = sha256_bytes(&raw_token);
    let now = Utc::now();

    let row = sqlx::query_as::<_, SessionLookupRow>(
        "SELECT u.id, u.email, u.name, u.created_at, s.expires_at
         FROM sessions s
         JOIN users u ON u.id = s.user_id
         WHERE s.token_hash = $1 AND s.expires_at > $2",
    )
    .bind(&token_hash)
    .bind(now)
    .fetch_optional(&state.pool)
    .await
    .map_err(|err| {
        tracing::error!(?err, "failed to look up session");
        ApiError::internal("failed to verify session")
    })?
    .ok_or_else(|| ApiError::unauthorized("authentication required"))?;

    if row.expires_at < now + Duration::days(SESSION_RENEW_THRESHOLD_DAYS) {
        let new_expires_at = now + Duration::days(SESSION_TTL_DAYS);
        if let Err(err) = sqlx::query("UPDATE sessions SET expires_at = $2 WHERE token_hash = $1")
            .bind(&token_hash)
            .bind(new_expires_at)
            .execute(&state.pool)
            .await
        {
            tracing::warn!(?err, "failed to slide session expiration");
        } else if let Some(slot) = parts.extensions.get::<RenewedCookieSlot>()
            && let Ok(mut guard) = slot.0.lock()
        {
            *guard = Some(build_session_cookie(&raw_token, state.secure_cookies));
        }
    }

    Ok(User {
        id: row.id,
        email: row.email,
        name: row.name,
        created_at: row.created_at,
    })
}

#[derive(FromRow)]
struct TokenLookupRow {
    secret_hash: Vec<u8>,
    user_id: Uuid,
    email: String,
    name: String,
    created_at: DateTime<Utc>,
}

async fn authenticate_bearer_token(
    pool: &PgPool,
    header_val: &HeaderValue,
) -> Result<User, ApiError> {
    let raw = header_val
        .to_str()
        .map_err(|_| ApiError::unauthorized("invalid authorization header"))?;
    let (scheme, credential) = raw
        .trim()
        .split_once(char::is_whitespace)
        .ok_or_else(|| ApiError::unauthorized("invalid authorization header"))?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return Err(ApiError::unauthorized("invalid authorization header"));
    }
    let token = credential.trim();
    if token.is_empty() {
        return Err(ApiError::unauthorized("invalid authorization header"));
    }

    let rest = token
        .strip_prefix("pat_")
        .ok_or_else(|| ApiError::unauthorized("invalid token"))?;
    let (id_str, secret) = rest
        .split_once('_')
        .ok_or_else(|| ApiError::unauthorized("invalid token"))?;
    if secret.is_empty() {
        return Err(ApiError::unauthorized("invalid token"));
    }
    let token_id = Uuid::parse_str(id_str).map_err(|_| ApiError::unauthorized("invalid token"))?;

    let row = sqlx::query_as::<_, TokenLookupRow>(
        "SELECT t.secret_hash, u.id AS user_id, u.email, u.name, u.created_at
         FROM api_tokens t
         JOIN users u ON u.id = t.user_id
         WHERE t.id = $1",
    )
    .bind(token_id)
    .fetch_optional(pool)
    .await
    .map_err(|err| {
        tracing::error!(?err, "failed to look up API token");
        ApiError::internal("failed to verify token")
    })?
    .ok_or_else(|| ApiError::unauthorized("invalid token"))?;

    let candidate_hash = sha256_bytes(secret);
    if candidate_hash.ct_eq(&row.secret_hash).unwrap_u8() != 1 {
        return Err(ApiError::unauthorized("invalid token"));
    }

    if let Err(err) = sqlx::query("UPDATE api_tokens SET last_used_at = now() WHERE id = $1")
        .bind(token_id)
        .execute(pool)
        .await
    {
        tracing::warn!(?err, "failed to update api_tokens.last_used_at");
    }

    Ok(User {
        id: row.user_id,
        email: row.email,
        name: row.name,
        created_at: row.created_at,
    })
}

impl<S> FromRequestParts<S> for SessionUser
where
    AppState: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        if parts.headers.contains_key(AUTHORIZATION) {
            return Err(ApiError::forbidden(
                "this endpoint requires a browser session",
            ));
        }
        let app_state = AppState::from_ref(state);
        let user = authenticate_session_cookie(&app_state, parts).await?;
        Ok(SessionUser(user))
    }
}

impl<S> FromRequestParts<S> for AuthenticatedUser
where
    AppState: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let app_state = AppState::from_ref(state);
        // Never fall back to cookies when an Authorization header is supplied.
        if let Some(auth_header) = parts.headers.get(AUTHORIZATION) {
            let user = authenticate_bearer_token(&app_state.pool, auth_header).await?;
            return Ok(AuthenticatedUser(user));
        }
        let user = authenticate_session_cookie(&app_state, parts).await?;
        Ok(AuthenticatedUser(user))
    }
}

pub async fn security_middleware(mut req: Request, next: Next) -> Result<Response, ApiError> {
    let is_safe = matches!(
        *req.method(),
        Method::GET | Method::HEAD | Method::OPTIONS | Method::TRACE
    );
    if !is_safe {
        enforce_same_origin(req.headers())?;
    }

    let slot = RenewedCookieSlot::default();
    req.extensions_mut().insert(slot.clone());

    let mut response = next.run(req).await;
    if let Ok(mut guard) = slot.0.lock()
        && let Some(cookie) = guard.take()
    {
        response.headers_mut().append(SET_COOKIE, cookie);
    }
    Ok(response)
}

fn enforce_same_origin(headers: &HeaderMap) -> Result<(), ApiError> {
    if let Some(sec_fetch_site) = headers.get("sec-fetch-site").and_then(|v| v.to_str().ok())
        && sec_fetch_site.eq_ignore_ascii_case("cross-site")
    {
        return Err(ApiError::forbidden("cross-site requests are not allowed"));
    }

    if let Some(origin_raw) = headers.get(ORIGIN) {
        let origin_str = origin_raw
            .to_str()
            .map_err(|_| ApiError::forbidden("invalid Origin header"))?;
        let origin_url =
            Url::parse(origin_str).map_err(|_| ApiError::forbidden("invalid Origin header"))?;
        let origin_host = match (origin_url.host_str(), origin_url.port()) {
            (Some(h), Some(p)) => format!("{h}:{p}"),
            (Some(h), None) => h.to_string(),
            _ => return Err(ApiError::forbidden("invalid Origin header")),
        };
        let host = headers
            .get(HOST)
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| ApiError::forbidden("missing Host header"))?;

        if !origin_host.eq_ignore_ascii_case(host) {
            return Err(ApiError::forbidden("cross-origin requests are not allowed"));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email_validation_rejects_multiple_at_signs() {
        assert_eq!(
            normalize_and_validate_email("  Sister@Example.com ").unwrap(),
            "sister@example.com"
        );
        assert!(normalize_and_validate_email("a@b@c.com").is_err());
        assert!(normalize_and_validate_email("user@.example.com").is_err());
    }

    #[test]
    fn password_validation_counts_characters_not_utf8_bytes() {
        // Two 4-byte emoji are 8 UTF-8 bytes but only 2 characters.
        assert!(validate_password("🔐🔐").is_err());
        assert!(validate_password("🔐🔐🔐🔐🔐🔐🔐🔐").is_ok());
        assert!(validate_password(&"a".repeat(129)).is_err());
    }
}
