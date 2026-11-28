use std::process::ExitCode;
use std::time::Duration;

use app::auth::CfAccessAuth;
use app::config::{Config, healthcheck_url_from_env};
use app::db::connect_and_migrate;
use app::household::HouseholdProfiles;
use app::routes::{AppState, ErrorBody, build_router};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::http::{HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use rust_embed::Embed;
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

#[derive(Embed)]
#[folder = "web/build"]
struct WebAssets;

#[tokio::main]
async fn main() -> ExitCode {
    if matches!(std::env::args().nth(1).as_deref(), Some("healthcheck")) {
        return run_healthcheck().await;
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cfg = match Config::from_env() {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("configuration error: {err}");
            return ExitCode::FAILURE;
        }
    };

    let pool = match connect_and_migrate(&cfg.database_url).await {
        Ok(pool) => pool,
        Err(err) => {
            eprintln!("database startup failed: {err}");
            return ExitCode::FAILURE;
        }
    };

    let app = build_router(AppState {
        pool,
        secure_cookies: cfg.secure_cookies,
        allow_open_registration: cfg.allow_open_registration,
        cf_access: cfg.cf_access.map(CfAccessAuth::new),
        household: cfg.household_users_file.map(HouseholdProfiles::new),
    })
    .fallback(serve_spa);

    let listener = match TcpListener::bind(cfg.listen_addr).await {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("failed to bind {}: {err}", cfg.listen_addr);
            return ExitCode::FAILURE;
        }
    };

    tracing::info!(addr = %cfg.listen_addr, "rust-home-server server listening");

    if let Err(err) = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
    {
        eprintln!("server error: {err}");
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}

async fn run_healthcheck() -> ExitCode {
    let url = match healthcheck_url_from_env() {
        Ok(url) => url,
        Err(err) => {
            eprintln!("healthcheck config error: {err}");
            return ExitCode::FAILURE;
        }
    };

    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(4))
        .build()
    {
        Ok(c) => c,
        Err(err) => {
            eprintln!("healthcheck client error: {err}");
            return ExitCode::FAILURE;
        }
    };

    match client.get(&url).send().await {
        Ok(resp) if resp.status().is_success() => ExitCode::SUCCESS,
        Ok(resp) => {
            eprintln!("healthcheck returned HTTP {}", resp.status());
            ExitCode::FAILURE
        }
        Err(err) => {
            eprintln!("healthcheck failed for {url}: {err}");
            ExitCode::FAILURE
        }
    }
}

async fn serve_spa(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');

    if path == "api" || path.starts_with("api/") {
        return (
            StatusCode::NOT_FOUND,
            axum::Json(ErrorBody {
                error: "not found".to_string(),
            }),
        )
            .into_response();
    }

    if !path.is_empty()
        && let Some(asset) = WebAssets::get(path)
    {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        let cache = if path.starts_with("assets/") {
            "public, max-age=31536000, immutable"
        } else {
            "no-cache"
        };
        return (
            StatusCode::OK,
            [
                (
                    CONTENT_TYPE,
                    HeaderValue::from_str(mime.as_ref())
                        .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
                ),
                (CACHE_CONTROL, HeaderValue::from_static(cache)),
            ],
            asset.data.into_owned(),
        )
            .into_response();
    }

    // Dotted paths that missed WebAssets are missing static files, not SPA routes.
    if path.rsplit('/').next().is_some_and(|seg| seg.contains('.')) {
        return StatusCode::NOT_FOUND.into_response();
    }

    match WebAssets::get("index.html") {
        Some(index) => (
            StatusCode::OK,
            [
                (
                    CONTENT_TYPE,
                    HeaderValue::from_static("text/html; charset=utf-8"),
                ),
                (CACHE_CONTROL, HeaderValue::from_static("no-cache")),
            ],
            index.data.into_owned(),
        )
            .into_response(),
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            "frontend bundle not built (run `cd web && bun run build`)",
        )
            .into_response(),
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut sigterm) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            sigterm.recv().await;
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
        if bytes.len() < 24 || &bytes[0..8] != b"\x89PNG\r\n\x1a\n" || &bytes[12..16] != b"IHDR" {
            return None;
        }
        let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
        let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
        Some((width, height))
    }

    #[test]
    fn embedded_spa_has_valid_pwa_manifest_and_icons() {
        let index = WebAssets::get("index.html").expect("index.html embedded");
        let html = std::str::from_utf8(index.data.as_ref()).expect("index.html UTF-8");
        assert!(
            html.contains(
                r#"rel="manifest" href="/manifest.webmanifest" crossorigin="use-credentials""#
            ),
            "index.html must link /manifest.webmanifest with use-credentials"
        );
        assert!(
            html.contains(r#"rel="apple-touch-icon" href="/icon-192.png""#),
            "index.html must link /icon-192.png as apple-touch-icon"
        );
        assert!(
            html.contains(r#"rel="icon" type="image/png" sizes="192x192" href="/icon-192.png""#),
            "index.html must link /icon-192.png as favicon"
        );

        let manifest_asset =
            WebAssets::get("manifest.webmanifest").expect("manifest.webmanifest embedded");
        let manifest: serde_json::Value =
            serde_json::from_slice(manifest_asset.data.as_ref()).expect("valid manifest JSON");
        assert!(
            manifest["name"].as_str().is_some_and(|s| !s.is_empty()),
            "manifest must have a non-empty name"
        );
        assert_eq!(manifest["display"].as_str(), Some("standalone"));

        let icons = manifest["icons"]
            .as_array()
            .expect("manifest must declare icons");
        assert!(icons.len() >= 2, "manifest must declare 192 and 512 icons");
        for icon in icons {
            let src = icon["src"]
                .as_str()
                .expect("icon src")
                .trim_start_matches('/');
            let declared = icon["sizes"].as_str().expect("icon sizes");
            let file = WebAssets::get(src)
                .unwrap_or_else(|| panic!("manifest icon {src} missing from embedded build"));
            let (w, h) = png_dimensions(file.data.as_ref())
                .unwrap_or_else(|| panic!("manifest icon {src} is not a valid PNG"));
            assert_eq!(
                format!("{w}x{h}"),
                declared,
                "icon {src} pixel size mismatch"
            );
        }
    }

    #[tokio::test]
    async fn serve_spa_serves_pwa_manifest_and_icons_with_expected_headers() {
        let manifest = serve_spa(Uri::from_static("/manifest.webmanifest")).await;
        assert_eq!(manifest.status(), StatusCode::OK);
        assert_eq!(
            manifest.headers()[CONTENT_TYPE],
            "application/manifest+json"
        );
        assert_eq!(manifest.headers()[CACHE_CONTROL], "no-cache");

        let icon = serve_spa(Uri::from_static("/icon-192.png")).await;
        assert_eq!(icon.status(), StatusCode::OK);
        assert_eq!(icon.headers()[CONTENT_TYPE], "image/png");
        assert_eq!(icon.headers()[CACHE_CONTROL], "no-cache");

        let missing = serve_spa(Uri::from_static("/missing-icon.png")).await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    }
}
