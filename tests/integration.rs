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
fn built_spa_includes_compiled_daisyui_themes() {
    let build_dir = repo_root().join("web/build");
    let index_html =
        std::fs::read_to_string(build_dir.join("index.html")).expect("read web/build/index.html");
    let (head, _) = index_html
        .split_once("</head>")
        .expect("index.html has </head>");
    assert!(
        head.contains("<script src=\"/theme.js\"></script>"),
        "web/build/index.html must load /theme.js blocking in <head> before paint"
    );

    let css_files: Vec<PathBuf> = std::fs::read_dir(build_dir.join("assets"))
        .expect("read web/build/assets")
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            (path.extension().and_then(|e| e.to_str()) == Some("css")).then_some(path)
        })
        .collect();
    assert!(
        !css_files.is_empty(),
        "expected compiled CSS bundle in web/build/assets"
    );

    let combined_css = css_files
        .iter()
        .map(|path| std::fs::read_to_string(path).expect("read compiled css"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        combined_css.contains("data-theme=light")
            && combined_css.contains("data-theme=dark")
            && combined_css.contains("prefers-color-scheme:dark")
            && combined_css.contains(":root:not([data-theme])")
            && combined_css.contains("--color-base-100")
            && combined_css.contains("--color-primary"),
        "compiled CSS must include DaisyUI light, dark, and prefers-color-scheme:dark theme rules"
    );
    assert!(
        !combined_css.contains("@plugin") && !combined_css.contains("@theme"),
        "compiled CSS must not contain unprocessed Tailwind/DaisyUI @plugin or @theme directives"
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

fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 24 || &bytes[0..8] != b"\x89PNG\r\n\x1a\n" || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    Some((width, height))
}

fn copy_template_repo() -> tempfile::TempDir {
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

    dir
}

fn write_solid_rgba_png(path: &std::path::Path, width: u32, height: u32, rgba: [u8; 4]) {
    let script = r#"
import struct, sys, zlib
path, w, h, r, g, b, a = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), int(sys.argv[4]), int(sys.argv[5]), int(sys.argv[6]), int(sys.argv[7])
def chunk(t, d):
    return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)
ihdr = struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0)
row = b"\x00" + bytes((r, g, b, a)) * w
idat = zlib.compress(row * h, level=1)
with open(path, "wb") as f:
    f.write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", idat) + chunk(b"IEND", b""))
"#;
    let status = std::process::Command::new("python3")
        .args([
            "-c",
            script,
            path.to_str().expect("utf8 path"),
            &width.to_string(),
            &height.to_string(),
            &rgba[0].to_string(),
            &rgba[1].to_string(),
            &rgba[2].to_string(),
            &rgba[3].to_string(),
        ])
        .status()
        .expect("write test png");
    assert!(status.success(), "failed to write test PNG");
}

#[test]
fn init_script_renames_template_cleanly() {
    let dir = copy_template_repo();
    let orig_512 = std::fs::read(dir.path().join("web/public/icon-512.png")).expect("orig 512");
    let orig_192 = std::fs::read(dir.path().join("web/public/icon-192.png")).expect("orig 192");

    // Plant a tracked binary PNG containing the template name bytes to verify binary files are skipped.
    let planted_path = dir.path().join("web/public/keep-binary.png");
    let planted_bytes = b"\x89PNG\r\n\x1a\nRust Home Server rust-home-server".to_vec();
    std::fs::write(&planted_path, &planted_bytes).expect("write planted png");
    assert!(
        std::process::Command::new("git")
            .args(["add", "web/public/keep-binary.png"])
            .current_dir(dir.path())
            .status()
            .expect("git add planted png")
            .success()
    );

    let icon_dir = tempfile::tempdir().expect("icon tempdir");
    let custom_icon = icon_dir.path().join("my app icon.png");
    write_solid_rgba_png(&custom_icon, 600, 600, [40, 120, 210, 255]);

    let output = std::process::Command::new("bash")
        .args([
            "scripts/init.sh",
            "github.com/acme/chore-board",
            "Chore Board",
            "",
            custom_icon.to_str().expect("custom icon path"),
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

    let manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("web/public/manifest.webmanifest"))
            .expect("read renamed manifest.webmanifest"),
    )
    .expect("renamed manifest.webmanifest must be valid JSON");
    assert_eq!(manifest["name"].as_str(), Some("Chore Board"));
    assert_eq!(manifest["short_name"].as_str(), Some("Chore Board"));

    let new_512 = std::fs::read(dir.path().join("web/public/icon-512.png")).expect("new 512");
    let new_192 = std::fs::read(dir.path().join("web/public/icon-192.png")).expect("new 192");
    assert_eq!(png_dimensions(&new_512), Some((512, 512)));
    assert_eq!(png_dimensions(&new_192), Some((192, 192)));
    assert_ne!(new_512, orig_512, "icon-512.png must be replaced");
    assert_ne!(new_192, orig_192, "icon-192.png must be replaced");

    assert_eq!(
        std::fs::read(&planted_path).expect("read planted png"),
        planted_bytes,
        "unrelated tracked PNG must stay byte-identical"
    );
}

#[test]
fn init_script_requires_new_icon_without_modifying_tree() {
    let dir = copy_template_repo();
    assert!(
        std::process::Command::new("git")
            .args([
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-q",
                "-m",
                "baseline",
            ])
            .current_dir(dir.path())
            .status()
            .expect("git commit baseline")
            .success()
    );

    let cases: [(&str, Vec<&str>, &str); 3] = [
        (
            "missing ICON",
            vec![
                "scripts/init.sh",
                "github.com/acme/chore-board",
                "Chore Board",
            ],
            "Usage:",
        ),
        (
            "unchanged template icon",
            vec![
                "scripts/init.sh",
                "github.com/acme/chore-board",
                "Chore Board",
                "",
                "web/public/icon-512.png",
            ],
            "default template icon",
        ),
        (
            "quoted NAME",
            vec![
                "scripts/init.sh",
                "github.com/acme/chore-board",
                "Chore \"Board\"",
                "",
                "web/public/icon-512.png",
            ],
            "NAME must not contain",
        ),
    ];

    for (label, args, want_stderr) in cases {
        let output = std::process::Command::new("bash")
            .args(&args)
            .current_dir(dir.path())
            .output()
            .expect("run scripts/init.sh");
        assert!(
            !output.status.success(),
            "expected scripts/init.sh to fail for {label}"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(want_stderr),
            "expected stderr for {label} to contain {want_stderr:?}, got: {stderr}"
        );

        let status_out = std::process::Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(dir.path())
            .output()
            .expect("git status");
        assert!(
            status_out.stdout.is_empty(),
            "failed init ({label}) left modified files: {}",
            String::from_utf8_lossy(&status_out.stdout)
        );
    }
}

#[test]
fn render_icons_script_validates_and_renders_variants() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let script = repo_root().join("scripts/render-icons.py");
    let out_dir = tmp.path().join("out");

    // 1. Reject non-PNG (e.g. SVG)
    let svg_path = tmp.path().join("icon.svg");
    std::fs::write(&svg_path, "<svg xmlns=\"http://www.w3.org/2000/svg\"/>").unwrap();
    assert!(
        !std::process::Command::new("python3")
            .args([
                script.to_str().unwrap(),
                "--out-dir",
                out_dir.to_str().unwrap(),
                svg_path.to_str().unwrap(),
            ])
            .status()
            .unwrap()
            .success(),
        "SVG input should be rejected"
    );

    // 2. Reject non-square and too-small PNGs
    let rect_png = tmp.path().join("rect.png");
    write_solid_rgba_png(&rect_png, 600, 512, [10, 20, 30, 255]);
    assert!(
        !std::process::Command::new("python3")
            .args([
                script.to_str().unwrap(),
                "--out-dir",
                out_dir.to_str().unwrap(),
                rect_png.to_str().unwrap(),
            ])
            .status()
            .unwrap()
            .success(),
        "non-square PNG should be rejected"
    );

    let small_png = tmp.path().join("small.png");
    write_solid_rgba_png(&small_png, 192, 192, [10, 20, 30, 255]);
    assert!(
        !std::process::Command::new("python3")
            .args([
                script.to_str().unwrap(),
                "--out-dir",
                out_dir.to_str().unwrap(),
                small_png.to_str().unwrap(),
            ])
            .status()
            .unwrap()
            .success(),
        "< 512x512 PNG should be rejected"
    );

    // 3. Valid indexed palette + tRNS PNG renders 512 and 192 preserving alpha
    let pal_png = tmp.path().join("palette.png");
    let make_variants = r#"
import struct, sys, zlib
def chunk(t, d):
    return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)

# 512x512 indexed PNG: index 0 = transparent, index 1 = opaque teal
plte = bytes([0, 0, 0, 20, 180, 140])
trns = bytes([0, 255])
rows = bytearray()
for y in range(512):
    rows.append(0)
    if y < 64:
        rows.extend(b"\x00" * 512)
    else:
        rows.extend(b"\x01" * 512)
ihdr = struct.pack(">IIBBBBB", 512, 512, 8, 3, 0, 0, 0)
with open(sys.argv[1], "wb") as f:
    f.write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"PLTE", plte) + chunk(b"tRNS", trns) + chunk(b"IDAT", zlib.compress(bytes(rows), 1)) + chunk(b"IEND", b""))

# 512x512 16-bit RGB PNG
ihdr16 = struct.pack(">IIBBBBB", 512, 512, 16, 2, 0, 0, 0)
row16 = b"\x00" + struct.pack(">HHH", 0x3300, 0x6600, 0x9900) * 512
with open(sys.argv[2], "wb") as f:
    f.write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr16) + chunk(b"IDAT", zlib.compress(row16 * 512, 1)) + chunk(b"IEND", b""))
"#;
    let rgb16_png = tmp.path().join("rgb16.png");
    assert!(
        std::process::Command::new("python3")
            .args([
                "-c",
                make_variants,
                pal_png.to_str().unwrap(),
                rgb16_png.to_str().unwrap(),
            ])
            .status()
            .unwrap()
            .success()
    );

    for (label, path) in [("palette+tRNS", &pal_png), ("16-bit RGB", &rgb16_png)] {
        let out = std::process::Command::new("python3")
            .args([
                script.to_str().unwrap(),
                "--out-dir",
                out_dir.to_str().unwrap(),
                path.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "render-icons.py failed for {label}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            png_dimensions(&std::fs::read(out_dir.join("icon-512.png")).unwrap()),
            Some((512, 512))
        );
        assert_eq!(
            png_dimensions(&std::fs::read(out_dir.join("icon-192.png")).unwrap()),
            Some((192, 192))
        );
        if label == "palette+tRNS" {
            let check_alpha = r#"
import importlib.util, sys
from pathlib import Path
spec = importlib.util.spec_from_file_location("ri", sys.argv[1])
mod = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mod)
w, h, rgba = mod.decode_png_rgba8(Path(sys.argv[2]).read_bytes())
assert (w, h) == (192, 192)
assert rgba[3] == 0, f"expected transparent top-left pixel, got alpha={rgba[3]}"
center = (100 * w + 100) * 4
assert rgba[center + 3] == 255, f"expected opaque center pixel, got alpha={rgba[center + 3]}"
"#;
            let status = std::process::Command::new("python3")
                .args([
                    "-B",
                    "-c",
                    check_alpha,
                    script.to_str().unwrap(),
                    out_dir.join("icon-192.png").to_str().unwrap(),
                ])
                .status()
                .unwrap();
            assert!(
                status.success(),
                "palette+tRNS transparency was not preserved"
            );
        }
    }

    // 4. Reject identical icon when --reject-same-as is supplied
    assert!(
        !std::process::Command::new("python3")
            .args([
                script.to_str().unwrap(),
                "--out-dir",
                out_dir.to_str().unwrap(),
                "--reject-same-as",
                out_dir.join("icon-512.png").to_str().unwrap(),
                rgb16_png.to_str().unwrap(),
            ])
            .status()
            .unwrap()
            .success(),
        "--reject-same-as should reject an identical rendered icon"
    );
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
        household: None,
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
    assert_eq!(
        app_info,
        json!({ "registrationOpen": true, "cloudflareAccessEnabled": false })
    );

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
    assert_eq!(
        before,
        json!({ "registrationOpen": true, "cloudflareAccessEnabled": false })
    );

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
    assert_eq!(
        after,
        json!({ "registrationOpen": false, "cloudflareAccessEnabled": false })
    );

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

    let manifest_resp = http
        .get(format!("{base_url}/manifest.webmanifest"))
        .send()
        .await
        .unwrap();
    assert_eq!(manifest_resp.status(), StatusCode::OK);
    assert_eq!(
        manifest_resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("application/manifest+json")
    );

    let icon_resp = http
        .get(format!("{base_url}/icon-192.png"))
        .send()
        .await
        .unwrap();
    assert_eq!(icon_resp.status(), StatusCode::OK);
    assert_eq!(
        icon_resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("image/png")
    );

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
        None,
        None,
    )
    .await
    .expect("mint token via login");

    let cfg_path = temp_dir.join("rust-home-server.json");
    let cfg = ClientConfig {
        app_url: base_url.clone(),
        token: minted.token.clone(),
        cf_access_client_id: None,
        cf_access_client_secret: None,
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

#[tokio::test]
async fn cloudflare_access_auth_provisioning_and_logout_flow() {
    use base64::Engine as _;
    use rsa::RsaPrivateKey;
    use rsa::pkcs8::EncodePrivateKey;
    use rsa::traits::PublicKeyParts;

    let Some(tdb) = TestDb::setup().await else {
        return;
    };

    let team_domain = "https://household.cloudflareaccess.com";
    let aud = "test-cf-aud";
    let kid = "cf-test-key";

    let mut rng = rsa::rand_core::OsRng;
    let priv_key = RsaPrivateKey::new(&mut rng, 2048).expect("generate RSA key");
    let pem = priv_key
        .to_pkcs8_pem(rsa::pkcs8::LineEnding::LF)
        .expect("encode PEM");
    let encoding_key =
        jsonwebtoken::EncodingKey::from_rsa_pem(pem.as_bytes()).expect("encoding key");
    let n = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(priv_key.n().to_bytes_be());
    let e = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(priv_key.e().to_bytes_be());
    let jwks: jsonwebtoken::jwk::JwkSet = serde_json::from_value(json!({
        "keys": [{
            "kty": "RSA",
            "alg": "RS256",
            "use": "sig",
            "kid": kid,
            "n": n,
            "e": e,
        }]
    }))
    .expect("parse JwkSet");

    let sign_jwt = |email: &str| {
        let now = chrono::Utc::now().timestamp();
        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
        header.kid = Some(kid.to_string());
        jsonwebtoken::encode(
            &header,
            &json!({
                "email": email,
                "iss": team_domain,
                "aud": [aud],
                "exp": now + 300,
                "nbf": now - 10,
            }),
            &encoding_key,
        )
        .expect("sign CF Access JWT")
    };

    let household_dir = tempfile::tempdir().expect("household tempdir");
    let png_b64 = base64::engine::general_purpose::STANDARD.encode(b"\x89PNG\r\n\x1a\ncf-avatar");
    let yaml_path = household_dir.path().join("users.yaml");
    std::fs::write(
        &yaml_path,
        format!(
            "users:\n  alice@example.com:\n    name: Alice Household\n    avatar: \"data:image/png;base64,{png_b64}\"\n"
        ),
    )
    .unwrap();

    let cf_auth = app::auth::CfAccessAuth::with_jwks(
        app::config::CfAccessConfig {
            team_domain: team_domain.to_string(),
            aud: aud.to_string(),
        },
        jwks,
    );
    let state = AppState {
        pool: tdb.pool.clone(),
        secure_cookies: false,
        allow_open_registration: false,
        cf_access: Some(cf_auth),
        household: Some(app::household::HouseholdProfiles::new(yaml_path.clone())),
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, build_router(state)).await.unwrap();
    });
    let base_url = format!("http://{addr}");
    let client = reqwest::Client::new();

    let app_info: Value = client
        .get(format!("{base_url}/api/app"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        app_info,
        json!({ "registrationOpen": true, "cloudflareAccessEnabled": true })
    );

    // Pre-existing password-backed user links by email (case-insensitively) without losing
    // their display name or password login.
    let existing_reg = client
        .post(format!("{base_url}/api/auth/register"))
        .json(&json!({
            "email": "carol@example.com",
            "password": "carol-strong-passphrase",
            "name": "Carol Custom"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(existing_reg.status(), StatusCode::CREATED);
    let carol_local: Value = existing_reg.json().await.unwrap();

    let carol_jwt = sign_jwt("Carol@Example.com");
    let carol_cf: Value = client
        .get(format!("{base_url}/api/auth/me"))
        .header("Cf-Access-Jwt-Assertion", &carol_jwt)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(carol_cf["id"], carol_local["id"]);
    assert_eq!(carol_cf["name"], "Carol Custom");

    let carol_relogin = client
        .post(format!("{base_url}/api/auth/login"))
        .json(&json!({
            "email": "carol@example.com",
            "password": "carol-strong-passphrase"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(carol_relogin.status(), StatusCode::OK);

    // Race two concurrent first requests to verify ON CONFLICT auto-provisioning
    let jwt = sign_jwt("Alice@Example.com");
    let req1 = client
        .get(format!("{base_url}/api/auth/me"))
        .header("Cf-Access-Jwt-Assertion", &jwt)
        .send();
    let req2 = client
        .get(format!("{base_url}/api/auth/me"))
        .header("Cf-Access-Jwt-Assertion", &jwt)
        .send();
    let (r1, r2) = tokio::join!(req1, req2);
    let r1 = r1.unwrap();
    let r2 = r2.unwrap();
    assert_eq!(r1.status(), StatusCode::OK);
    assert_eq!(r2.status(), StatusCode::OK);
    let u1: Value = r1.json().await.unwrap();
    let u2: Value = r2.json().await.unwrap();
    assert_eq!(u1["id"], u2["id"]);
    assert_eq!(u1["email"], "alice@example.com");
    assert_eq!(u1["name"], "Alice Household");
    let alice_id = u1["id"].as_str().expect("alice id");
    assert!(
        u1["avatarUrl"]
            .as_str()
            .is_some_and(|url| url.starts_with(&format!("/api/users/{alice_id}/avatar")))
    );
    let stored_alice_name = sqlx::query_scalar::<_, String>("SELECT name FROM users WHERE id = $1")
        .bind(Uuid::parse_str(alice_id).unwrap())
        .fetch_one(&tdb.pool)
        .await
        .unwrap();
    assert_eq!(stored_alice_name, "Alice Household");

    // CF Access user can mint a PAT via SessionUser and Bearer PAT takes precedence over CF JWT
    let minted: Value = client
        .post(format!("{base_url}/api/tokens"))
        .header("Cf-Access-Jwt-Assertion", &jwt)
        .json(&json!({ "name": "cf-mcp" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let pat = minted["token"].as_str().expect("minted PAT");
    let other_jwt = sign_jwt("bob@example.com");
    let bearer_me: Value = client
        .get(format!("{base_url}/api/auth/me"))
        .header("Authorization", format!("Bearer {pat}"))
        .header("Cf-Access-Jwt-Assertion", &other_jwt)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(bearer_me["email"], "alice@example.com");

    // Logout with CF Access JWT returns 200 with Cloudflare logoutUrl
    let cf_logout = client
        .post(format!("{base_url}/api/auth/logout"))
        .header("Cf-Access-Jwt-Assertion", &jwt)
        .send()
        .await
        .unwrap();
    assert_eq!(cf_logout.status(), StatusCode::OK);
    let cf_logout_body: Value = cf_logout.json().await.unwrap();
    assert_eq!(
        cf_logout_body,
        json!({ "logoutUrl": "/cdn-cgi/access/logout" })
    );

    // Logout without CF Access JWT returns 200 with logoutUrl: null
    let plain_logout = client
        .post(format!("{base_url}/api/auth/logout"))
        .send()
        .await
        .unwrap();
    assert_eq!(plain_logout.status(), StatusCode::OK);
    let plain_logout_body: Value = plain_logout.json().await.unwrap();
    assert_eq!(plain_logout_body, json!({ "logoutUrl": null }));

    // Local login and registration are rejected while Cloudflare Access credentials are present
    let conflict_login = client
        .post(format!("{base_url}/api/auth/login"))
        .header("Cf-Access-Jwt-Assertion", &jwt)
        .json(&json!({
            "email": "bob@example.com",
            "password": "bob-strong-passphrase"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(conflict_login.status(), StatusCode::CONFLICT);
    assert!(conflict_login.headers().get("set-cookie").is_none());

    let conflict_login_cookie = client
        .post(format!("{base_url}/api/auth/login"))
        .header(COOKIE, format!("CF_Authorization={jwt}"))
        .json(&json!({
            "email": "bob@example.com",
            "password": "bob-strong-passphrase"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(conflict_login_cookie.status(), StatusCode::CONFLICT);

    let conflict_register = client
        .post(format!("{base_url}/api/auth/register"))
        .header("Cf-Access-Jwt-Assertion", &jwt)
        .json(&json!({
            "email": "bob@example.com",
            "password": "bob-strong-passphrase"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(conflict_register.status(), StatusCode::CONFLICT);
    assert!(conflict_register.headers().get("set-cookie").is_none());

    // Passwordless CF-provisioned user cannot sign in locally without a password
    let passwordless_login = client
        .post(format!("{base_url}/api/auth/login"))
        .json(&json!({
            "email": "alice@example.com",
            "password": "any-attempted-password"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(passwordless_login.status(), StatusCode::UNAUTHORIZED);
    assert!(passwordless_login.headers().get("set-cookie").is_none());

    // Cookie-only CF_Authorization authenticates browser requests
    let cookie_me = client
        .get(format!("{base_url}/api/auth/me"))
        .header(COOKIE, format!("CF_Authorization={jwt}"))
        .send()
        .await
        .unwrap();
    assert_eq!(cookie_me.status(), StatusCode::OK);
    let cookie_user: Value = cookie_me.json().await.unwrap();
    assert_eq!(cookie_user["email"], "alice@example.com");

    // Subsequent CF authentication reflects manifest updates without rewriting the stored DB name
    std::fs::write(
        &yaml_path,
        "users:\n  alice@example.com:\n    name: Alice Updated\n",
    )
    .unwrap();
    let updated_cf_me: Value = client
        .get(format!("{base_url}/api/auth/me"))
        .header("Cf-Access-Jwt-Assertion", &jwt)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(updated_cf_me["name"], "Alice Updated");
    assert!(updated_cf_me["avatarUrl"].is_null());
    let stored_alice_name_after =
        sqlx::query_scalar::<_, String>("SELECT name FROM users WHERE id = $1")
            .bind(Uuid::parse_str(alice_id).unwrap())
            .fetch_one(&tdb.pool)
            .await
            .unwrap();
    assert_eq!(stored_alice_name_after, "Alice Household");

    // Invalid CF JWT must not fall back to a valid local session cookie
    let local_session = app::auth::create_session(
        &tdb.pool,
        Uuid::parse_str(u1["id"].as_str().unwrap()).unwrap(),
    )
    .await
    .unwrap();
    let bad_cf_with_session = client
        .get(format!("{base_url}/api/auth/me"))
        .header(
            COOKIE,
            format!("{}={local_session}", app::auth::SESSION_COOKIE_NAME),
        )
        .header("Cf-Access-Jwt-Assertion", "not-a-valid-jwt")
        .send()
        .await
        .unwrap();
    assert_eq!(bad_cf_with_session.status(), StatusCode::UNAUTHORIZED);

    tdb.teardown().await;
}

#[tokio::test]
async fn household_profiles_override_name_and_serve_authenticated_avatars() {
    use base64::Engine as _;

    let Some(tdb) = TestDb::setup().await else {
        return;
    };

    let household_dir = tempfile::tempdir().expect("household tempdir");
    let png_bytes = b"\x89PNG\r\n\x1a\nhousehold-avatar";
    let png_b64 = base64::engine::general_purpose::STANDARD.encode(png_bytes);
    let yaml_path = household_dir.path().join("users.yaml");
    std::fs::write(
        &yaml_path,
        format!(
            "users:\n  robert@example.com:\n    name: Robert Crandall\n    avatar: \"data:image/png;base64,{png_b64}\"\n"
        ),
    )
    .unwrap();

    let state = AppState {
        pool: tdb.pool.clone(),
        secure_cookies: false,
        allow_open_registration: true,
        cf_access: None,
        household: Some(app::household::HouseholdProfiles::new(yaml_path.clone())),
    };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, build_router(state)).await.unwrap();
    });
    let base_url = format!("http://{addr}");

    let authed = reqwest::Client::builder()
        .cookie_store(true)
        .build()
        .unwrap();
    let anon = reqwest::Client::new();

    let reg = authed
        .post(format!("{base_url}/api/auth/register"))
        .json(&json!({
            "email": "robert@example.com",
            "password": "very-strong-passphrase"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(reg.status(), StatusCode::CREATED);
    let user: Value = reg.json().await.unwrap();
    let user_id = user["id"].as_str().expect("user id");
    assert_eq!(user["name"], "Robert Crandall");
    let avatar_url = user["avatarUrl"].as_str().expect("avatarUrl");
    assert!(avatar_url.starts_with(&format!("/api/users/{user_id}/avatar")));

    // Password login also returns hydrated household name and avatarUrl
    let login_resp = anon
        .post(format!("{base_url}/api/auth/login"))
        .json(&json!({
            "email": "robert@example.com",
            "password": "very-strong-passphrase"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(login_resp.status(), StatusCode::OK);
    let login_user: Value = login_resp.json().await.unwrap();
    assert_eq!(login_user["name"], "Robert Crandall");
    assert_eq!(login_user["avatarUrl"], avatar_url);

    // Unauthenticated callers cannot fetch avatars
    let unauth_resp = anon
        .get(format!("{base_url}{avatar_url}"))
        .send()
        .await
        .unwrap();
    assert_eq!(unauth_resp.status(), StatusCode::UNAUTHORIZED);

    // Authenticated callers receive the avatar image with nosniff header
    let avatar_resp = authed
        .get(format!("{base_url}{avatar_url}"))
        .send()
        .await
        .unwrap();
    assert_eq!(avatar_resp.status(), StatusCode::OK);
    assert_eq!(
        avatar_resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("image/png")
    );
    assert_eq!(
        avatar_resp
            .headers()
            .get(reqwest::header::X_CONTENT_TYPE_OPTIONS)
            .and_then(|v| v.to_str().ok()),
        Some("nosniff")
    );
    assert_eq!(avatar_resp.bytes().await.unwrap().as_ref(), png_bytes);

    // Bearer tokens also receive hydrated household profiles and can fetch avatars
    let tok_resp = authed
        .post(format!("{base_url}/api/tokens"))
        .json(&json!({ "name": "household-pat" }))
        .send()
        .await
        .unwrap();
    assert_eq!(tok_resp.status(), StatusCode::CREATED);
    let tok_body: Value = tok_resp.json().await.unwrap();
    let bearer = format!("Bearer {}", tok_body["token"].as_str().unwrap());

    let pat_me = anon
        .get(format!("{base_url}/api/auth/me"))
        .header(reqwest::header::AUTHORIZATION, &bearer)
        .send()
        .await
        .unwrap();
    assert_eq!(pat_me.status(), StatusCode::OK);
    let pat_user: Value = pat_me.json().await.unwrap();
    assert_eq!(pat_user["name"], "Robert Crandall");
    assert_eq!(pat_user["avatarUrl"], avatar_url);

    let pat_avatar = anon
        .get(format!("{base_url}{avatar_url}"))
        .header(reqwest::header::AUTHORIZATION, &bearer)
        .send()
        .await
        .unwrap();
    assert_eq!(pat_avatar.status(), StatusCode::OK);
    assert_eq!(pat_avatar.bytes().await.unwrap().as_ref(), png_bytes);

    // Updating users.yaml hot-reloads on /api/auth/me without changing the DB row
    std::fs::write(
        &yaml_path,
        "users:\n  robert@example.com:\n    name: Bobby\n",
    )
    .unwrap();
    let me_resp = authed
        .get(format!("{base_url}/api/auth/me"))
        .send()
        .await
        .unwrap();
    assert_eq!(me_resp.status(), StatusCode::OK);
    let me_user: Value = me_resp.json().await.unwrap();
    assert_eq!(me_user["name"], "Bobby");
    assert!(me_user["avatarUrl"].is_null());

    // Previously issued avatar URL now returns 404 after removal
    let removed_avatar_resp = authed
        .get(format!("{base_url}{avatar_url}"))
        .send()
        .await
        .unwrap();
    assert_eq!(removed_avatar_resp.status(), StatusCode::NOT_FOUND);

    let stored_name = sqlx::query_scalar::<_, String>("SELECT name FROM users WHERE id = $1")
        .bind(uuid::Uuid::parse_str(user_id).unwrap())
        .fetch_one(&tdb.pool)
        .await
        .unwrap();
    assert_eq!(stored_name, "Robert Crandall");

    tdb.teardown().await;
}
