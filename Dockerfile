# Stage 1: Build the application in a full Rust environment
FROM rust:1-slim-bookworm AS builder

# Install build dependencies required by crates like `openssl-sys`
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev

# Use /app as the working directory
WORKDIR /app

# Copy dependency definitions to cache them
COPY rust-server/Cargo.toml rust-server/Cargo.lock ./rust-server/
# Create a dummy src directory to build only dependencies
RUN mkdir -p rust-server/src && echo "fn main() {}" > rust-server/src/main.rs
WORKDIR /app/rust-server
RUN cargo build --release

# Copy the actual source code and build the final binaries
COPY rust-server/src ./src
RUN touch src/main.rs && cargo build --release

# Stage 2: Create the final, minimal production image
FROM debian:bookworm-slim

# Install utilities needed for data conversion
RUN apt-get update && apt-get install -y --no-install-recommends dos2unix gawk libssl3 && rm -rf /var/lib/apt/lists/*

# Copy the compiled binaries from the builder stage
COPY --from=builder /app/rust-server/target/release/xau-scalper-server /usr/local/bin/xau-scalper-server
COPY --from=builder /app/rust-server/target/release/backtest /usr/local/bin/backtest

# Expose the port the app runs on (as seen in main.rs)
EXPOSE 3000

# Copy the CSV data into the final image
COPY m1_data.csv /app/m1_data.csv
COPY m5_data.csv /app/m5_data.csv

# Convert the tab-delimited data to comma-separated for the backtester
# This awk script does two things:
# 1. NR==1: For the first record (the header), it prints the correct CSV headers.
# 2. NR>1: For all other records, it prints the required columns.
# We now process both M1 and M5 files.
RUN dos2unix /app/m1_data.csv && \
    dos2unix /app/m5_data.csv && \
    (head -n 1 /app/m1_data.csv | tr '\t' ',' && tail -n +2 /app/m1_data.csv | tr '\t' ',') > /app/m1_data_comma.csv && \
    (head -n 1 /app/m5_data.csv | tr '\t' ',' && tail -n +2 /app/m5_data.csv | tr '\t' ',') > /app/m5_data_comma.csv

# Set the default container command to run the server
CMD ["xau-scalper-server"]