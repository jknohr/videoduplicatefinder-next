# ── Build stage ───────────────────────────────────────────────────────────────
FROM rust:1.87-bookworm AS builder

# System dependencies needed at build time: FFmpeg dev headers + pkg-config
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libavcodec-dev \
    libavformat-dev \
    libavfilter-dev \
    libavdevice-dev \
    libavutil-dev \
    libswresample-dev \
    libswscale-dev \
    clang \
    libclang-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Layer the dependency fetch separately from the source build.
# Copy workspace manifest and all crate Cargo.tomls, then build deps.
COPY Cargo.toml Cargo.lock ./
COPY core/Cargo.toml core/
COPY ui/Cargo.toml ui/
COPY cli/Cargo.toml cli/

# Stub src directories so cargo can resolve the workspace without source
RUN mkdir -p core/src ui/src cli/src \
    && echo 'fn main() {}' > ui/src/main.rs \
    && echo 'fn main() {}' > cli/src/main.rs \
    && echo '' > core/src/lib.rs

# Build dependencies only (the actual source is replaced below)
RUN cargo build --release --features web,server -p ui \
    ; cargo build --release -p cli \
    ; true

# Remove stub artifacts so the real build picks up our sources
RUN find target/release -name "*.d" -delete

# Now copy real sources and do the full build
COPY . .

# Build the web server binary (Axum + Dioxus WASM bundle)
RUN cargo build --release -p ui --features web,server
# Build the CLI binary
RUN cargo build --release -p cli

# ── Runtime stage ─────────────────────────────────────────────────────────────
FROM debian:bookworm-slim

# FFmpeg runtime + VA-API drivers for hardware-accelerated decode
RUN apt-get update && apt-get install -y --no-install-recommends \
    ffmpeg \
    libavcodec60 \
    libavformat60 \
    libavfilter9 \
    libswresample4 \
    libswscale7 \
    # VA-API runtime (Intel / AMD)
    libva-drm2 \
    libva-x11-2 \
    vainfo \
    mesa-va-drivers \
    i965-va-driver \
    # CA certs for HTTPS outbound (MusicBrainz, TVDB etc. in future)
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy binaries from builder
COPY --from=builder /build/target/release/mediaorganizer-ui  ./mediaorganizer-ui
COPY --from=builder /build/target/release/mediaorganizer-cli ./mediaorganizer-cli

# Dioxus web target also generates a `dist/` folder of static assets
COPY --from=builder /build/target/dx/ui/release/web/public ./public

# Default environment
ENV RUST_LOG=info
ENV VDF_DB_PATH=/data/db
ENV VDF_CONFIG_PATH=/config

# Data and config volumes
VOLUME ["/data", "/config", "/media"]

EXPOSE 8080

ENTRYPOINT ["./mediaorganizer-ui"]
CMD ["--port", "8080"]
