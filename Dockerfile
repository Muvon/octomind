# Multi-stage Dockerfile for building static Octodev binary
# This creates a minimal runtime image with just the static binary

# Build stage
FROM rust:1.75 as builder

WORKDIR /usr/src/app

# Install cross-compilation dependencies
RUN apt-get update && apt-get install -y \
		musl-tools \
		&& rm -rf /var/lib/apt/lists/*

# Add musl target for static linking
RUN rustup target add x86_64-unknown-linux-musl

# Copy manifests
COPY Cargo.toml Cargo.lock ./

# Copy source code
COPY src ./src

# Build for musl target to create static binary
ENV RUSTFLAGS="-C target-feature=+crt-static"
RUN cargo build --release --target x86_64-unknown-linux-musl

# Runtime stage - use scratch for minimal image
FROM scratch

# Copy the static binary from builder stage
COPY --from=builder /usr/src/app/target/x86_64-unknown-linux-musl/release/octodev /octodev

# Expose any ports if needed (uncomment if your app serves HTTP)
# EXPOSE 8080

# Set the binary as entrypoint
ENTRYPOINT ["/octodev"]
