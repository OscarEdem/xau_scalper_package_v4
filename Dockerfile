# Stage 1: Build the application in a full Rust environment
FROM rust:1-slim-bookworm AS builder

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
RUN apt-get update && apt-get install -y --no-install-recommends dos2unix gawk && rm -rf /var/lib/apt/lists/*

# Copy the compiled binaries from the builder stage
COPY --from=builder /app/rust-server/target/release/xau-scalper-server /usr/local/bin/xau-scalper-server
COPY --from=builder /app/rust-server/target/release/backtest /usr/local/bin/backtest

# Expose the port the app runs on (as seen in main.rs)
EXPOSE 3000

# Copy the CSV data into the final image
COPY rust-server/xauusd_m1_data.csv /app/xauusd_m1_data.csv

# Convert the tab-delimited data to comma-separated for the backtester
# This awk script does two things:
# 1. NR==1: For the first record (the header), it prints the correct CSV headers.
# 2. NR>1: For all other records, it combines the first two fields (date and time)
#    into a single 'time' field and prints the required columns.
RUN dos2unix /app/xauusd_m1_data.csv && \
    awk -F'\t' 'BEGIN {OFS=","} NR==1 {print "time,open,high,low,close,volume"} NR>1 {print $1" "$2, $3, $4, $5, $6, $7}' /app/xauusd_m1_data.csv > /app/data.csv

# Set the default container command to run the server
CMD ["xau-scalper-server"]