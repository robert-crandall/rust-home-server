use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};
use url::Url;
use uuid::Uuid;

use crate::auth::{CreatedApiToken, User};
use crate::routes::{CreateTokenRequest, ErrorBody, LoginRequest};

pub const APP_SLUG: &str = "rust-home-server";
const CF_ACCESS_CLIENT_ID_HEADER: HeaderName = HeaderName::from_static("cf-access-client-id");
const CF_ACCESS_CLIENT_SECRET_HEADER: HeaderName =
    HeaderName::from_static("cf-access-client-secret");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientConfig {
    pub app_url: String,
    pub token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cf_access_client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cf_access_client_secret: Option<String>,
}

impl ClientConfig {
    pub fn default_path() -> Result<PathBuf, String> {
        let home = env::var("HOME")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| "HOME environment variable is not set".to_string())?;
        Ok(PathBuf::from(home)
            .join(".config")
            .join(format!("{APP_SLUG}.json")))
    }

    pub fn load(config_path: Option<&Path>) -> Result<Self, String> {
        Self::load_with_env(
            config_path,
            env_non_empty("MCP_APP_URL"),
            env_non_empty("MCP_APP_TOKEN"),
            env_non_empty("MCP_CF_ACCESS_CLIENT_ID"),
            env_non_empty("MCP_CF_ACCESS_CLIENT_SECRET"),
        )
    }

    fn load_with_env(
        config_path: Option<&Path>,
        env_url: Option<String>,
        env_token: Option<String>,
        env_cf_id: Option<String>,
        env_cf_secret: Option<String>,
    ) -> Result<Self, String> {
        let file_cfg = {
            let resolved_path = match config_path {
                Some(p) => Some(p.to_path_buf()),
                None => match Self::default_path() {
                    Ok(p) => Some(p),
                    Err(_) if env_url.is_some() && env_token.is_some() => None,
                    Err(err) => return Err(err),
                },
            };
            match resolved_path {
                Some(path) if path.exists() => {
                    let raw = fs::read_to_string(&path)
                        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
                    let parsed: ClientConfigFile = serde_json::from_str(&raw)
                        .map_err(|err| format!("invalid config in {}: {err}", path.display()))?;
                    Some(parsed)
                }
                _ => None,
            }
        };

        let app_url = env_url
            .as_deref()
            .or_else(|| file_cfg.as_ref().and_then(|c| c.app_url.as_deref()))
            .map(|v| v.trim().trim_end_matches('/').to_string())
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                "missing appUrl (set MCP_APP_URL or run `make mcp-token` to write ~/.config/rust-home-server.json)".to_string()
            })?;

        let parsed_app_url = validate_base_url(&app_url)?;

        let token = env_token
            .or_else(|| file_cfg.as_ref().and_then(|c| c.token.clone()))
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                "missing token (set MCP_APP_TOKEN or run `make mcp-token` to write ~/.config/rust-home-server.json)".to_string()
            })?;

        let (raw_cf_id, raw_cf_secret) = if env_cf_id.is_some() || env_cf_secret.is_some() {
            (env_cf_id, env_cf_secret)
        } else {
            let allow_file_cf = config_path.is_some()
                || env_url.is_none()
                || file_cfg
                    .as_ref()
                    .and_then(|c| c.app_url.as_deref())
                    .and_then(|raw| validate_base_url(raw.trim().trim_end_matches('/')).ok())
                    .is_some_and(|file_url| file_url.origin() == parsed_app_url.origin());
            if allow_file_cf {
                (
                    file_cfg
                        .as_ref()
                        .and_then(|c| c.cf_access_client_id.clone()),
                    file_cfg
                        .as_ref()
                        .and_then(|c| c.cf_access_client_secret.clone()),
                )
            } else {
                (None, None)
            }
        };
        let (cf_access_client_id, cf_access_client_secret) =
            normalize_cf_service_token(raw_cf_id.as_deref(), raw_cf_secret.as_deref())?;

        Ok(Self {
            app_url,
            token,
            cf_access_client_id,
            cf_access_client_secret,
        })
    }

    pub fn write_atomic(&self, path: &Path) -> Result<(), String> {
        validate_base_url(&self.app_url)?;
        normalize_cf_service_token(
            self.cf_access_client_id.as_deref(),
            self.cf_access_client_secret.as_deref(),
        )?;
        let parent = path
            .parent()
            .ok_or_else(|| format!("invalid config path {}", path.display()))?;
        fs::create_dir_all(parent)
            .map_err(|err| format!("failed to create {}: {err}", parent.display()))?;

        let tmp_path = parent.join(format!(
            ".{}.tmp.{}",
            path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("rust-home-server.json"),
            Uuid::new_v4()
        ));

        let mut body = serde_json::to_string_pretty(self)
            .map_err(|err| format!("failed to serialize config: {err}"))?;
        body.push('\n');

        write_mode_0600(&tmp_path, body.as_bytes())?;
        fs::rename(&tmp_path, path).map_err(|err| {
            let _ = fs::remove_file(&tmp_path);
            format!("failed to move config into {}: {err}", path.display())
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ClientConfigFile {
    app_url: Option<String>,
    token: Option<String>,
    cf_access_client_id: Option<String>,
    cf_access_client_secret: Option<String>,
}

fn env_non_empty(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

pub fn normalize_cf_service_token(
    client_id: Option<&str>,
    client_secret: Option<&str>,
) -> Result<(Option<String>, Option<String>), String> {
    let id = client_id
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(ToString::to_string);
    let secret = client_secret
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(ToString::to_string);

    match (id, secret) {
        (Some(id), Some(secret)) => {
            HeaderValue::from_str(&id).map_err(|_| {
                "cfAccessClientId contains invalid HTTP header characters".to_string()
            })?;
            HeaderValue::from_str(&secret).map_err(|_| {
                "cfAccessClientSecret contains invalid HTTP header characters".to_string()
            })?;
            Ok((Some(id), Some(secret)))
        }
        (None, None) => Ok((None, None)),
        _ => Err(
            "cfAccessClientId and cfAccessClientSecret must either both be set or both be unset"
                .to_string(),
        ),
    }
}

fn insert_cf_service_headers(
    headers: &mut HeaderMap,
    client_id: Option<&str>,
    client_secret: Option<&str>,
) -> Result<(), String> {
    if let (Some(id), Some(secret)) = normalize_cf_service_token(client_id, client_secret)? {
        headers.insert(
            CF_ACCESS_CLIENT_ID_HEADER,
            HeaderValue::from_str(&id)
                .map_err(|_| "cfAccessClientId contains invalid HTTP header characters")?,
        );
        headers.insert(
            CF_ACCESS_CLIENT_SECRET_HEADER,
            HeaderValue::from_str(&secret)
                .map_err(|_| "cfAccessClientSecret contains invalid HTTP header characters")?,
        );
    }
    Ok(())
}

fn validate_base_url(raw: &str) -> Result<Url, String> {
    let parsed = Url::parse(raw).map_err(|err| format!("invalid appUrl '{raw}': {err}"))?;
    match parsed.scheme() {
        "http" | "https" => {}
        other => {
            return Err(format!(
                "appUrl must use http:// or https:// (got '{other}://')"
            ));
        }
    }
    if parsed.query().is_some()
        || parsed.fragment().is_some()
        || raw.contains('?')
        || raw.contains('#')
    {
        return Err("appUrl must not include a query string or fragment".to_string());
    }
    Ok(parsed)
}

#[cfg(unix)]
fn write_mode_0600(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|err| format!("failed to create {}: {err}", path.display()))?;
    file.write_all(bytes)
        .map_err(|err| format!("failed to write {}: {err}", path.display()))?;
    file.sync_all()
        .map_err(|err| format!("failed to sync {}: {err}", path.display()))
}

#[cfg(not(unix))]
fn write_mode_0600(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|err| format!("failed to create {}: {err}", path.display()))?;
    file.write_all(bytes)
        .map_err(|err| format!("failed to write {}: {err}", path.display()))?;
    file.sync_all()
        .map_err(|err| format!("failed to sync {}: {err}", path.display()))
}

#[derive(Clone)]
pub struct ApiClient {
    base_url: String,
    http: reqwest::Client,
}

impl ApiClient {
    pub fn new(cfg: &ClientConfig) -> Result<Self, String> {
        validate_base_url(&cfg.app_url)?;
        let mut headers = HeaderMap::new();
        let bearer = HeaderValue::from_str(&format!("Bearer {}", cfg.token))
            .map_err(|_| "token contains invalid HTTP header characters".to_string())?;
        headers.insert(AUTHORIZATION, bearer);
        insert_cf_service_headers(
            &mut headers,
            cfg.cf_access_client_id.as_deref(),
            cfg.cf_access_client_secret.as_deref(),
        )?;

        let http = reqwest::Client::builder()
            .default_headers(headers)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|err| format!("failed to build HTTP client: {err}"))?;

        Ok(Self {
            base_url: cfg.app_url.trim_end_matches('/').to_string(),
            http,
        })
    }

    pub async fn me(&self) -> Result<User, String> {
        let resp = self
            .http
            .get(format!("{}/api/auth/me", self.base_url))
            .send()
            .await
            .map_err(|err| format!("request to /api/auth/me failed: {err}"))?;

        if !resp.status().is_success() {
            return Err(read_api_error(resp).await);
        }

        resp.json::<User>()
            .await
            .map_err(|err| format!("invalid /api/auth/me response: {err}"))
    }
}

pub async fn mint_token_via_login(
    app_url: &str,
    email: &str,
    password: &str,
    token_name: &str,
    cf_access_client_id: Option<&str>,
    cf_access_client_secret: Option<&str>,
) -> Result<CreatedApiToken, String> {
    let base = app_url.trim().trim_end_matches('/');
    validate_base_url(base)?;

    let mut headers = HeaderMap::new();
    insert_cf_service_headers(&mut headers, cf_access_client_id, cf_access_client_secret)?;

    let client = reqwest::Client::builder()
        .default_headers(headers)
        .redirect(reqwest::redirect::Policy::none())
        .cookie_store(true)
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|err| format!("failed to build HTTP client: {err}"))?;

    let login_resp = client
        .post(format!("{base}/api/auth/login"))
        .json(&LoginRequest {
            email: email.to_string(),
            password: password.to_string(),
        })
        .send()
        .await
        .map_err(|err| format!("login request failed: {err}"))?;

    if !login_resp.status().is_success() {
        return Err(read_api_error(login_resp).await);
    }

    let token_resp = client
        .post(format!("{base}/api/tokens"))
        .json(&CreateTokenRequest {
            name: token_name.to_string(),
        })
        .send()
        .await
        .map_err(|err| format!("token creation request failed: {err}"))?;

    if !token_resp.status().is_success() {
        return Err(read_api_error(token_resp).await);
    }

    token_resp
        .json::<CreatedApiToken>()
        .await
        .map_err(|err| format!("invalid /api/tokens response: {err}"))
}

async fn read_api_error(resp: reqwest::Response) -> String {
    let status = resp.status();
    if status.is_redirection() {
        return format!(
            "HTTP {} redirect (if behind Cloudflare Access, verify your Service Auth policy and cfAccessClientId/cfAccessClientSecret)",
            status.as_u16()
        );
    }
    if let Ok(err_body) = resp.json::<ErrorBody>().await {
        return format!("HTTP {}: {}", status.as_u16(), err_body.error);
    }
    format!("HTTP {}", status.as_u16())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unknown_fields_in_config_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rust-home-server.json");
        fs::write(
            &path,
            r#"{"appUrl":"http://localhost:8080","token":"pat_x_y","typo":true}"#,
        )
        .unwrap();

        let err = ClientConfig::load(Some(&path)).unwrap_err();
        assert!(err.contains("unknown field `typo`"), "got: {err}");
    }

    #[test]
    fn writes_atomic_0600_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("rust-home-server.json");
        let cfg = ClientConfig {
            app_url: "http://127.0.0.1:8080".to_string(),
            token: "pat_00000000-0000-0000-0000-000000000000_secret".to_string(),
            cf_access_client_id: None,
            cf_access_client_secret: None,
        };
        cfg.write_atomic(&path).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        let raw = fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("cfAccessClientId"));

        // Overwriting an existing config file atomically preserves 0600 mode,
        // while `write_mode_0600` itself refuses pre-existing temp paths/symlinks.
        let updated = ClientConfig {
            app_url: "http://127.0.0.1:8080".to_string(),
            token: "pat_00000000-0000-0000-0000-000000000000_rotated".to_string(),
            cf_access_client_id: Some("cf-id-123.access".to_string()),
            cf_access_client_secret: Some("cf-secret-456".to_string()),
        };
        updated.write_atomic(&path).unwrap();
        assert_eq!(ClientConfig::load(Some(&path)).unwrap(), updated);

        let existing = dir.path().join("preexisting.tmp");
        fs::write(&existing, b"stale").unwrap();
        assert!(write_mode_0600(&existing, b"secret").is_err());
    }

    #[test]
    fn rejects_partial_cf_service_token() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rust-home-server.json");
        fs::write(
            &path,
            r#"{"appUrl":"https://app.example.com","token":"pat_x_y","cfAccessClientId":"only-id"}"#,
        )
        .unwrap();

        let err = ClientConfig::load(Some(&path)).unwrap_err();
        assert!(err.contains("both be set or both be unset"), "got: {err}");

        // Env overrides for URL + token still inherit CF creds from explicit config
        let full_path = dir.path().join("full.json");
        fs::write(
            &full_path,
            r#"{"appUrl":"https://app.example.com","token":"pat_old","cfAccessClientId":"id-1","cfAccessClientSecret":"sec-1"}"#,
        )
        .unwrap();
        let inherited = ClientConfig::load_with_env(
            Some(&full_path),
            Some("https://app.example.com".to_string()),
            Some("pat_new".to_string()),
            None,
            None,
        )
        .unwrap();
        assert_eq!(inherited.token, "pat_new");
        assert_eq!(inherited.cf_access_client_id.as_deref(), Some("id-1"));
        assert_eq!(inherited.cf_access_client_secret.as_deref(), Some("sec-1"));

        let half_cfg = ClientConfig {
            app_url: "https://app.example.com".to_string(),
            token: "pat_x_y".to_string(),
            cf_access_client_id: None,
            cf_access_client_secret: Some("only-secret".to_string()),
        };
        assert!(ApiClient::new(&half_cfg).is_err());
    }

    #[tokio::test]
    async fn api_client_sends_cf_service_token_and_bearer_headers() {
        use axum::routing::get;
        use axum::{Json, Router};

        let app = Router::new().route(
            "/api/auth/me",
            get(|headers: axum::http::HeaderMap| async move {
                assert_eq!(
                    headers
                        .get("cf-access-client-id")
                        .and_then(|v| v.to_str().ok()),
                    Some("svc-id-123.access")
                );
                assert_eq!(
                    headers
                        .get("cf-access-client-secret")
                        .and_then(|v| v.to_str().ok()),
                    Some("svc-secret-456")
                );
                assert_eq!(
                    headers.get("authorization").and_then(|v| v.to_str().ok()),
                    Some("Bearer pat_00000000-0000-0000-0000-000000000000_secret")
                );
                Json(User {
                    id: Uuid::nil(),
                    email: "robert@example.com".to_string(),
                    name: "Robert".to_string(),
                    avatar_url: None,
                    created_at: chrono::Utc::now(),
                })
            }),
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let cfg = ClientConfig {
            app_url: format!("http://{addr}"),
            token: "pat_00000000-0000-0000-0000-000000000000_secret".to_string(),
            cf_access_client_id: Some("svc-id-123.access".to_string()),
            cf_access_client_secret: Some("svc-secret-456".to_string()),
        };
        let client = ApiClient::new(&cfg).unwrap();
        let me = client.me().await.unwrap();
        assert_eq!(me.email, "robert@example.com");
        server.abort();
    }

    #[tokio::test]
    async fn api_client_does_not_follow_redirects_with_cf_secret() {
        use axum::Router;
        use axum::http::StatusCode;
        use axum::response::Redirect;
        use axum::routing::get;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let leaked = Arc::new(AtomicBool::new(false));
        let leaked_target = leaked.clone();
        let target_app = Router::new().route(
            "/leaked",
            get(move || async move {
                leaked_target.store(true, Ordering::SeqCst);
                StatusCode::OK
            }),
        );
        let target_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_addr = target_listener.local_addr().unwrap();
        let target_server = tokio::spawn(async move {
            let _ = axum::serve(target_listener, target_app).await;
        });

        let redirect_url = format!("http://{target_addr}/leaked");
        let origin_app = Router::new().route(
            "/api/auth/me",
            get(move || async move { Redirect::temporary(&redirect_url) }),
        );
        let origin_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin_addr = origin_listener.local_addr().unwrap();
        let origin_server = tokio::spawn(async move {
            let _ = axum::serve(origin_listener, origin_app).await;
        });

        let cfg = ClientConfig {
            app_url: format!("http://{origin_addr}"),
            token: "pat_00000000-0000-0000-0000-000000000000_secret".to_string(),
            cf_access_client_id: Some("svc-id.access".to_string()),
            cf_access_client_secret: Some("svc-secret".to_string()),
        };
        let err = ApiClient::new(&cfg).unwrap().me().await.unwrap_err();
        assert!(err.contains("redirect"), "got: {err}");
        assert!(
            !leaked.load(Ordering::SeqCst),
            "redirect target must not be called"
        );

        origin_server.abort();
        target_server.abort();
    }

    #[tokio::test]
    async fn mint_token_via_login_sends_cf_headers_and_rejects_redirects() {
        use axum::http::StatusCode;
        use axum::response::Redirect;
        use axum::routing::post;
        use axum::{Json, Router};
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let app = Router::new()
            .route(
                "/api/auth/login",
                post(|headers: axum::http::HeaderMap| async move {
                    assert_eq!(
                        headers
                            .get("cf-access-client-id")
                            .and_then(|v| v.to_str().ok()),
                        Some("login-id.access")
                    );
                    assert_eq!(
                        headers
                            .get("cf-access-client-secret")
                            .and_then(|v| v.to_str().ok()),
                        Some("login-secret")
                    );
                    StatusCode::OK
                }),
            )
            .route(
                "/api/tokens",
                post(|headers: axum::http::HeaderMap| async move {
                    assert_eq!(
                        headers
                            .get("cf-access-client-id")
                            .and_then(|v| v.to_str().ok()),
                        Some("login-id.access")
                    );
                    assert_eq!(
                        headers
                            .get("cf-access-client-secret")
                            .and_then(|v| v.to_str().ok()),
                        Some("login-secret")
                    );
                    (
                        StatusCode::CREATED,
                        Json(CreatedApiToken {
                            id: Uuid::nil(),
                            name: "mcp".to_string(),
                            token: "pat_00000000-0000-0000-0000-000000000000_minted".to_string(),
                            last4: "nted".to_string(),
                            created_at: chrono::Utc::now(),
                        }),
                    )
                }),
            );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let created = mint_token_via_login(
            &format!("http://{addr}"),
            "robert@example.com",
            "pass",
            "mcp",
            Some("login-id.access"),
            Some("login-secret"),
        )
        .await
        .unwrap();
        assert_eq!(created.last4, "nted");
        server.abort();

        let leaked = Arc::new(AtomicBool::new(false));
        let leaked_target = leaked.clone();
        let target_app = Router::new().route(
            "/leaked",
            post(move || async move {
                leaked_target.store(true, Ordering::SeqCst);
                StatusCode::OK
            })
            .get({
                let leaked_get = leaked.clone();
                move || async move {
                    leaked_get.store(true, Ordering::SeqCst);
                    StatusCode::OK
                }
            }),
        );
        let target_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_addr = target_listener.local_addr().unwrap();
        let target_server = tokio::spawn(async move {
            let _ = axum::serve(target_listener, target_app).await;
        });

        let redirect_url = format!("http://{target_addr}/leaked");
        let redirect_login_url = redirect_url.clone();
        let redirect_token_url = redirect_url.clone();
        let redir_app = Router::new()
            .route(
                "/api/auth/login",
                post(move || async move { Redirect::temporary(&redirect_login_url) }),
            )
            .route(
                "/api/tokens",
                post(move || async move { Redirect::temporary(&redirect_token_url) }),
            );
        let redir_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let redir_addr = redir_listener.local_addr().unwrap();
        let redir_server = tokio::spawn(async move {
            let _ = axum::serve(redir_listener, redir_app).await;
        });

        let err = mint_token_via_login(
            &format!("http://{redir_addr}"),
            "robert@example.com",
            "pass",
            "mcp",
            Some("login-id.access"),
            Some("login-secret"),
        )
        .await
        .unwrap_err();
        assert!(err.contains("redirect"), "got: {err}");
        assert!(!leaked.load(Ordering::SeqCst));

        redir_server.abort();
        target_server.abort();
    }

    #[test]
    fn rejects_app_urls_with_queries_or_fragments() {
        for bad in [
            "http://localhost:8080/#focus",
            "http://localhost:8080?view=focus",
            "https://rust-home-server.home/?",
            "https://rust-home-server.home/#",
        ] {
            let cfg = ClientConfig {
                app_url: bad.to_string(),
                token: "pat_00000000-0000-0000-0000-000000000000_secret".to_string(),
                cf_access_client_id: None,
                cf_access_client_secret: None,
            };
            assert!(ApiClient::new(&cfg).is_err(), "expected error for {bad}");
        }
    }
}
