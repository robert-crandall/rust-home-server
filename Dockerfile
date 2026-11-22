# Homelab production image: one Rust binary with embedded Solid 2.0 SPA,
# running on nonroot distroless with no shell or package manager.

# --- 1. Build the Solid 2.0 SPA ---
# Use Debian slim (glibc) rather than Alpine (musl) because @solidjs/compiler
# ships native Linux prebuilds for gnu targets only.
FROM --platform=$BUILDPLATFORM oven/bun:1.3-slim AS web-build
WORKDIR /src/web

COPY web/package.json web/bun.lock web/bunfig.toml ./
RUN bun install --frozen-lockfile

COPY web/ ./
RUN bun run build

# --- 2. Build the Rust server binary ---
FROM rust:1.95-bookworm AS rust-build
WORKDIR /src

COPY Cargo.toml Cargo.lock ./
COPY migrations ./migrations
COPY src ./src

# .dockerignore excludes web/build so the embedded SPA always comes from stage 1.
COPY --from=web-build /src/web/build ./web/build

RUN mkdir -p /out \
    && cargo build --locked --release --bin rust-home-template \
    && strip target/release/rust-home-template \
    && cp target/release/rust-home-template /out/app

# --- 3. Distroless nonroot runtime ---
FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=rust-build /out/app /app

EXPOSE 8080

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["/app", "healthcheck"]

ENTRYPOINT ["/app"]
