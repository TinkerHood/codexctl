# Codex Controller - Docker Image
# Multi-stage build for minimal size

# Stage 1: Build
FROM rust:1.98.1-slim-bookworm AS builder

WORKDIR /usr/src/codexctl

# Install build dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Copy manifests
COPY Cargo.toml Cargo.lock ./

# Copy source code
COPY src ./src

# Build release binary
RUN cargo build --release --locked \
    && strip target/release/codexctl

# Stage 2: Runtime
FROM debian:bookworm-slim

LABEL maintainer="Bhanu Korthiwada"
LABEL description="Codex Controller for Codex CLI profile management"

# Install runtime dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN useradd --create-home --shell /usr/sbin/nologin codexctl

# Copy binary from builder
COPY --from=builder --chmod=755 /usr/src/codexctl/target/release/codexctl /usr/local/bin/codexctl
RUN ln -s /usr/local/bin/codexctl /usr/local/bin/cdx

# Create profiles directory
RUN mkdir -p /home/codexctl/.local/share/codexctl && \
    chown -R codexctl:codexctl /home/codexctl

# Switch to non-root user
USER codexctl
WORKDIR /home/codexctl

# Set environment
ENV CODEXCTL_DIR=/home/codexctl/.local/share/codexctl
ENV RUST_LOG=info

# Volume for persistent profiles
VOLUME ["/home/codexctl/.local/share/codexctl"]

# Default command
ENTRYPOINT ["codexctl"]
CMD ["--help"]
