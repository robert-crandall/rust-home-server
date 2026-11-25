use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::header::{
    AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE, COOKIE, SET_COOKIE, X_CONTENT_TYPE_OPTIONS,
};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use utoipa::openapi::security::{ApiKey, ApiKeyValue, HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use crate::auth::{
    ApiToken, AuthenticatedUser, CfAccessAuth, CreatedApiToken, SESSION_COOKIE_NAME, SessionUser,
    User, build_clear_session_cookie, build_session_cookie, create_session, extract_cf_access_jwt,
    extract_session_cookie, hash_password, normalize_and_validate_email, normalize_display_name,
    random_hex_secret, register_user_with_session, security_middleware, sha256_bytes,
    validate_password, verify_password,
};

use crate::household::HouseholdProfiles;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub secure_cookies: bool,
    pub allow_open_registration: bool,
    pub cf_access: Option<CfAccessAuth>,
    pub household: Option<HouseholdProfiles>,
}

impl AppState {
    pub async fn hydrate_user(&self, user: User) -> User {
        match &self.household {
            Some(household) => household.apply_to_user_async(user).await,
            None => user,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
pub struct ErrorBody {
    pub error: String,
}

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
}

impl ApiError {
    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: msg.into(),
        }
    }

    pub fn unauthorized(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            message: msg.into(),
        }
    }

    pub fn forbidden(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            message: msg.into(),
        }
    }

    pub fn not_found(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: msg.into(),
        }
    }

    pub fn conflict(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            message: msg.into(),
        }
    }

    pub fn too_many_requests(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            message: msg.into(),
        }
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: msg.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorBody {
                error: self.message,
            }),
        )
            .into_response()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HealthStatus {
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub registration_open: bool,
    pub cloudflare_access_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LogoutResponse {
    pub logout_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RegisterRequest {
    pub email: String,
    pub password: String,
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct CreateTokenRequest {
    pub name: String,
}

struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "sessionCookie",
            SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::new(SESSION_COOKIE_NAME))),
        );
        components.add_security_scheme(
            "bearerToken",
            SecurityScheme::Http(
                HttpBuilder::new()
                    .scheme(HttpAuthScheme::Bearer)
                    .bearer_format("pat_<uuid>_<secret>")
                    .build(),
            ),
        );
    }
}

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Rust Home Server API",
        version = "0.1.0",
        description = "Self-hosted homelab web portal and MCP backend."
    ),
    modifiers(&SecurityAddon),
    tags(
        (name = "system", description = "Health and bootstrap metadata"),
        (name = "auth", description = "Email + password authentication"),
        (name = "tokens", description = "Personal access tokens for local MCP clients")
    )
)]
pub struct ApiDoc;

pub fn build_openapi_router() -> OpenApiRouter<AppState> {
    OpenApiRouter::with_openapi(ApiDoc::openapi())
        .routes(routes!(healthz))
        .routes(routes!(get_app_info))
        .routes(routes!(register))
        .routes(routes!(login))
        .routes(routes!(logout))
        .routes(routes!(me))
        .routes(routes!(get_user_avatar))
        .routes(routes!(list_tokens, create_token))
        .routes(routes!(delete_token))
}

pub fn openapi_spec() -> utoipa::openapi::OpenApi {
    let (_, spec) = build_openapi_router().split_for_parts();
    spec
}

pub fn build_router(state: AppState) -> Router {
    let (router, _) = build_openapi_router().split_for_parts();
    router
        .layer(middleware::from_fn(security_middleware))
        .with_state(state)
}

#[utoipa::path(
    get,
    path = "/healthz",
    tag = "system",
    responses(
        (status = 200, description = "Server and database are healthy", body = HealthStatus),
        (status = 503, description = "Database is unreachable", body = HealthStatus)
    )
)]
async fn healthz(State(state): State<AppState>) -> Response {
    let check = tokio::time::timeout(
        Duration::from_secs(2),
        sqlx::query_scalar::<_, i32>("SELECT 1").fetch_one(&state.pool),
    )
    .await;

    match check {
        Ok(Ok(1)) => (
            StatusCode::OK,
            Json(HealthStatus {
                status: "ok".to_string(),
            }),
        )
            .into_response(),
        _ => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(HealthStatus {
                status: "unavailable".to_string(),
            }),
        )
            .into_response(),
    }
}

async fn any_user_exists(pool: &PgPool) -> Result<bool, ApiError> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users)")
        .fetch_one(pool)
        .await
        .map_err(|err| {
            tracing::error!(?err, "failed to check existing users");
            ApiError::internal("failed to load app state")
        })
}

#[utoipa::path(
    get,
    path = "/api/app",
    tag = "system",
    responses(
        (status = 200, description = "Application bootstrap settings", body = AppInfo),
        (status = 500, description = "Internal error", body = ErrorBody)
    )
)]
async fn get_app_info(State(state): State<AppState>) -> Result<Json<AppInfo>, ApiError> {
    let registration_open = if state.allow_open_registration {
        true
    } else {
        !any_user_exists(&state.pool).await?
    };

    Ok(Json(AppInfo {
        registration_open,
        cloudflare_access_enabled: state.cf_access.is_some(),
    }))
}

#[utoipa::path(
    post,
    path = "/api/auth/register",
    tag = "auth",
    request_body = RegisterRequest,
    responses(
        (status = 201, description = "Account created and signed in", body = User),
        (status = 400, description = "Invalid email, password, or name", body = ErrorBody),
        (status = 403, description = "Registration is closed", body = ErrorBody),
        (status = 409, description = "Email already registered or conflicting Cloudflare Access session", body = ErrorBody),
        (status = 429, description = "Authentication queue busy", body = ErrorBody)
    )
)]
async fn register(
    State(state): State<AppState>,
    req_headers: HeaderMap,
    Json(payload): Json<RegisterRequest>,
) -> Result<Response, ApiError> {
    if state.cf_access.is_some() && extract_cf_access_jwt(&req_headers).is_some() {
        return Err(ApiError::conflict(
            "local authentication is disabled while Cloudflare Access credentials are present",
        ));
    }
    let email = normalize_and_validate_email(&payload.email)?;
    validate_password(&payload.password)?;
    let requested_name = payload
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty());
    let household_name = if requested_name.is_none() {
        match &state.household {
            Some(h) => h.display_name_for_async(email.clone()).await,
            None => None,
        }
    } else {
        None
    };
    let name = normalize_display_name(requested_name.or(household_name.as_deref()), &email)?;

    // Reject closed registration before scheduling Argon2 work; the advisory-locked
    // check inside `register_user_with_session` remains authoritative for first-user races.
    if !state.allow_open_registration && any_user_exists(&state.pool).await? {
        return Err(ApiError::forbidden("registration is closed"));
    }

    let password_hash = hash_password(payload.password).await?;
    let (user, raw_session) = register_user_with_session(
        &state.pool,
        state.allow_open_registration,
        &email,
        &password_hash,
        &name,
    )
    .await?;
    let user = state.hydrate_user(user).await;

    let mut headers = HeaderMap::new();
    headers.insert(
        SET_COOKIE,
        build_session_cookie(&raw_session, state.secure_cookies),
    );
    Ok((StatusCode::CREATED, headers, Json(user)).into_response())
}

#[derive(sqlx::FromRow)]
struct LoginUserRow {
    id: Uuid,
    email: String,
    name: String,
    created_at: chrono::DateTime<chrono::Utc>,
    password_hash: Option<String>,
}

#[utoipa::path(
    post,
    path = "/api/auth/login",
    tag = "auth",
    request_body = LoginRequest,
    responses(
        (status = 200, description = "Signed in", body = User),
        (status = 400, description = "Malformed credentials", body = ErrorBody),
        (status = 401, description = "Invalid email or password", body = ErrorBody),
        (status = 409, description = "Conflicting Cloudflare Access session", body = ErrorBody),
        (status = 429, description = "Authentication queue busy", body = ErrorBody)
    )
)]
async fn login(
    State(state): State<AppState>,
    req_headers: HeaderMap,
    Json(payload): Json<LoginRequest>,
) -> Result<Response, ApiError> {
    if state.cf_access.is_some() && extract_cf_access_jwt(&req_headers).is_some() {
        return Err(ApiError::conflict(
            "local authentication is disabled while Cloudflare Access credentials are present",
        ));
    }
    let email = normalize_and_validate_email(&payload.email)?;
    if payload.password.is_empty() || payload.password.len() > 128 {
        // Still run constant-time dummy hash verification so password length
        // boundaries don't leak account existence.
        let _ = verify_password("invalid-length-probe".to_string(), None).await;
        return Err(ApiError::unauthorized("invalid email or password"));
    }

    let row = sqlx::query_as::<_, LoginUserRow>(
        "SELECT id, email, name, created_at, password_hash
         FROM users
         WHERE lower(email) = $1",
    )
    .bind(&email)
    .fetch_optional(&state.pool)
    .await
    .map_err(|err| {
        tracing::error!(?err, "failed to query user during login");
        ApiError::internal("failed to verify credentials")
    })?;

    let stored_hash = row.as_ref().and_then(|r| r.password_hash.clone());
    let valid = verify_password(payload.password, stored_hash).await?;
    let Some(row) = row.filter(|_| valid) else {
        return Err(ApiError::unauthorized("invalid email or password"));
    };

    let raw_session = create_session(&state.pool, row.id).await?;
    let user = User {
        id: row.id,
        email: row.email,
        name: row.name,
        avatar_url: None,
        created_at: row.created_at,
    };
    let user = state.hydrate_user(user).await;

    let mut headers = HeaderMap::new();
    headers.insert(
        SET_COOKIE,
        build_session_cookie(&raw_session, state.secure_cookies),
    );
    Ok((StatusCode::OK, headers, Json(user)).into_response())
}

#[utoipa::path(
    post,
    path = "/api/auth/logout",
    tag = "auth",
    responses(
        (status = 200, description = "Signed out", body = LogoutResponse),
        (status = 403, description = "Cross-origin or non-browser request rejected", body = ErrorBody),
        (status = 500, description = "Internal error", body = ErrorBody)
    )
)]
async fn logout(
    State(state): State<AppState>,
    req_headers: HeaderMap,
) -> Result<Response, ApiError> {
    if req_headers.contains_key(AUTHORIZATION) {
        return Err(ApiError::forbidden(
            "this endpoint requires a browser session",
        ));
    }

    if req_headers.contains_key(COOKIE)
        && let Some(raw_token) = extract_session_cookie(&req_headers)
    {
        let token_hash = sha256_bytes(&raw_token);
        sqlx::query("DELETE FROM sessions WHERE token_hash = $1")
            .bind(token_hash)
            .execute(&state.pool)
            .await
            .map_err(|err| {
                tracing::error!(?err, "failed to delete session on logout");
                ApiError::internal("failed to sign out")
            })?;
    }

    let logout_url = if state.cf_access.is_some() && extract_cf_access_jwt(&req_headers).is_some() {
        Some("/cdn-cgi/access/logout".to_string())
    } else {
        None
    };

    let mut headers = HeaderMap::new();
    headers.insert(SET_COOKIE, build_clear_session_cookie(state.secure_cookies));
    Ok((StatusCode::OK, headers, Json(LogoutResponse { logout_url })).into_response())
}

#[utoipa::path(
    get,
    path = "/api/auth/me",
    tag = "auth",
    security(
        ("sessionCookie" = []),
        ("bearerToken" = [])
    ),
    responses(
        (status = 200, description = "Current authenticated user", body = User),
        (status = 401, description = "Not signed in", body = ErrorBody)
    )
)]
async fn me(AuthenticatedUser(user): AuthenticatedUser) -> Json<User> {
    Json(user)
}

#[utoipa::path(
    get,
    path = "/api/users/{id}/avatar",
    tag = "auth",
    params(
        ("id" = Uuid, Path, description = "User ID")
    ),
    security(
        ("sessionCookie" = []),
        ("bearerToken" = [])
    ),
    responses(
        (status = 200, description = "User avatar image", content_type = "image/*"),
        (status = 401, description = "Not signed in", body = ErrorBody),
        (status = 404, description = "User or avatar not found", body = ErrorBody)
    )
)]
async fn get_user_avatar(
    State(state): State<AppState>,
    AuthenticatedUser(_viewer): AuthenticatedUser,
    Path(id): Path<Uuid>,
) -> Result<Response, ApiError> {
    let Some(household) = state.household.clone() else {
        return Err(ApiError::not_found("avatar not found"));
    };

    let Some(email) = sqlx::query_scalar::<_, String>("SELECT email FROM users WHERE id = $1")
        .bind(id)
        .fetch_optional(&state.pool)
        .await
        .map_err(|err| {
            tracing::error!(?err, "failed to look up user for avatar");
            ApiError::internal("failed to load avatar")
        })?
    else {
        return Err(ApiError::not_found("avatar not found"));
    };

    let asset = household
        .load_avatar_async(email)
        .await
        .ok_or_else(|| ApiError::not_found("avatar not found"))?;

    Ok((
        StatusCode::OK,
        [
            (CONTENT_TYPE, HeaderValue::from_static(asset.content_type)),
            (
                CACHE_CONTROL,
                HeaderValue::from_static("private, max-age=3600"),
            ),
            (X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")),
        ],
        asset.bytes.clone(),
    )
        .into_response())
}

#[utoipa::path(
    get,
    path = "/api/tokens",
    tag = "tokens",
    security(("sessionCookie" = [])),
    responses(
        (status = 200, description = "Personal access tokens for the current user", body = [ApiToken]),
        (status = 401, description = "Not signed in", body = ErrorBody),
        (status = 403, description = "Browser session required", body = ErrorBody)
    )
)]
async fn list_tokens(
    State(state): State<AppState>,
    SessionUser(user): SessionUser,
) -> Result<Json<Vec<ApiToken>>, ApiError> {
    let tokens = sqlx::query_as::<_, ApiToken>(
        "SELECT id, name, last4, last_used_at, created_at
         FROM api_tokens
         WHERE user_id = $1
         ORDER BY created_at DESC",
    )
    .bind(user.id)
    .fetch_all(&state.pool)
    .await
    .map_err(|err| {
        tracing::error!(?err, "failed to list API tokens");
        ApiError::internal("failed to load API tokens")
    })?;

    Ok(Json(tokens))
}

#[utoipa::path(
    post,
    path = "/api/tokens",
    tag = "tokens",
    security(("sessionCookie" = [])),
    request_body = CreateTokenRequest,
    responses(
        (status = 201, description = "Personal access token minted (plaintext returned once)", body = CreatedApiToken),
        (status = 400, description = "Invalid token name", body = ErrorBody),
        (status = 401, description = "Not signed in", body = ErrorBody),
        (status = 403, description = "Browser session required", body = ErrorBody)
    )
)]
async fn create_token(
    State(state): State<AppState>,
    SessionUser(user): SessionUser,
    Json(payload): Json<CreateTokenRequest>,
) -> Result<(StatusCode, Json<CreatedApiToken>), ApiError> {
    let name = payload.name.trim();
    if name.is_empty() || name.chars().count() > 80 {
        return Err(ApiError::bad_request(
            "token name must be between 1 and 80 characters",
        ));
    }

    let id = Uuid::new_v4();
    let secret = random_hex_secret();
    let secret_hash = sha256_bytes(&secret);
    let last4 = secret[secret.len() - 4..].to_string();
    let plaintext = format!("pat_{id}_{secret}");

    let created_at: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
        "INSERT INTO api_tokens (id, user_id, name, secret_hash, last4)
         VALUES ($1, $2, $3, $4, $5)
         RETURNING created_at",
    )
    .bind(id)
    .bind(user.id)
    .bind(name)
    .bind(secret_hash)
    .bind(&last4)
    .fetch_one(&state.pool)
    .await
    .map_err(|err| {
        tracing::error!(?err, "failed to insert API token");
        ApiError::internal("failed to create API token")
    })?;

    Ok((
        StatusCode::CREATED,
        Json(CreatedApiToken {
            id,
            name: name.to_string(),
            token: plaintext,
            last4,
            created_at,
        }),
    ))
}

#[utoipa::path(
    delete,
    path = "/api/tokens/{id}",
    tag = "tokens",
    params(
        ("id" = Uuid, Path, description = "Personal access token ID")
    ),
    security(("sessionCookie" = [])),
    responses(
        (status = 204, description = "Token revoked"),
        (status = 401, description = "Not signed in", body = ErrorBody),
        (status = 403, description = "Browser session required", body = ErrorBody),
        (status = 404, description = "Token not found", body = ErrorBody)
    )
)]
async fn delete_token(
    State(state): State<AppState>,
    SessionUser(user): SessionUser,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let result = sqlx::query("DELETE FROM api_tokens WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(user.id)
        .execute(&state.pool)
        .await
        .map_err(|err| {
            tracing::error!(?err, "failed to delete API token");
            ApiError::internal("failed to revoke API token")
        })?;

    if result.rows_affected() == 0 {
        return Err(ApiError::not_found("token not found"));
    }
    Ok(StatusCode::NO_CONTENT)
}
