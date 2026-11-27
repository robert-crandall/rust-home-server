# AGENTS.md

Guidance for AI coding agents working in `robert-crandall/rust-home-server`.

## Overview

`rust-home-server` is a self-hosted homelab web application and local `stdio` MCP server starter built on **Rust + Solid 2.0**.

- **Backend**: Rust 2024 (`1.95+`), Axum `0.8`, SQLx `0.8` (PostgreSQL 18), `utoipa` + `utoipa-axum` (OpenAPI 3.1), Argon2id sessions + `pat_` personal access tokens, `rust-embed` SPA serving.
- **MCP server**: `rust-home-server-mcp` (`rmcp` over local `stdio`), talking to the HTTP API using `~/.config/rust-home-server.json` (`MCP_APP_URL` / `MCP_APP_TOKEN` overrides).
- **Frontend**: Solid `2.0` (`solid-js@2.0.0-rc.*`, `@solidjs/web`, `@solidjs/vite-plugin`), Tailwind CSS `4` (`@tailwindcss/vite`) + DaisyUI `5`, Vite `8`, Vitest `5`, Bun `1.3.x`, and `openapi-fetch` + `openapi-typescript`.

## Authentication & Security Model

- Registration defaults to first-account-only (`ALLOW_OPEN_REGISTRATION=false`, enforced under a Postgres transaction advisory lock); set `ALLOW_OPEN_REGISTRATION=true` for multi-user apps where each user registers their own account.
- Browser mutations (`/api/...` unsafe methods) are protected by `HttpOnly; SameSite=Lax` cookies plus `Sec-Fetch-Site` / `Origin` CSRF checks in `src/auth.rs`.
- Bearer token callers (`Authorization: Bearer pat_<uuid>_<secret>`) bypass cookie fallback completely: if an `Authorization` header is present, cookies are never consulted.

## Single-Source-of-Truth API Contract

1. Add or update Axum handlers and request/response `#[derive(ToSchema)]` structs in `src/routes.rs` (or domain route modules wired into `routes::build_openapi_router`).
2. Run `make spec` to regenerate both `docs/openapi.json` and `web/src/lib/api/schema.d.ts`.
3. Never hand-edit `docs/openapi.json` or `web/src/lib/api/schema.d.ts`. CI's `spec` job runs `make spec` without Postgres or `web/build` and fails on any diff.
4. Keep `#[derive(Embed)]` isolated to `src/bin/server.rs` so `cargo run --bin openapi` never depends on `web/build`.
5. Keep `[profile.release.build-override] debug = 1` in `Cargo.toml` so macOS `ld`/`dyld` links `sqlx-macros` cleanly during `--release` builds without changing the stripped release binaries.

## Solid 2.0 Rules (Do Not Write Solid 1.x or React)

Read `web/node_modules/solid-js/CHEATSHEET.md` whenever touching `web/`. Solid 2.0 differs from Solid 1.x and React in ways that compiler and runtime diagnostics enforce:

1. **Imports & JSX**:
   - `tsconfig.json` must keep `"jsx": "preserve"` and `"jsxImportSource": "@solidjs/web"`.
   - Import `render` and `type JSX` from `"@solidjs/web"`.
   - Import `createSignal`, `createMemo`, `createEffect`, `createStore`, `reconcile`, `createContext`, `useContext`, `Show`, `For`, `Switch`, `Match`, `Loading`, `Errored`, `onSettled`, `flush`, `refresh`, and `isPending` from `"solid-js"`. Subpaths `"solid-js/web"` and `"solid-js/store"` do not exist in Solid 2.0.
2. **Effects & Lifecycle**:
   - `createEffect` always takes **two** arguments: `createEffect(() => compute(), (value) => apply(value))`. Never read reactive signals inside the effect callback (`[STRICT_READ_UNTRACKED]`). Prefer derived functions or `createMemo` over syncing signals in an effect.
   - Use `onSettled(() => { ...; return () => cleanup(); })` for DOM mount and cleanup (`onMount` and `onCleanup` are removed).
3. **Context, Props, and HTML Attributes**:
   - Render context directly as `<AuthContext value={store}>` (`<AuthContext.Provider>` is removed).
   - Never destructure `props` in component parameters (`function Card(props: Props)` -> `props.title`). Pass values, not signal accessors (`<Card title={title()} />`).
   - Conditional classes use `class={["base", { active: isSelected() }]}` (`classList` is removed).
   - Use standard lowercase HTML attribute names (`for`, `class`, `minlength`, `maxlength`, `autocomplete`), not React camelCase (`htmlFor`, `className`, `minLength`, `maxLength`).
4. **Testing Reactivity**:
   - Signal and store writes flush on a microtask. In synchronous Vitest assertions, call `flush()` from `"solid-js"` after dispatching events or updating signals before inspecting the DOM.

## ADHD-Friendly UI & Theme Rules

- **One job per view**: Keep primary screens calm and scannable. Put secondary details, long configuration, and setup controls inside progressive disclosures (`<details>` or drawers) rather than showing everything at once.
- **DaisyUI 5 + semantic theme tokens**: Configure DaisyUI 5 themes (`light` and `dark`, with `themes: false` so stock themes do not shadow custom tokens) and semantic `@theme` tokens in `web/src/themes.css`, and keep only minimal `@layer base` / `@layer utilities` recipes in `web/src/app.css` (override DaisyUI component default variables inside `@layer daisyui.l1.l2.l3` so modifiers and utilities still win). Use DaisyUI component classes and semantic Tailwind utilities (`bg-base-100`, `text-muted`, etc.); never hardcode hex palettes or `dark:` color patches inside components.
- **Zero-FOUC theme switching**: `web/public/theme.js` owns `window.appTheme` (`system`, `light`, `dark`); UI controls call `window.appTheme` instead of duplicating `localStorage` logic.

## Commands

- `make init MODULE=github.com/you/app NAME="My App"` - rename all template references for a new project.
- `make setup` - install frontend dependencies, copy `.env` if absent, and build `web/build` (if behind GitHub CFS, run `cd web && gh cfs -- bun install --frozen-lockfile`).
- `make dev` - start the Rust API server on `:8080` and Vite HMR on `:5173`.
- `make spec` - regenerate `docs/openapi.json` and `web/src/lib/api/schema.d.ts`.
- `make check` - run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and frontend `tsc --noEmit`.
- `make test` - run Rust unit + integration tests (`TEST_DATABASE_URL` enables Postgres tests) and frontend Vitest tests.
- `make install-mcp` / `make mcp-token` / `make mcp-config` - install and configure `rust-home-server-mcp` for local `stdio` MCP clients.
