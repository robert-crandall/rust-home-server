# Tech Stack & Architecture Decisions

`rust-home-template` collapses the two-repo homelab pattern (`robert-crandall/go-home-template` + `robert-crandall/go-home-server`) into a single **Rust + Solid 2.0** starter repository extracted from `robert-crandall/scholarships`.

## Stack Summary

| Layer | Choice | Why |
| --- | --- | --- |
| HTTP & API contract | Rust 2024 (`1.95`), `axum 0.8`, `utoipa 6` + `utoipa-axum 0.3` | Route handlers and OpenAPI 3.1 schemas live in one place (`src/routes.rs`) so endpoints and docs cannot drift. |
| Database | PostgreSQL `18`, `sqlx 0.8` (`runtime-tokio-rustls`) | Embedded SQL migrations (`migrations/`), async connection pooling, and zero OpenSSL runtime coupling. |
| Authentication | Argon2id (`m=19 MiB, t=2, p=1`), 30-day sliding cookie sessions, `pat_` bearer tokens | Direct homelab exposure without an auth proxy today; household members get separate logins over one shared database with per-user sessions and tokens. |
| MCP server | `rust-home-template-mcp` (`rmcp 3.11` over `stdio`) | Matches `go-home-server` ergonomics: `<slug>-mcp token` mints and stores `~/.config/<slug>.json` (`0600`), and `<slug>-mcp serve` runs locally in any MCP client. |
| Frontend | Solid `2.0` (`solid-js@2.0.0-rc`), Vite `8`, Vitest `5`, Bun `1.3`, `openapi-fetch` | Fine-grained reactivity with strict LLM-targeted diagnostics (`@solidjs/diagnostics`) and generated API types (`web/src/lib/api/schema.d.ts`). |
| Container & CD | Multi-stage `Dockerfile` -> `gcr.io/distroless/cc-debian12:nonroot`, GHCR + Watchtower | Single binary with embedded `web/build`, built-in `["/app", "healthcheck"]` probe, immutable `:sha-<40hex>` tags, and serialized `:main` promotion. |

## Key Invariants

### 1. Decoupled OpenAPI Generation (`src/bin/openapi.rs`)

`src/bin/server.rs` owns `#[derive(Embed)] #[folder = "web/build"]`, while `src/routes.rs` owns `build_openapi_router()`. Because `src/bin/openapi.rs` calls `split_for_parts()` on `build_openapi_router()` without touching `rust-embed` or `PgPool`, `make spec` runs cleanly with neither `web/build` nor `DATABASE_URL`. CI's `spec` job enforces zero contract drift on every PR.

### 2. Multi-User Household Workspace

Each household member registers or signs in with their own email and password (`users`), sessions (`sessions`), and personal access tokens (`api_tokens`). `GET /api/users` and MCP `list_users` expose the household directory so domain tables can combine shared household records with per-user state. `ALLOW_OPEN_REGISTRATION` defaults to `true` for frictionless household onboarding and can be flipped to `false` once accounts are created (which also enforces an advisory-locked first-user bootstrap check on fresh databases).

### 3. Cookie vs. Bearer Separation & CSRF Defense

- Browser requests authenticate via `rust_home_template_session` (`HttpOnly`, `SameSite=Lax`, `Secure` in `APP_ENV=production`) with a 30-day TTL renewed when fewer than 15 days remain.
- Unsafe cookie-authenticated methods must pass `security_middleware`: `Sec-Fetch-Site: cross-site` is rejected immediately, and any present `Origin` header must match the request `Host`.
- Personal access tokens (`Authorization: Bearer pat_<uuid>_<secret>`) hash secrets with SHA-256 and compare digests in constant time (`subtle::ConstantTimeEq`). When an `Authorization` header is present, session cookies are ignored so a malformed or revoked bearer token never silently succeeds via ambient browser state.
- `/api/tokens` management endpoints require `SessionUser` (cookie session only), preventing a leaked bearer token from minting replacement tokens.
