<img src="assets/icon.svg" width="64" height="64" alt="">

# Rust Home Template

Single-repo starter template for self-hosted homelab web portals and local `stdio` MCP servers built with **Rust** (`axum`, `sqlx`, `utoipa`, `rmcp`) and **Solid 2.0** (`solid-js@2.0.0-rc`, Vite 8, Vitest 5, Bun 1.3).

Unlike the two-repo `go-home-server` + `go-home-template` split, this template includes the complete runtime foundation directly in one repository (`src/auth.rs`, `src/db.rs`, `src/config.rs`, `src/apiclient.rs`, `src/routes.rs`, `src/bin/server.rs`, `src/bin/mcp.rs`, and `src/bin/openapi.rs`). See architecture details in [`docs/tech-stack.md`](docs/tech-stack.md).

## Creating a New App from This Template

1. Create a new repository from `robert-crandall/rust-home-template` on GitHub (or clone it directly).
2. Rename all identifiers, crate names, database names, and MCP binary names in one step:

```bash
make init MODULE=github.com/robert-crandall/my-app NAME="My App"
make spec
```

## Tech Stack

- **Backend**: Rust 2024 (`1.95+`), Axum `0.8`, SQLx `0.8` (PostgreSQL `18`), `utoipa` OpenAPI `3.1`, Argon2id cookie sessions + `pat_` personal access tokens, `rust-embed` single-binary SPA serving.
- **Frontend**: Solid `2.0`, Vite `8`, Vitest `5`, Bun `1.3`, `openapi-fetch` + `openapi-typescript`.
- **MCP Server**: `rust-home-template-mcp` (`rmcp` over local `stdio`), reading credentials from `~/.config/rust-home-template.json` or `MCP_APP_URL` / `MCP_APP_TOKEN`.
- **Deployment**: Nonroot distroless container (`ghcr.io/robert-crandall/rust-home-template:main`) with built-in healthcheck and Watchtower webhook promotion.

## Local Development

Prerequisites: Rust `1.95+`, Bun `1.3+`, and PostgreSQL `18+`.

```bash
# 1. Install frontend packages, initialize .env, and build web/build once
make setup

# 2. Point DATABASE_URL in .env at a local Postgres database, then start HMR + API
make dev
```

Vite serves on `http://localhost:5173` and proxies `/api` and `/healthz` to the Rust server on `http://127.0.0.1:8080`.

### Useful Make Targets

- `make init MODULE=github.com/you/my-app NAME="My App"` - rename the template to your new project.
- `make spec` - regenerate `docs/openapi.json` and `web/src/lib/api/schema.d.ts` from Rust route definitions.
- `make check` - run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `tsc --noEmit`.
- `make test` - run Rust tests (`TEST_DATABASE_URL` enables Postgres integration tests) and frontend Vitest tests.
- `make build` - produce the standalone release binary at `bin/rust-home-template`.

## Local MCP Server (`rust-home-template-mcp`)

Install the binary into `~/bin`, sign in once to mint a personal access token (saved with `0600` permissions to `~/.config/rust-home-template.json`), and print the MCP client snippet:

```bash
make install-mcp
make mcp-token
make mcp-config
```

## Homelab Deployment (Docker Compose)

```yaml
services:
  rust-home-template:
    image: ghcr.io/robert-crandall/rust-home-template:main
    restart: unless-stopped
    ports:
      - "8080:8080"
    environment:
      DATABASE_URL: postgres://rust_home_template:${POSTGRES_PASSWORD}@db:5432/rust_home_template?sslmode=disable
      ADDR: ":8080"
      # Put HTTPS termination in front when setting APP_ENV=production;
      # leave APP_ENV unset when serving plain HTTP on a trusted LAN.
      APP_ENV: production
      ALLOW_OPEN_REGISTRATION: "true"
    depends_on:
      db:
        condition: service_healthy

  db:
    image: postgres:18
    restart: unless-stopped
    environment:
      POSTGRES_USER: rust_home_template
      POSTGRES_PASSWORD: ${POSTGRES_PASSWORD}
      POSTGRES_DB: rust_home_template
    volumes:
      - pgdata:/var/lib/postgresql
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U rust_home_template"]
      interval: 10s
      retries: 5

volumes:
  pgdata:
```

- **TLS vs. trusted-LAN HTTP**: `APP_ENV=production` marks the `rust_home_template_session` cookie `Secure`, so browsers (and `rust-home-template-mcp token` against an `http://` URL) drop it over plain HTTP. Put TLS termination in front of the container in production (preserving the incoming `Host` header, e.g., `proxy_set_header Host $host` in `nginx`, so same-origin CSRF checks match `Origin`), or leave `APP_ENV` unset when accessing `http://<homelab-ip>:8080` directly on a trusted home network.
- **Household onboarding**: Each household member registers with their own email and password. Because anyone who can reach the server can register while `ALLOW_OPEN_REGISTRATION=true`, flip `ALLOW_OPEN_REGISTRATION=false` once your household accounts are created to lock new sign-ups.

