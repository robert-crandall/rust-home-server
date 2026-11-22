.PHONY: help init setup build install-mcp mcp-token mcp-config run dev test check spec clean

.DELETE_ON_ERROR:

help: ## Show this help
	@grep -hE '^[a-zA-Z_-]+:.*?## ' $(MAKEFILE_LIST) \
		| awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-12s\033[0m %s\n", $$1, $$2}'

init: ## Initialize template (MODULE=github.com/you/app NAME="My App" [SLUG=my-app])
	@scripts/init.sh "$(MODULE)" "$(NAME)" "$(SLUG)"

setup: ## First run after cloning: frontend deps, .env, and an initial frontend build
	cd web && bun install --frozen-lockfile
	@test -f .env || { cp .env.example .env && echo "wrote .env from .env.example"; }
	cd web && bun run build

build: ## Build the Solid 2.0 SPA, then compile the release binary into bin/
	cd web && bun run build
	mkdir -p bin
	cargo build --locked --release --bin rust-home-template
	cp target/release/rust-home-template bin/rust-home-template

install-mcp: ## Build and install rust-home-template-mcp into ~/bin
	mkdir -p "$(HOME)/bin"
	cargo build --locked --release --bin rust-home-template-mcp
	cp target/release/rust-home-template-mcp "$(HOME)/bin/rust-home-template-mcp"
	@echo "installed $(HOME)/bin/rust-home-template-mcp"

mcp-token: ## Sign in and mint an MCP token into ~/.config/rust-home-template.json
	cargo run --locked --bin rust-home-template-mcp -- token

mcp-config: ## Print the local stdio MCP client configuration
	@printf '{\n  "mcpServers": {\n    "rust-home-template": { "command": "%s/bin/rust-home-template-mcp", "args": ["serve"] }\n  }\n}\n' \
		"$(HOME)"

run: ## Run the release binary built by `make build`
	./bin/rust-home-template

dev: ## Run Vite on :5173 with the Rust API server on :8080
	@scripts/dev.sh

web/build/index.html:
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
