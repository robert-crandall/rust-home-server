use std::collections::BTreeMap;
use std::net::TcpListener as StdTcpListener;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use app::apiclient::{ClientConfig, mint_token_via_login};
use app::db;
use app::routes::{self, AppState, build_router};
use reqwest::StatusCode;
use reqwest::header::{AUTHORIZATION, COOKIE, ORIGIN};
use serde_json::{Value, json};
use sqlx::{Connection, Executor, PgConnection, PgPool};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::process::Command;
use uuid::Uuid;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn openapi_spec_matches_committed_file() {
    let mut generated = routes::openapi_spec()
        .to_pretty_json()
        .expect("generate openapi json");
    generated.push('\n');
    let committed_path = repo_root().join("docs/openapi.json");
    let committed = std::fs::read_to_string(&committed_path).expect("read docs/openapi.json");
    assert_eq!(
        generated, committed,
        "docs/openapi.json is out of date; run `make spec`"
    );
}

#[test]
fn bun_lockfile_uses_default_registry_resolution() {
    let lock_path = repo_root().join("web/bun.lock");
    let lock = std::fs::read_to_string(&lock_path).expect("read web/bun.lock");
    assert!(
        !lock.contains("https://"),
        "web/bun.lock pins explicit tarball URLs; strip private registry URLs to \"\" so CI and Docker can resolve packages"
    );
}

#[test]
fn publish_plan_script_matrix() {
    let sha = "0123456789abcdef0123456789abcdef01234567";
    let other_sha = "fedcba9876543210fedcba9876543210fedcba98";

    struct Case {
        name: &'static str,
        env: Vec<(&'static str, &'static str)>,
        want_publish: &'static str,
        want_promote: &'static str,
        want_ref: &'static str,
        want_sha_tag: &'static str,
        want_version: &'static str,
    }

    let cases = vec![
        Case {
            name: "green push CI at main tip publishes and promotes",
            env: vec![
                ("EVENT_NAME", "workflow_run"),
                ("REPOSITORY", "Robert-Crandall/Rust-Home-Server"),
                ("RUN_EVENT", "push"),
                ("RUN_CONCLUSION", "success"),
                ("RUN_HEAD_SHA", sha),
                ("BRANCH_TIP", sha),
            ],
            want_publish: "true",
            want_promote: "true",
            want_ref: sha,
            want_sha_tag: "sha-0123456789abcdef0123456789abcdef01234567",
            want_version: "",
        },
        Case {
            name: "pull_request CI run never publishes even if tip matches",
            env: vec![
                ("EVENT_NAME", "workflow_run"),
                ("REPOSITORY", "robert-crandall/rust-home-server"),
                ("RUN_EVENT", "pull_request"),
                ("RUN_CONCLUSION", "success"),
                ("RUN_HEAD_SHA", sha),
                ("BRANCH_TIP", sha),
            ],
            want_publish: "false",
            want_promote: "false",
            want_ref: "",
            want_sha_tag: "",
            want_version: "",
        },
        Case {
            name: "stale main CI run declines when main moved",
            env: vec![
                ("EVENT_NAME", "workflow_run"),
                ("REPOSITORY", "robert-crandall/rust-home-server"),
                ("RUN_EVENT", "push"),
                ("RUN_CONCLUSION", "success"),
                ("RUN_HEAD_SHA", sha),
                ("BRANCH_TIP", other_sha),
            ],
            want_publish: "false",
            want_promote: "false",
            want_ref: "",
            want_sha_tag: "",
            want_version: "",
        },
        Case {
            name: "failed CI run declines",
            env: vec![
                ("EVENT_NAME", "workflow_run"),
                ("REPOSITORY", "robert-crandall/rust-home-server"),
                ("RUN_EVENT", "push"),
                ("RUN_CONCLUSION", "failure"),
                ("RUN_HEAD_SHA", sha),
                ("BRANCH_TIP", sha),
            ],
            want_publish: "false",
            want_promote: "false",
            want_ref: "",
            want_sha_tag: "",
            want_version: "",
        },
        Case {
            name: "version tag push publishes immutable tag without moving main",
            env: vec![
                ("EVENT_NAME", "push"),
                ("REPOSITORY", "robert-crandall/rust-home-server"),
                ("GITHUB_REF", "refs/tags/v0.2.0"),
                ("SHA", sha),
            ],
            want_publish: "true",
            want_promote: "false",
            want_ref: sha,
            want_sha_tag: "",
            want_version: "v0.2.0",
        },
        Case {
            name: "workflow_dispatch on main publishes and promotes",
            env: vec![
                ("EVENT_NAME", "workflow_dispatch"),
                ("REPOSITORY", "robert-crandall/rust-home-server"),
                ("GITHUB_REF", "refs/heads/main"),
                ("SHA", sha),
            ],
            want_publish: "true",
            want_promote: "true",
            want_ref: sha,
            want_sha_tag: "sha-0123456789abcdef0123456789abcdef01234567",
            want_version: "",
        },
        Case {
            name: "workflow_dispatch on feature branch is rejected",
            env: vec![
                ("EVENT_NAME", "workflow_dispatch"),
                ("REPOSITORY", "robert-crandall/rust-home-server"),
                ("GITHUB_REF", "refs/heads/feature-branch"),
                ("SHA", sha),
            ],
            want_publish: "false",
            want_promote: "false",
            want_ref: "",
            want_sha_tag: "",
            want_version: "",
        },
    ];

    let script = repo_root().join("scripts/ci/publish-plan.sh");
    for tc in cases {
        let output = std::process::Command::new(&script)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .envs(tc.env)
            .output()
            .expect("run publish-plan.sh");
        assert!(
            output.status.success(),
            "case '{}' exited non-zero: {}",
            tc.name,
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
        let parsed: BTreeMap<&str, &str> = stdout
            .lines()
            .map(|line| line.split_once('=').expect("key=value line"))
            .collect();

        assert_eq!(
            parsed.get("image").copied(),
            Some("ghcr.io/robert-crandall/rust-home-server"),
            "case '{}': image must be lowercased",
            tc.name
        );
        assert_eq!(
            parsed.get("publish").copied(),
            Some(tc.want_publish),
            "{}",
            tc.name
        );
        assert_eq!(
            parsed.get("promote_main").copied(),
            Some(tc.want_promote),
            "{}",
            tc.name
        );
        assert_eq!(parsed.get("ref").copied(), Some(tc.want_ref), "{}", tc.name);
        assert_eq!(
            parsed.get("sha_tag").copied(),
            Some(tc.want_sha_tag),
            "{}",
            tc.name
        );
        assert_eq!(
            parsed.get("version").copied(),
            Some(tc.want_version),
            "{}",
            tc.name
        );
    }
}

#[test]
fn notify_decision_script_matrix() {
    let cases = vec![
        (
            "CI/push/success",
            "CI",
            "push",
            "success",
            "false",
            "not-a-failure",
        ),
        (
            "CI/push/failure",
            "CI",
            "push",
            "failure",
            "true",
            "main-broken",
        ),
        (
            "CI/push/timed_out",
            "CI",
            "push",
            "timed_out",
            "true",
            "main-broken",
        ),
        (
            "CI/push/startup_failure",
            "CI",
            "push",
            "startup_failure",
            "true",
            "main-broken",
        ),
        (
            "CI/push/cancelled",
            "CI",
            "push",
            "cancelled",
            "false",
            "not-a-failure",
        ),
        (
            "CI/pull_request/success",
            "CI",
            "pull_request",
            "success",
            "false",
            "not-a-failure",
        ),
        (
            "CI/pull_request/failure",
            "CI",
            "pull_request",
            "failure",
            "false",
            "not-a-push",
        ),
        (
            "CI/pull_request/timed_out",
            "CI",
            "pull_request",
            "timed_out",
            "false",
            "not-a-push",
        ),
        (
            "CI/pull_request/startup_failure",
            "CI",
            "pull_request",
            "startup_failure",
            "false",
            "not-a-push",
        ),
        (
            "CI/pull_request/cancelled",
            "CI",
            "pull_request",
            "cancelled",
            "false",
            "not-a-failure",
        ),
        (
            "Publish/workflow_run/success",
            "Publish",
            "workflow_run",
            "success",
            "false",
            "not-a-failure",
        ),
        (
            "Publish/workflow_run/failure",
            "Publish",
            "workflow_run",
            "failure",
            "true",
            "publish-failed",
        ),
        (
            "Publish/workflow_run/skipped",
            "Publish",
            "workflow_run",
            "skipped",
            "false",
            "not-a-failure",
        ),
        (
            "Publish/workflow_run/cancelled",
            "Publish",
            "workflow_run",
            "cancelled",
            "false",
            "not-a-failure",
        ),
        (
            "Publish/workflow_dispatch/success",
            "Publish",
            "workflow_dispatch",
            "success",
            "false",
            "not-a-failure",
        ),
        (
            "Publish/workflow_dispatch/failure",
            "Publish",
            "workflow_dispatch",
            "failure",
            "true",
            "publish-failed",
        ),
        (
            "Publish/workflow_dispatch/skipped",
            "Publish",
            "workflow_dispatch",
            "skipped",
            "false",
            "not-a-failure",
        ),
        (
            "Publish/workflow_dispatch/cancelled",
            "Publish",
            "workflow_dispatch",
            "cancelled",
            "false",
            "not-a-failure",
        ),
        (
            "Publish/push/success",
            "Publish",
            "push",
            "success",
            "false",
            "not-a-failure",
        ),
        (
            "Publish/push/failure",
            "Publish",
            "push",
            "failure",
            "true",
            "publish-failed",
        ),
        (
            "Publish/push/skipped",
            "Publish",
            "push",
            "skipped",
            "false",
            "not-a-failure",
        ),
        (
            "Publish/push/cancelled",
            "Publish",
            "push",
            "cancelled",
            "false",
            "not-a-failure",
        ),
    ];

    let script = repo_root().join("scripts/ci/notify-decision.sh");
    for (name, workflow, event, concl, want_notify, want_reason) in cases {
        let output = std::process::Command::new(&script)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("WORKFLOW_NAME", workflow)
            .env("RUN_EVENT", event)
            .env("RUN_CONCLUSION", concl)
            .output()
            .expect("run notify-decision.sh");
        assert!(
            output.status.success(),
            "case '{}' exited non-zero: {}",
            name,
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
        let parsed: BTreeMap<&str, &str> = stdout
            .lines()
            .filter_map(|line| line.split_once('='))
            .collect();

        assert_eq!(parsed.get("notify").copied(), Some(want_notify), "{}", name);
        assert_eq!(parsed.get("reason").copied(), Some(want_reason), "{}", name);
    }
}

#[test]
fn init_script_renames_template_cleanly() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = repo_root();

    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg("tar -cf - --exclude=./target --exclude=./bin --exclude=./.bin --exclude=./web/node_modules --exclude=./web/build --exclude=./.git . | (cd \"$1\" && tar -xf -)")
        .arg("copy-template")
        .arg(dir.path())
        .current_dir(&root)
        .status()
        .expect("copy template tree");
    assert!(status.success(), "copy template tree failed");

    let git_init = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(dir.path())
        .status()
        .expect("git init");
    assert!(git_init.success());

    let git_add = std::process::Command::new("git")
        .args(["add", "-A"])
        .current_dir(dir.path())
        .status()
        .expect("git add");
    assert!(git_add.success());

    let output = std::process::Command::new("bash")
        .args([
            "scripts/init.sh",
            "github.com/acme/chore-board",
            "Chore Board",
        ])
        .current_dir(dir.path())
        .output()
        .expect("run scripts/init.sh");
    assert!(
        output.status.success(),
        "scripts/init.sh failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let cargo_toml =
        std::fs::read_to_string(dir.path().join("Cargo.toml")).expect("read renamed Cargo.toml");
    assert!(cargo_toml.contains("name = \"chore-board\""));
    assert!(cargo_toml.contains("name = \"chore-board-mcp\""));
    assert!(cargo_toml.contains("name = \"app\""));

    let apiclient_rs = std::fs::read_to_string(dir.path().join("src/apiclient.rs"))
        .expect("read renamed src/apiclient.rs");
    assert!(apiclient_rs.contains("pub const APP_SLUG: &str = \"chore-board\";"));

    let auth_rs =
        std::fs::read_to_string(dir.path().join("src/auth.rs")).expect("read renamed src/auth.rs");
    assert!(auth_rs.contains("pub const SESSION_COOKIE_NAME: &str = \"chore_board_session\";"));
}

struct TestDb {
    admin_url: String,
    db_name: String,
    db_url: String,
    pool: PgPool,
}

impl TestDb {
    async fn setup() -> Option<Self> {
        let base_url = match std::env::var("TEST_DATABASE_URL") {
            Ok(val) if !val.trim().is_empty() => val,
            _ => {
                if std::env::var("REQUIRE_TEST_DB").as_deref() == Ok("1") {
                    panic!("REQUIRE_TEST_DB=1 is set, but TEST_DATABASE_URL is missing");
                }
                eprintln!("skipping Postgres integration test (TEST_DATABASE_URL not set)");
                return None;
            }
        };

        let mut parsed = url::Url::parse(&base_url).expect("parse TEST_DATABASE_URL");
        let db_name = format!("rust_home_server_test_{}", Uuid::new_v4().simple());

        let mut admin_conn = PgConnection::connect(parsed.as_str())
            .await
            .expect("connect to admin database");
        admin_conn
            .execute(format!("CREATE DATABASE \"{db_name}\"").as_str())
            .await
            .expect("create isolated test database");
        drop(admin_conn);

        parsed.set_path(&format!("/{db_name}"));
        let db_url = parsed.to_string();

        let pool = db::connect_and_migrate(&db_url)
            .await
            .expect("connect and run sqlx migrations");

        Some(Self {
            admin_url: base_url,
            db_name,
            db_url,
            pool,
        })
    }

    async fn teardown(self) {
        self.pool.close().await;
        if let Ok(mut admin_conn) = PgConnection::connect(&self.admin_url).await {
            let _ = admin_conn
                .execute(
                    format!("DROP DATABASE IF EXISTS \"{}\" WITH (FORCE)", self.db_name).as_str(),
                )
                .await;
        }
    }
}

async fn spawn_in_process_server(pool: PgPool, allow_open_registration: bool) -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local addr");

    let state = AppState {
        pool,
        secure_cookies: false,
        allow_open_registration,
        cf_access: None,
    };
    let app = build_router(state);
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    format!("http://{addr}")
}

#[tokio::test]
async fn multi_user_auth_csrf_and_tokens_flow() {
    let Some(tdb) = TestDb::setup().await else {
        return;
    };
    let base_url = spawn_in_process_server(tdb.pool.clone(), true).await;

    let client_a = reqwest::Client::builder()
        .cookie_store(true)
        .build()
        .expect("client a");
    let client_b = reqwest::Client::builder()
        .cookie_store(true)
        .build()
        .expect("client b");
    let bare_client = reqwest::Client::new();

    // 1. Healthz and open-registration app info
    let health: Value = bare_client
        .get(format!("{base_url}/healthz"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(health, json!({ "status": "ok" }));

    let app_info: Value = bare_client
        .get(format!("{base_url}/api/app"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(app_info, json!({ "registrationOpen": true }));

    // 2. Register two users; duplicate email (case-insensitive) fails with 409
    let reg_a = client_a
        .post(format!("{base_url}/api/auth/register"))
        .json(&json!({
            "email": "Leo@Example.com",
            "password": "very-strong-passphrase",
            "name": "Leo"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(reg_a.status(), StatusCode::CREATED);
    let user_a: Value = reg_a.json().await.unwrap();
    assert_eq!(user_a["email"], "leo@example.com");
    assert_eq!(user_a["name"], "Leo");

    let dup = bare_client
        .post(format!("{base_url}/api/auth/register"))
        .json(&json!({
            "email": "leo@example.com",
            "password": "another-strong-passphrase"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(dup.status(), StatusCode::CONFLICT);

    let reg_b = client_b
        .post(format!("{base_url}/api/auth/register"))
        .json(&json!({
            "email": "dad@example.com",
            "password": "dad-strong-passphrase",
            "name": "Dad"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(reg_b.status(), StatusCode::CREATED);

    // 3. Invalid login attempts return 401
    let bad_pw = bare_client
        .post(format!("{base_url}/api/auth/login"))
        .json(&json!({
            "email": "leo@example.com",
            "password": "wrong-password"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(bad_pw.status(), StatusCode::UNAUTHORIZED);

    let bad_user = bare_client
        .post(format!("{base_url}/api/auth/login"))
        .json(&json!({
            "email": "nobody@example.com",
            "password": "wrong-password"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(bad_user.status(), StatusCode::UNAUTHORIZED);

    // 4. CSRF defense blocks cross-site Sec-Fetch-Site and mismatched Origin
    let csrf_fetch_site = client_a
        .post(format!("{base_url}/api/tokens"))
        .header("Sec-Fetch-Site", "cross-site")
        .json(&json!({ "name": "evil" }))
        .send()
        .await
        .unwrap();
    assert_eq!(csrf_fetch_site.status(), StatusCode::FORBIDDEN);

    let csrf_origin = client_a
        .post(format!("{base_url}/api/tokens"))
        .header(ORIGIN, "https://attacker.example")
        .json(&json!({ "name": "evil" }))
        .send()
        .await
        .unwrap();
    assert_eq!(csrf_origin.status(), StatusCode::FORBIDDEN);

    // 5. Same-origin token minting, listing, bearer auth, and cookie-fallback isolation
    let create_tok = client_a
        .post(format!("{base_url}/api/tokens"))
        .header(ORIGIN, &base_url)
        .json(&json!({ "name": "laptop-mcp" }))
        .send()
        .await
        .unwrap();
    assert_eq!(create_tok.status(), StatusCode::CREATED);
    let minted: Value = create_tok.json().await.unwrap();
    let token_id = minted["id"].as_str().unwrap().to_string();
    let raw_token = minted["token"].as_str().unwrap().to_string();
    assert!(raw_token.starts_with("pat_"));

    // Bearer token authenticates /api/auth/me without cookies (including case-insensitive scheme)
    let bearer_me = bare_client
        .get(format!("{base_url}/api/auth/me"))
        .header(AUTHORIZATION, format!("BEARER {raw_token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(bearer_me.status(), StatusCode::OK);
    let bearer_user: Value = bearer_me.json().await.unwrap();
    assert_eq!(bearer_user["email"], "leo@example.com");

    // Unauthenticated /api/auth/me returns 401
    let unauth_me = bare_client
        .get(format!("{base_url}/api/auth/me"))
        .send()
        .await
        .unwrap();
    assert_eq!(unauth_me.status(), StatusCode::UNAUTHORIZED);

    // Logout rejects Authorization headers (403) and preserves any active browser session
    let bearer_logout = client_a
        .post(format!("{base_url}/api/auth/logout"))
        .header(ORIGIN, &base_url)
        .header(AUTHORIZATION, format!("Bearer {raw_token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(bearer_logout.status(), StatusCode::FORBIDDEN);

    let still_logged_in = client_a
        .get(format!("{base_url}/api/auth/me"))
        .send()
        .await
        .unwrap();
    assert_eq!(still_logged_in.status(), StatusCode::OK);

    // Token listing returns metadata + updated lastUsedAt, never the raw token
    let list_tok: Vec<Value> = client_a
        .get(format!("{base_url}/api/tokens"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list_tok.len(), 1);
    assert!(list_tok[0].get("token").is_none());
    assert!(list_tok[0]["lastUsedAt"].is_string());

    // Bearer token cannot mint another token (SessionUser required -> 403 Forbidden)
    let bearer_mint = bare_client
        .post(format!("{base_url}/api/tokens"))
        .header(AUTHORIZATION, format!("Bearer {raw_token}"))
        .json(&json!({ "name": "escalate" }))
        .send()
        .await
        .unwrap();
    assert_eq!(bearer_mint.status(), StatusCode::FORBIDDEN);

    // Invalid Authorization header must NOT fall back to a valid session cookie
    let bad_bearer_with_cookie = client_a
        .get(format!("{base_url}/api/auth/me"))
        .header(AUTHORIZATION, "Bearer pat_invalid_token")
        .send()
        .await
        .unwrap();
    assert_eq!(bad_bearer_with_cookie.status(), StatusCode::UNAUTHORIZED);

    // User B cannot delete User A's personal access token
    let wrong_owner_del = client_b
        .delete(format!("{base_url}/api/tokens/{token_id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(wrong_owner_del.status(), StatusCode::NOT_FOUND);

    // User A revokes token; subsequent bearer call fails
    let owner_del = client_a
        .delete(format!("{base_url}/api/tokens/{token_id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(owner_del.status(), StatusCode::NO_CONTENT);

    let revoked_me = bare_client
        .get(format!("{base_url}/api/auth/me"))
        .header(AUTHORIZATION, format!("Bearer {raw_token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(revoked_me.status(), StatusCode::UNAUTHORIZED);

    // 6. Sliding session renewal extends DB expiry and re-emits Set-Cookie
    sqlx::query("UPDATE sessions SET expires_at = now() + interval '5 days'")
        .execute(&tdb.pool)
        .await
        .unwrap();

    let login_fresh = bare_client
        .post(format!("{base_url}/api/auth/login"))
        .json(&json!({
            "email": "leo@example.com",
            "password": "very-strong-passphrase"
        }))
        .send()
        .await
        .unwrap();
    let set_cookie = login_fresh
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    assert!(set_cookie.starts_with("rust_home_server_session="));

    sqlx::query("UPDATE sessions SET expires_at = now() + interval '5 days'")
        .execute(&tdb.pool)
        .await
        .unwrap();

    let renewed_resp = bare_client
        .get(format!("{base_url}/api/auth/me"))
        .header(COOKIE, &set_cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(renewed_resp.status(), StatusCode::OK);
    assert!(
        renewed_resp.headers().get("set-cookie").is_some(),
        "sliding renewal must refresh Set-Cookie when <15 days remain"
    );

    tdb.teardown().await;
}

#[tokio::test]
async fn closed_registration_allows_only_bootstrap_user() {
    let Some(tdb) = TestDb::setup().await else {
        return;
    };
    let base_url = spawn_in_process_server(tdb.pool.clone(), false).await;
    let client = reqwest::Client::new();

    // Fresh DB reports registrationOpen=true so the initial bootstrap user can register
    let before: Value = client
        .get(format!("{base_url}/api/app"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(before, json!({ "registrationOpen": true }));

    // Race two simultaneous registrations against ALLOW_OPEN_REGISTRATION=false
    let fut1 = client
        .post(format!("{base_url}/api/auth/register"))
        .json(&json!({
            "email": "first@example.com",
            "password": "first-strong-passphrase"
        }))
        .send();
    let fut2 = client
        .post(format!("{base_url}/api/auth/register"))
        .json(&json!({
            "email": "second@example.com",
            "password": "second-strong-passphrase"
        }))
        .send();

    let (r1, r2) = tokio::join!(fut1, fut2);
    let mut statuses = [r1.unwrap().status(), r2.unwrap().status()];
    statuses.sort_by_key(|s| s.as_u16());
    assert_eq!(statuses, [StatusCode::CREATED, StatusCode::FORBIDDEN]);

    let after: Value = client
        .get(format!("{base_url}/api/app"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(after, json!({ "registrationOpen": false }));

    tdb.teardown().await;
}

#[tokio::test]
async fn binaries_spa_healthcheck_and_mcp_stdio_end_to_end() {
    let Some(tdb) = TestDb::setup().await else {
        return;
    };

    // Reserve a free port for the compiled `rust-home-server` server binary
    let probe = StdTcpListener::bind("127.0.0.1:0").expect("reserve port");
    let addr = probe.local_addr().expect("local addr");
    drop(probe);

    let temp_dir =
        std::env::temp_dir().join(format!("rust-home-server-mcp-test-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&temp_dir).expect("create temp test dir");

    let server_bin = env!("CARGO_BIN_EXE_rust-home-server");
    let mut server_child = Command::new(server_bin)
        .current_dir(&temp_dir)
        .env("DATABASE_URL", &tdb.db_url)
        .env("ADDR", addr.to_string())
        .env("APP_ENV", "development")
        .env_remove("ALLOW_OPEN_REGISTRATION")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn rust-home-server binary");

    let base_url = format!("http://{addr}");
    let http = reqwest::Client::builder()
        .cookie_store(true)
        .build()
        .unwrap();

    let mut healthy = false;
    for _ in 0..50 {
        if let Ok(resp) = http.get(format!("{base_url}/healthz")).send().await
            && resp.status() == StatusCode::OK
        {
            healthy = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        healthy,
        "rust-home-server binary did not become healthy on {base_url}"
    );

    // 1. Embedded Solid 2.0 SPA assets + client-side route fallback + API 404 boundary
    let root_html = http.get(&base_url).send().await.unwrap();
    assert_eq!(root_html.status(), StatusCode::OK);
    let body = root_html.text().await.unwrap();
    assert!(body.contains("<div id=\"app\"></div>"));

    let spa_route = http.get(format!("{base_url}/login")).send().await.unwrap();
    assert_eq!(spa_route.status(), StatusCode::OK);

    let theme_js = http
        .get(format!("{base_url}/theme.js"))
        .send()
        .await
        .unwrap();
    assert_eq!(theme_js.status(), StatusCode::OK);

    let api_404 = http
        .get(format!("{base_url}/api/does-not-exist"))
        .send()
        .await
        .unwrap();
    assert_eq!(api_404.status(), StatusCode::NOT_FOUND);

    // 2. Built-in container healthcheck subcommand (`rust-home-server healthcheck`)
    let hc_status = Command::new(server_bin)
        .arg("healthcheck")
        .env("ADDR", addr.to_string())
        .status()
        .await
        .expect("run rust-home-server healthcheck");
    assert!(hc_status.success(), "rust-home-server healthcheck failed");

    // 3. Register a user, mint MCP config (0600), and exercise rust-home-server-mcp over stdio
    let reg = http
        .post(format!("{base_url}/api/auth/register"))
        .json(&json!({
            "email": "agent@example.com",
            "password": "mcp-strong-passphrase",
            "name": "Homelab Agent"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(reg.status(), StatusCode::CREATED);

    let second_reg = reqwest::Client::new()
        .post(format!("{base_url}/api/auth/register"))
        .json(&json!({
            "email": "second@example.com",
            "password": "second-strong-passphrase"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(second_reg.status(), StatusCode::FORBIDDEN);

    let minted = mint_token_via_login(
        &base_url,
        "agent@example.com",
        "mcp-strong-passphrase",
        "stdio-test",
    )
    .await
    .expect("mint token via login");

    let cfg_path = temp_dir.join("rust-home-server.json");
    let cfg = ClientConfig {
        app_url: base_url.clone(),
        token: minted.token.clone(),
    };
    cfg.write_atomic(&cfg_path)
        .expect("write atomic mcp config");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&cfg_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "MCP config must be written with 0600 permissions"
        );
    }

    // Invoke `rust-home-server-mcp --config <path>` without `serve` to verify default subcommand dispatch
    let mcp_bin = env!("CARGO_BIN_EXE_rust-home-server-mcp");
    let mut mcp_child = Command::new(mcp_bin)
        .arg("--config")
        .arg(&cfg_path)
        .env_remove("MCP_APP_URL")
        .env_remove("MCP_APP_TOKEN")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn rust-home-server-mcp");

    let mut mcp_stdin = mcp_child.stdin.take().unwrap();
    let mut mcp_lines = BufReader::new(mcp_child.stdout.take().unwrap()).lines();

    // JSON-RPC 1: initialize
    let init_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "integration-test", "version": "0.1.0" }
        }
    });
    mcp_stdin
        .write_all(format!("{init_req}\n").as_bytes())
        .await
        .unwrap();
    mcp_stdin.flush().await.unwrap();

    let init_line = tokio::time::timeout(Duration::from_secs(5), mcp_lines.next_line())
        .await
        .expect("initialize timeout")
        .expect("read initialize line")
        .expect("initialize eof");
    let init_resp: Value = serde_json::from_str(&init_line).unwrap();
    assert_eq!(init_resp["id"], 1);
    assert_eq!(
        init_resp["result"]["serverInfo"]["name"],
        "rust-home-server-mcp"
    );

    // JSON-RPC notification: notifications/initialized
    let initialized_notif = json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    });
    mcp_stdin
        .write_all(format!("{initialized_notif}\n").as_bytes())
        .await
        .unwrap();

    // JSON-RPC 2: tools/call whoami
    let call_req = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "whoami",
            "arguments": {}
        }
    });
    mcp_stdin
        .write_all(format!("{call_req}\n").as_bytes())
        .await
        .unwrap();
    mcp_stdin.flush().await.unwrap();

    let call_line = tokio::time::timeout(Duration::from_secs(5), mcp_lines.next_line())
        .await
        .expect("whoami timeout")
        .expect("read whoami line")
        .expect("whoami eof");
    let call_resp: Value = serde_json::from_str(&call_line).unwrap();
    assert_eq!(call_resp["id"], 2);
    let text = call_resp["result"]["content"][0]["text"]
        .as_str()
        .expect("whoami text content");
    let whoami_user: Value = serde_json::from_str(text).expect("whoami json user");
    assert_eq!(whoami_user["email"], "agent@example.com");
    assert_eq!(whoami_user["name"], "Homelab Agent");

    drop(mcp_stdin);
    let _ = mcp_child.kill().await;
    let _ = std::fs::remove_dir_all(&temp_dir);
    let _ = server_child.kill().await;
    tdb.teardown().await;
}
