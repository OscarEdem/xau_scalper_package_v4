# Stage 1: Build the application in a full Rust environment
# Using Ubuntu 24.04 (Noble) as builder because its glibc 2.39 satisfies the
# prebuilt ONNX Runtime binaries downloaded by `ort` (ort-sys >=2.0-rc.10)
# which require glibc 2.38+ symbols (__isoc23_strtoll etc.).
FROM ubuntu:24.04 AS rust-installer
RUN apt-get update && apt-get install -y --no-install-recommends \
    curl ca-certificates build-essential && \
    rm -rf /var/lib/apt/lists/*
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable
ENV PATH="/root/.cargo/bin:${PATH}"

FROM ubuntu:24.04 AS builder
COPY --from=rust-installer /root/.cargo /root/.cargo
COPY --from=rust-installer /root/.rustup /root/.rustup
ENV PATH="/root/.cargo/bin:${PATH}"

# Install build dependencies required by crates like `openssl-sys`
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev build-essential dos2unix && \
    rm -rf /var/lib/apt/lists/*

# Use /app as the working directory
WORKDIR /app

# Copy dependency definitions to cache them
COPY rust-server/Cargo.toml rust-server/Cargo.lock ./rust-server/
# Create a dummy src directory to build only dependencies
RUN mkdir -p rust-server/src && echo "fn main() {}" > rust-server/src/main.rs
WORKDIR /app/rust-server
RUN cargo build --release

# Copy the actual source code and build the final binaries
# Ensure migrations are present for the sqlx::migrate! macro
COPY rust-server/migrations ./migrations
# Normalize line endings to LF before the macro runs to ensure checksum matches production
RUN find migrations -type f -exec dos2unix {} +
COPY rust-server/src ./src
RUN touch src/main.rs && cargo build --release --bin xau-scalper-server

# Stage 2: Create the final, minimal production image
# Must use Ubuntu 24.04 (glibc 2.39) to match the builder so that the
# prebuilt ONNX Runtime shared objects loaded at runtime are compatible.
FROM ubuntu:24.04

# Add label to link this image to the source repository
LABEL org.opencontainers.image.source=https://github.com/OscarEdem/xau_scalper_package_v4

# Install runtime dependencies: SSL certificates for HTTPS requests, and utilities for data conversion.
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates openssl dos2unix gawk libssl3 libstdc++6 && \
    rm -rf /var/lib/apt/lists/*

# Copy the compiled binaries from the builder stage
COPY --from=builder /app/rust-server/target/release/xau-scalper-server /usr/local/bin/xau_scalper_server

# Expose the port the app runs on (as seen in main.rs)
EXPOSE 3000

# Set a working directory for the application
WORKDIR /app

# Copy the CSV data and all model assets (configs, ONNX files, etc.) into the final image.
# Copying the entire `engines` directory ensures all current and future models are included.
# COPY m1_data.csv /app/m1_data.csv
# COPY m5_data.csv /app/m5_data.csv
COPY rust-server/src/engines /app/src/engines

# Copy model assets into `/app/models` so runtime code can load them using ./models/...
RUN mkdir -p /app/models
COPY rust-server/src/engines/gbm/models /app/models
COPY rust-server/src/engines/heston/models /app/models
COPY rust-server/src/engines/lstm/models /app/models

# Convert the tab-delimited data to comma-separated for the backtester
# This is a more robust way to convert TSV to CSV than using awk.
# It handles the header and data separately and combines them.
# RUN dos2unix /app/m1_data.csv && \
#     dos2unix /app/m5_data.csv && \
#     (head -n 1 /app/m1_data.csv | tr '\t' ',' && tail -n +2 /app/m1_data.csv | tr '\t' ',') > /app/m1_data_comma.csv && \
#     (head -n 1 /app/m5_data.csv | tr '\t' ',' && tail -n +2 /app/m5_data.csv | tr '\t' ',') > /app/m5_data_comma.csv

# Set production environment variables
# NOTE: The config crate uses double-underscore (__) as the hierarchy separator.
# APP__SERVER__PORT maps to settings.server.port etc.
ENV APP__SERVER__HOST=0.0.0.0 \
    APP__SERVER__PORT=3000 \
    APP__TRADING__MAX_BUFFER_SIZE=500 \
    APP__PATHS__PUSH_TOKENS_FILE=/app/push_tokens.json \
    APP__PATHS__MODELS_DIR=/app/models/ \
    APP__PATHS__SESSIONS_DIR=/app/sessions_data \
    RUST_LOG=info,xau_scalper_server=debug

# Set the default container command to run the server
CMD ["xau_scalper_server"]