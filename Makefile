.PHONY: help init icons setup build install-mcp mcp-token mcp-config run dev test check spec clean

.DELETE_ON_ERROR:

help: ## Show this help
	@grep -hE '^[a-zA-Z_-]+:.*?## ' $(MAKEFILE_LIST) \
		| awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-12s\033[0m %s\n", $$1, $$2}'

init: ## Initialize template (MODULE=github.com/you/app NAME="My App" ICON=path/to/icon.png [SLUG=my-app])
	@scripts/init.sh "$(MODULE)" "$(NAME)" "$(SLUG)" "$(ICON)"

icons: ## Regenerate PWA icons from a square PNG >= 512x512 (ICON=path/to/icon.png)
	@test -n "$(ICON)" || { echo "Usage: make icons ICON=path/to/icon.png" >&2; exit 1; }
	@python3 -B scripts/render-icons.py "$(ICON)"

setup: ## First run after cloning: frontend deps, .env, and an initial frontend build
	cd web && bun install --frozen-lockfile
	@test -f .env || { cp .env.example .env && echo "wrote .env from .env.example"; }
	cd web && bun run build

build: ## Build the Solid 2.0 SPA, then compile the release binary into bin/
	cd web && bun run build
	mkdir -p bin
	cargo build --locked --release --bin rust-home-server
	cp target/release/rust-home-server bin/rust-home-server

install-mcp: ## Build and install rust-home-server-mcp into ~/bin
	mkdir -p "$(HOME)/bin"
	cargo build --locked --release --bin rust-home-server-mcp
	cp target/release/rust-home-server-mcp "$(HOME)/bin/rust-home-server-mcp"
	@echo "installed $(HOME)/bin/rust-home-server-mcp"

mcp-token: ## Sign in and mint an MCP token into ~/.config/rust-home-server.json
	cargo run --locked --bin rust-home-server-mcp -- token

mcp-config: ## Print the local stdio MCP client configuration
	@printf '{\n  "mcpServers": {\n    "rust-home-server": { "command": "%s/bin/rust-home-server-mcp", "args": ["serve"] }\n  }\n}\n' \
		"$(HOME)"

run: ## Run the release binary built by `make build`
	./bin/rust-home-server

dev: ## Run Vite on :5173 with the Rust API server on :8080
	@scripts/dev.sh

web/build/index.html: web/index.html web/public/manifest.webmanifest web/public/icon-192.png web/public/icon-512.png web/public/theme.js
	cd web && bun run build

test: web/build/index.html ## Run Rust unit/integration tests and frontend Vitest suite
	cargo test --locked
	cd web && bun run test

check: web/build/index.html ## Run rustfmt check, clippy (-D warnings), and frontend tsc
	cargo fmt --all -- --check
	cargo clippy --all-targets --locked -- -D warnings
	cd web && bun run check

spec: ## Regenerate docs/openapi.json and web/src/lib/api/schema.d.ts from Rust routes
	cargo run --locked --bin openapi
	cd web && bun run gen:api

clean: ## Remove build artifacts
	rm -rf target bin .bin web/build
