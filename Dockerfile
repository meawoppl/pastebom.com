# =============================================================================
# Stage 1: Build Rust binaries
# =============================================================================
FROM rust:1.93-bookworm AS builder

RUN apt-get update && apt-get install -y \
    cmake \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Install trunk and wasm target for viewer build.
# Pin the version and use --locked so the build resolves trunk's own tested
# dependency set; an unpinned install picks up broken transitive deps (e.g. a
# cssparser/cssparser-macros mismatch) and fails to compile.
RUN cargo install trunk --version 0.21.14 --locked \
    && cargo install wasm-pack --version 0.15.0 --locked \
    && rustup target add wasm32-unknown-unknown

WORKDIR /build

# Cache dependencies by copying manifests first
COPY Cargo.toml Cargo.toml
COPY Cargo.lock Cargo.lock
COPY crates/pcb-extract/Cargo.toml crates/pcb-extract/Cargo.toml
COPY crates/server/Cargo.toml crates/server/Cargo.toml
COPY crates/viewer/Cargo.toml crates/viewer/Cargo.toml
COPY crates/gds-viewer/Cargo.toml crates/gds-viewer/Cargo.toml
COPY crates/gerber-view/Cargo.toml crates/gerber-view/Cargo.toml

# Create dummy source files for dependency caching
RUN mkdir -p crates/pcb-extract/src crates/server/src crates/server/static crates/viewer/src crates/gds-viewer/src crates/gerber-view/src \
    && echo "pub fn main() {}" > crates/pcb-extract/src/main.rs \
    && echo "pub fn lib() {}" > crates/pcb-extract/src/lib.rs \
    && echo "fn main() {}" > crates/server/src/main.rs \
    && echo "<html></html>" > crates/server/static/index.html \
    && echo "fn main() {}" > crates/viewer/src/main.rs \
    && echo "fn main() {}" > crates/gds-viewer/src/main.rs \
    && echo "pub fn lib() {}" > crates/gerber-view/src/lib.rs

# Build dependencies only (cached layer)
RUN cargo build --release --locked 2>/dev/null || true

# Copy actual source code
COPY crates/ crates/

# Touch source files to invalidate cache
RUN touch crates/pcb-extract/src/main.rs \
    crates/pcb-extract/src/lib.rs \
    crates/server/src/main.rs \
    crates/viewer/src/main.rs \
    crates/gds-viewer/src/main.rs \
    crates/gerber-view/src/lib.rs

# Build release binaries
RUN cargo build --release --locked

# Build viewer WASM
RUN cd crates/viewer && trunk build --release

# Build GDSII tile viewer WASM
RUN cd crates/gds-viewer && trunk build --release

# Build the embeddable Gerber viewer bundle
RUN cd crates/gerber-view && wasm-pack build --release --target web --out-dir pkg

# =============================================================================
# Stage 2: Runtime
# =============================================================================
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy binaries and viewer assets from builder
COPY --from=builder /build/target/release/pastebom-server /app/pastebom-server
COPY --from=builder /build/target/release/pcb-extract /app/pcb-extract
COPY --from=builder /build/crates/viewer/dist /app/viewer
COPY --from=builder /build/crates/gds-viewer/dist /app/gview
COPY --from=builder /build/crates/gerber-view/pkg /app/gerber-view/pkg
COPY --from=builder /build/crates/gerber-view/examples /app/gerber-view/examples

ENV BIND_ADDR=0.0.0.0:8080
ENV VIEWER_DIR=/app/viewer
ENV GDS_VIEWER_DIR=/app/gview
ENV GERBER_VIEW_DIR=/app/gerber-view
ENV STORAGE_PATH=/app/data
ENV RUST_LOG=info
ENV MAX_UPLOAD_SIZE=52428800

EXPOSE 8080

HEALTHCHECK --interval=30s --timeout=5s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:8080/health || exit 1

CMD ["/app/pastebom-server"]
