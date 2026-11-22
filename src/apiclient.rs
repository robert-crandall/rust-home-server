use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use url::Url;
use uuid::Uuid;

use crate::auth::{CreatedApiToken, User};
use crate::routes::{CreateTokenRequest, ErrorBody, LoginRequest};

pub const APP_SLUG: &str = "rust-home-template";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientConfig {
    pub app_url: String,
    pub token: String,
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
        let env_url = env::var("MCP_APP_URL")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        let env_token = env::var("MCP_APP_TOKEN")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());

        let file_cfg = if env_url.is_some() && env_token.is_some() {
            None
        } else {
            let path = match config_path {
                Some(p) => p.to_path_buf(),
                None => Self::default_path()?,
            };
            if path.exists() {
                let raw = fs::read_to_string(&path)
                    .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
                let parsed: ClientConfigFile = serde_json::from_str(&raw)
                    .map_err(|err| format!("invalid config in {}: {err}", path.display()))?;
                Some(parsed)
            } else {
                None
            }
        };

        let app_url = env_url
            .or_else(|| file_cfg.as_ref().and_then(|c| c.app_url.clone()))
            .map(|v| v.trim().trim_end_matches('/').to_string())
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                "missing appUrl (set MCP_APP_URL or run `make mcp-token` to write ~/.config/rust-home-template.json)".to_string()
            })?;

        validate_base_url(&app_url)?;

        let token = env_token
            .or_else(|| file_cfg.and_then(|c| c.token))
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                "missing token (set MCP_APP_TOKEN or run `make mcp-token` to write ~/.config/rust-home-template.json)".to_string()
            })?;

        Ok(Self { app_url, token })
    }

    pub fn write_atomic(&self, path: &Path) -> Result<(), String> {
        validate_base_url(&self.app_url)?;
        let parent = path
            .parent()
            .ok_or_else(|| format!("invalid config path {}", path.display()))?;
        fs::create_dir_all(parent)
            .map_err(|err| format!("failed to create {}: {err}", parent.display()))?;

        let tmp_path = parent.join(format!(
            ".{}.tmp.{}",
            path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("rust-home-template.json"),
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

        let http = reqwest::Client::builder()
            .default_headers(headers)
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

    pub async fn list_users(&self) -> Result<Vec<User>, String> {
        let resp = self
            .http
            .get(format!("{}/api/users", self.base_url))
            .send()
            .await
            .map_err(|err| format!("request to /api/users failed: {err}"))?;

        if !resp.status().is_success() {
            return Err(read_api_error(resp).await);
        }

        resp.json::<Vec<User>>()
            .await
            .map_err(|err| format!("invalid /api/users response: {err}"))
    }
}

pub async fn mint_token_via_login(
    app_url: &str,
    email: &str,
    password: &str,
    token_name: &str,
) -> Result<CreatedApiToken, String> {
    let base = app_url.trim().trim_end_matches('/');
    validate_base_url(base)?;

    let client = reqwest::Client::builder()
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
        let path = dir.path().join("rust-home-template.json");
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
        let path = dir.path().join("nested").join("rust-home-template.json");
        let cfg = ClientConfig {
            app_url: "http://127.0.0.1:8080".to_string(),
            token: "pat_00000000-0000-0000-0000-000000000000_secret".to_string(),
        };
        cfg.write_atomic(&path).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        // Overwriting an existing config file atomically preserves 0600 mode,
        // while `write_mode_0600` itself refuses pre-existing temp paths/symlinks.
        let updated = ClientConfig {
            app_url: "http://127.0.0.1:8080".to_string(),
            token: "pat_00000000-0000-0000-0000-000000000000_rotated".to_string(),
        };
        updated.write_atomic(&path).unwrap();
        assert_eq!(ClientConfig::load(Some(&path)).unwrap(), updated);

        let existing = dir.path().join("preexisting.tmp");
        fs::write(&existing, b"stale").unwrap();
        assert!(write_mode_0600(&existing, b"secret").is_err());
    }

    #[test]
    fn rejects_app_urls_with_queries_or_fragments() {
        for bad in [
            "http://localhost:8080/#focus",
            "http://localhost:8080?view=focus",
            "https://rust-home-template.home/?",
            "https://rust-home-template.home/#",
        ] {
            let cfg = ClientConfig {
                app_url: bad.to_string(),
                token: "pat_00000000-0000-0000-0000-000000000000_secret".to_string(),
            };
            assert!(ApiClient::new(&cfg).is_err(), "expected error for {bad}");
        }
    }
}
