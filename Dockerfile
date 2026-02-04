# Multi-stage Dockerfile for moltis
# Builds a minimal debian-based image with the moltis gateway

# Build stage
FROM rust:bookworm AS builder

WORKDIR /build

# Copy manifests first for better caching
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

# Build release binary
RUN cargo build --release

# Runtime stage
FROM debian:bookworm-slim

# Install runtime dependencies (CA certificates for HTTPS)
RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN useradd --create-home --user-group moltis

# Copy binary from builder
COPY --from=builder /build/target/release/moltis /usr/local/bin/moltis

# Create config and data directories
RUN mkdir -p /home/moltis/.moltis && \
    chown -R moltis:moltis /home/moltis/.moltis

USER moltis
WORKDIR /home/moltis

# Expose gateway port
EXPOSE 13131

# Run the gateway on the specified port
ENTRYPOINT ["moltis"]
CMD ["--port", "13131"]
