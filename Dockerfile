# Copyright 2026 Muvon Un Limited
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#     http://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.

# Single-stage Dockerfile for octomind — downloads pre-built static binary
# from GitHub Releases instead of compiling from source.
# Build with: docker build --build-arg OCTOMIND_VERSION=0.30.0 .
FROM debian:bookworm-slim

ARG OCTOMIND_VERSION
ARG TARGETARCH

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
		ca-certificates \
		curl \
		wget \
		unzip \
		ripgrep \
		&& rm -rf /var/lib/apt/lists/* \
		&& update-ca-certificates

# Map Docker TARGETARCH to the release asset target triple
# amd64 → x86_64-unknown-linux-musl
# arm64 → aarch64-unknown-linux-musl
RUN set -eu; \
		case "${TARGETARCH}" in \
			amd64)  ASSET_TARGET="x86_64-unknown-linux-musl" ;; \
			arm64)  ASSET_TARGET="aarch64-unknown-linux-musl" ;; \
			*) echo "unsupported arch ${TARGETARCH}"; exit 1 ;; \
		esac; \
		ASSET="octomind-${OCTOMIND_VERSION}-${ASSET_TARGET}.tar.gz"; \
		URL="https://github.com/muvon/octomind/releases/download/${OCTOMIND_VERSION}/${ASSET}"; \
		echo "Downloading ${URL}"; \
		curl -fsSL "${URL}" -o /tmp/octomind.tar.gz; \
		tar xzf /tmp/octomind.tar.gz -C /tmp; \
		mv /tmp/octomind /usr/local/bin/octomind; \
		chmod +x /usr/local/bin/octomind; \
		rm /tmp/octomind.tar.gz

# Install ast-grep (sg) from GitHub Releases
# Release assets use the scheme: app-{target}.zip (gnu only — works on glibc base image)
ARG AST_GREP_VERSION=0.38.6
RUN set -eu; \
		case "${TARGETARCH}" in \
			amd64)  SG_TARGET="x86_64-unknown-linux-gnu" ;; \
			arm64)  SG_TARGET="aarch64-unknown-linux-gnu" ;; \
			*) echo "unsupported arch ${TARGETARCH}"; exit 1 ;; \
		esac; \
		URL="https://github.com/ast-grep/ast-grep/releases/download/${AST_GREP_VERSION}/app-${SG_TARGET}.zip"; \
		echo "Downloading ${URL}"; \
		curl -fsSL "${URL}" -o /tmp/ast-grep.zip; \
		unzip -q /tmp/ast-grep.zip sg -d /tmp/sg-bin; \
		mv /tmp/sg-bin/sg /usr/local/bin/sg; \
		chmod +x /usr/local/bin/sg; \
		rm -rf /tmp/ast-grep.zip /tmp/sg-bin

# Install octocode via install script
ENV OCTOCODE_INSTALL_DIR=/usr/local/bin
RUN curl -fsSL https://raw.githubusercontent.com/Muvon/octocode/master/install.sh | sh

# Create a non-root user
RUN groupadd -r octomind && useradd -r -g octomind octomind

# Create app directory
WORKDIR /app

# Change ownership to non-root user
RUN chown -R octomind:octomind /app

# Switch to non-root user
USER octomind

# Health check
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
		CMD octomind --help || exit 1

# Set the entrypoint
ENTRYPOINT ["octomind"]
CMD ["--help"]
