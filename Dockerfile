# =============================================================================
# Dockerfile — Multi-stage production build for adk-gateway
# =============================================================================
# Stage 1: Rust builder (compiles the binary with optional feature flags)
# Stage 2: Minimal Alpine runtime (final image < 100MB)
#
# Build examples:
#   docker build -t adk-gateway .
#   docker build --build-arg FEATURES="browser,postgres" -t adk-gateway .
#   docker build --build-arg RUST_VERSION=1.85.0 -t adk-gateway .
#
# Run examples:
#   docker run -p 3000:3000 -v ./config:/etc/adk-gateway adk-gateway
#   docker run -e ADK_CONFIG_PATH=/etc/adk-gateway/gateway.json adk-gateway
# =============================================================================

# ─── Build Arguments ────────────────────────────────────────────────────────
# Rust toolchain version
ARG RUST_VERSION=1.85.0

# Feature flags to enable (comma-separated, e.g., "browser,postgres,acp")
ARG FEATURES=""

# Build profile: release (optimized) or dev (faster builds for testing)
ARG BUILD_PROFILE=release

# =============================================================================
# Stage 1: Builder — Compile the Rust binary
# =============================================================================
FROM rust:${RUST_VERSION}-alpine AS builder

# Install build dependencies for musl-based static linking
# - musl-dev: C standard library for static linking
# - pkgconfig: for finding system libraries
# - openssl-dev/openssl-libs-static: TLS support
# - sqlite-dev: SQLite bundled compilation support
RUN apk add --no-cache \
    musl-dev \
    pkgconfig \
    openssl-dev \
    openssl-libs-static \
    sqlite-dev \
    perl \
    make

# Set environment for static OpenSSL linking
ENV OPENSSL_STATIC=1
ENV OPENSSL_DIR=/usr

WORKDIR /build

# ─── Dependency Caching Layer ───────────────────────────────────────────────
# Copy only dependency manifests first to leverage Docker layer caching.
# Dependencies are rebuilt only when Cargo.toml or Cargo.lock change.
COPY Cargo.toml Cargo.lock ./
COPY build.rs ./

# Create a dummy main.rs to compile dependencies without source code
RUN mkdir -p src && \
    echo 'fn main() { println!("dependency build"); }' > src/main.rs

# Build dependencies only (this layer is cached unless manifests change)
ARG FEATURES
RUN if [ -n "${FEATURES}" ]; then \
        cargo build --release --features "${FEATURES}" 2>/dev/null || true; \
    else \
        cargo build --release 2>/dev/null || true; \
    fi && \
    rm -rf src

# ─── Full Source Build ──────────────────────────────────────────────────────
# Copy the actual source code
COPY src/ src/
COPY examples/ examples/

# Touch main.rs to ensure it's rebuilt with actual source
RUN touch src/main.rs

# Build the final binary with optional features
ARG BUILD_PROFILE
RUN if [ -n "${FEATURES}" ]; then \
        cargo build --release --features "${FEATURES}"; \
    else \
        cargo build --release; \
    fi

# Strip the binary to reduce size (saves ~30-50%)
RUN strip /build/target/release/adk-gateway

# =============================================================================
# Stage 2: Runtime — Minimal Alpine image
# =============================================================================
FROM alpine:3.20 AS runtime

# Install minimal runtime dependencies:
# - ca-certificates: for HTTPS/TLS connections to APIs
# - tini: lightweight init for proper signal handling (PID 1)
# - curl: for HEALTHCHECK endpoint probing
# - libgcc: runtime support for Rust binaries
RUN apk add --no-cache \
    ca-certificates \
    tini \
    curl \
    libgcc

# Create a non-root user for security
RUN addgroup -S gateway && adduser -S gateway -G gateway

# Create required directories with proper ownership
RUN mkdir -p /etc/adk-gateway /var/log/adk-gateway /var/lib/adk-gateway && \
    chown -R gateway:gateway /etc/adk-gateway /var/log/adk-gateway /var/lib/adk-gateway

# Copy the compiled binary from the builder stage
COPY --from=builder /build/target/release/adk-gateway /usr/local/bin/adk-gateway

# ─── Configuration ──────────────────────────────────────────────────────────
# Configuration can be provided via:
# 1. Environment variables (ADK_CONFIG_PATH, TELEGRAM_BOT_TOKEN, etc.)
# 2. Mounted config file at /etc/adk-gateway/gateway.json
# 3. Combination of both (env vars override file values)

# Default config file path (can be overridden with ADK_CONFIG_PATH env var)
ENV ADK_CONFIG_PATH=/etc/adk-gateway/gateway.json

# Default port the gateway listens on
ENV ADK_GATEWAY_PORT=3000

# Log format: "json" (default, structured) or "pretty" (human-readable)
ENV LOG_FORMAT=json

# Graceful shutdown drain timeout in seconds
ENV ADK_DRAIN_TIMEOUT_SECS=30

# ─── Health Check ───────────────────────────────────────────────────────────
# Verify the gateway is healthy by calling the /health endpoint.
# - interval: check every 30 seconds
# - timeout: fail if no response within 5 seconds
# - start-period: allow 10 seconds for startup before checking
# - retries: mark unhealthy after 3 consecutive failures
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -f http://localhost:${ADK_GATEWAY_PORT}/health || exit 1

# ─── Runtime Configuration ──────────────────────────────────────────────────
# Expose the default gateway port
EXPOSE ${ADK_GATEWAY_PORT}

# Volume mount points for persistent data and configuration
VOLUME ["/etc/adk-gateway", "/var/log/adk-gateway", "/var/lib/adk-gateway"]

# Switch to non-root user
USER gateway

# Use tini as PID 1 for proper signal forwarding (SIGTERM → graceful shutdown)
ENTRYPOINT ["tini", "--"]

# Start the gateway with config from environment
CMD ["adk-gateway", "--config", "/etc/adk-gateway/gateway.json"]
