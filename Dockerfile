# Multi-stage Dockerfile for octomind
# Stage 1: Build
FROM rust:1.95-slim AS builder

# Install system dependencies
RUN apt-get update && apt-get install -y \
		pkg-config \
		libssl-dev \
		curl \
		g++ \
		&& rm -rf /var/lib/apt/lists/*

# Create app directory
WORKDIR /app

# Copy manifests
COPY Cargo.toml Cargo.lock ./

# Copy source code and config templates
COPY src ./src
COPY config-templates ./config-templates
COPY assets ./assets

# Build the application
RUN cargo build --release

# Install additional tools in builder stage
RUN cargo install ripgrep --locked --root /usr/local && \
    cargo install ast-grep --locked --root /usr/local

# Install octocode to a specific directory
ENV OCTOCODE_INSTALL_DIR=/usr/local/bin
RUN curl -fsSL https://raw.githubusercontent.com/Muvon/octocode/master/install.sh | sh

# Stage 2: Runtime
FROM debian:bookworm-slim

# Install runtime dependencies only
RUN apt-get update && apt-get install -y \
		ca-certificates \
		curl \
		wget \
		&& rm -rf /var/lib/apt/lists/* \
		&& update-ca-certificates

# Create a non-root user
RUN groupadd -r octomind && useradd -r -g octomind octomind

# Create app directory
WORKDIR /app

# Copy the binary from builder stage
COPY --from=builder /app/target/release/octomind /usr/local/bin/octomind

# Copy additional tools from builder stage
COPY --from=builder /usr/local/bin/rg /usr/local/bin/rg
COPY --from=builder /usr/local/bin/sg /usr/local/bin/sg
COPY --from=builder /usr/local/bin/octocode /usr/local/bin/octocode

# Change ownership to non-root user
RUN chown -R octomind:octomind /app

# Switch to non-root user
USER octomind

# Expose port (if applicable)
# EXPOSE 8080

# Health check (customize based on your application)
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
		CMD octomind --help || exit 1

# Set the entrypoint
ENTRYPOINT ["octomind"]
CMD ["--help"]
