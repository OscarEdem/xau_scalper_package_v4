# Stage 1: Build the application using cargo-chef for optimal dependency caching
# Using Ubuntu 24.04 (Noble) as builder for glibc 2.39 compatibility (required by ort/ONNX Runtime)
FROM ubuntu:24.04 AS chef
RUN apt-get update && apt-get install -y --no-install-recommends \
    curl ca-certificates build-essential pkg-config libssl-dev dos2unix && \
    rm -rf /var/lib/apt/lists/*

# Install Rust
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable
ENV PATH="/root/.cargo/bin:${PATH}"

# Install cargo-chef (download binary instead of compiling from source to save ~3-5 mins)
RUN curl -L https://github.com/LukeMathWalker/cargo-chef/releases/latest/download/cargo-chef-x86_64-unknown-linux-musl.tar.gz | tar xz -C /usr/local/bin

WORKDIR /app

FROM chef AS planner
# Copy only the files needed for dependency resolution
# We only copy the rust-server folder as it's the main workspace
COPY rust-server/Cargo.toml rust-server/Cargo.lock ./rust-server/
COPY rust-server/src ./rust-server/src
WORKDIR /app/rust-server
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
WORKDIR /app/rust-server
COPY --from=planner /app/rust-server/recipe.json recipe.json
# Build dependencies - this layer is cached until Cargo.lock changes.
# This is the longest part of the build (5-8 mins), now fully cached.
RUN cargo chef cook --release --recipe-path recipe.json

# Copy actual source code and build the final binary
COPY rust-server/migrations ./migrations
COPY rust-server/src ./src
# Normalize line endings in migrations (dos2unix installed in chef stage)
RUN find migrations -type f -exec dos2unix {} +
RUN cargo build --release --bin xau-scalper-server

# Stage 2: Create the final, minimal production image
FROM ubuntu:24.04
LABEL org.opencontainers.image.source=https://github.com/OscarEdem/xau_scalper_package_v4

# Install runtime dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates openssl libssl3 libstdc++6 && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy the compiled binary from the builder
COPY --from=builder /app/rust-server/target/release/xau-scalper-server /usr/local/bin/xau_scalper_server

# Copy model assets
# We copy them directly into /app/models to keep the structure clean
COPY rust-server/src/engines/gbm/models /app/models
COPY rust-server/src/engines/heston/models /app/models
COPY rust-server/src/engines/lstm/models /app/models

# Expose port
EXPOSE 3000

# Set production environment variables
ENV APP__SERVER__HOST=0.0.0.0 \
    APP__SERVER__PORT=3000 \
    APP__TRADING__MAX_BUFFER_SIZE=500 \
    APP__PATHS__PUSH_TOKENS_FILE=/app/push_tokens.json \
    APP__PATHS__MODELS_DIR=/app/models/ \
    APP__PATHS__SESSIONS_DIR=/app/sessions_data \
    RUST_LOG=info,xau_scalper_server=debug

CMD ["xau_scalper_server"]