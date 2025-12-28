# Stage 1: Build the application in a full Rust environment
FROM rust:1-slim-bookworm AS builder

# Install build dependencies required by crates like `openssl-sys`
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev build-essential

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
COPY rust-server/src ./src
RUN touch src/main.rs && cargo build --release --bin xau-scalper-server

# Stage 2: Create the final, minimal production image
FROM debian:bookworm-slim

# Install runtime dependencies: SSL certificates for HTTPS requests, and utilities for data conversion.
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates openssl dos2unix gawk libssl3 libstdc++6 && rm -rf /var/lib/apt/lists/*

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
ENV APP_SERVER__HOST=0.0.0.0 \
    APP_SERVER__PORT=3000 \
    APP_TRADING__MAX_BUFFER_SIZE=500 \
    APP_PATHS__PUSH_TOKENS_FILE=/app/push_tokens.json \
    APP_PATHS__MODELS_DIR=/app/models/ \
    RUST_LOG=info

# Set the default container command to run the server
CMD ["xau_scalper_server"]