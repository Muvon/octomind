# Octomind Cross-Platform Build System
# Copyright 2025 Muvon Un Limited
# Licensed under the Apache License, Version 2.0
# This Makefile builds static binaries for multiple platforms

# Configuration
BINARY_NAME := octomind
VERSION := $(shell grep '^version' Cargo.toml | head -n1 | cut -d'"' -f2)
BUILD_DIR := target/cross
DIST_DIR := dist

# Build flags for static linking
export RUSTFLAGS := -C target-feature=+crt-static

# Cross-compilation targets
TARGETS := \
	x86_64-unknown-linux-gnu \
	x86_64-unknown-linux-musl \
	aarch64-unknown-linux-gnu \
	aarch64-unknown-linux-musl \
	x86_64-pc-windows-gnu \
	x86_64-apple-darwin \
	aarch64-apple-darwin

# Linux targets (require cross)
LINUX_TARGETS := \
	x86_64-unknown-linux-gnu \
	x86_64-unknown-linux-musl \
	aarch64-unknown-linux-gnu \
	aarch64-unknown-linux-musl

# Windows targets
WINDOWS_TARGETS := \
	x86_64-pc-windows-gnu

# macOS targets (native only)
MACOS_TARGETS := \
	x86_64-apple-darwin \
	aarch64-apple-darwin

# Determine host OS for conditional compilation
UNAME_S := $(shell uname -s)
UNAME_M := $(shell uname -m)

# Default target
.PHONY: all
all: help

# Help target
.PHONY: help
help:
	@echo "Octomind Cross-Platform Build System"
	@echo "Version: $(VERSION)"
	@echo ""
	@echo "Available targets:"
	@echo "  help              - Show this help"
	@echo "  setup             - Install required tools for cross-compilation"
	@echo "  check             - Check if required tools are installed"
	@echo "  build             - Build for current platform"
	@echo "  build-all         - Build for all supported platforms"
	@echo "  build-linux       - Build for Linux platforms"
	@echo "  build-windows     - Build for Windows platforms"
	@echo "  build-macos       - Build for macOS platforms (macOS host only)"
	@echo "  clean             - Clean build artifacts"
	@echo "  dist              - Create distribution archives"
	@echo "  install           - Install binary to /usr/local/bin"
	@echo "  install-completions - Install shell completions"
	@echo "  test              - Run tests"
	@echo "  test-completions  - Test shell completion generation"
	@echo "  fmt               - Format code"
	@echo "  fmt-check         - Check code formatting without modifying"
	@echo "  clippy            - Run clippy lints"
	@echo "  pre-commit        - Run pre-commit hooks on all files"
	@echo "  pre-commit-install - Install pre-commit hooks"
	@echo ""
	@echo "Individual platform targets:"
	@echo "  $(TARGETS)" | tr ' ' '\n' | sed 's/^/  /'
	@echo ""
	@echo "Environment variables:"
	@echo "  CROSS_CONTAINER_ENGINE - Container engine for cross (docker/podman)"
	@echo "  RELEASE               - Build in release mode (default: true)"
	@echo "  STATIC                - Build with static linking (default: true)"

# Setup tools for cross-compilation
.PHONY: setup
setup:
	@echo "Setting up cross-compilation tools..."
	@echo "Installing Rust targets..."
	$(foreach target,$(TARGETS),rustup target add $(target) || true;)
	@echo "Installing cross..."
	cargo install cross --git https://github.com/cross-rs/cross || true
	@echo "Installing cargo-audit for security checks..."
	cargo install cargo-audit || true
	@echo "Installing cargo-outdated for dependency checks..."
	cargo install cargo-outdated || true
ifeq ($(UNAME_S),Darwin)
	@echo "macOS detected. Make sure you have Xcode command line tools installed:"
	@echo "  xcode-select --install"
endif
ifeq ($(UNAME_S),Linux)
	@echo "Linux detected. Installing additional dependencies..."
	@echo "For Ubuntu/Debian, run:"
	@echo "  sudo apt-get install gcc-mingw-w64 gcc-aarch64-linux-gnu musl-tools"
	@echo "For Alpine/musl, run:"
	@echo "  apk add musl-dev gcc"
endif
	@echo "Setup complete!"

# Check if required tools are available
.PHONY: check
check:
	@echo "Checking build environment..."
	@command -v rustc >/dev/null 2>&1 || { echo "Error: rustc not found. Install Rust first."; exit 1; }
	@command -v cargo >/dev/null 2>&1 || { echo "Error: cargo not found. Install Rust first."; exit 1; }
	@command -v cross >/dev/null 2>&1 || { echo "Warning: cross not found. Run 'make setup' or install manually."; }
	@echo "Rust version: $$(rustc --version)"
	@echo "Cargo version: $$(cargo --version)"
	@echo "Available targets:"
	@rustup target list --installed | grep -E "(linux|windows|darwin)" || echo "  No cross-compilation targets installed"
	@echo "Environment check complete!"

# Build for current platform
.PHONY: build
build:
	@echo "Building for current platform..."
	cargo build --release
	@echo "Build complete: target/release/$(BINARY_NAME)"

# Build for all platforms
.PHONY: build-all
build-all: build-linux build-windows build-macos
	@echo "All builds complete!"

# Build for Linux platforms
.PHONY: build-linux
build-linux: $(LINUX_TARGETS)
	@echo "Linux builds complete!"

# Build for Windows platforms
.PHONY: build-windows
build-windows: $(WINDOWS_TARGETS)
	@echo "Windows builds complete!"

# Build for macOS platforms (only on macOS)
.PHONY: build-macos
build-macos:
ifeq ($(UNAME_S),Darwin)
	$(MAKE) $(MACOS_TARGETS)
	@echo "macOS builds complete!"
else
	@echo "Warning: macOS builds can only be created on macOS hosts"
	@echo "Skipping macOS targets: $(MACOS_TARGETS)"
endif

# Individual target builds using cross for Linux/Windows
$(LINUX_TARGETS) $(WINDOWS_TARGETS):
	@echo "Building for $@..."
	@mkdir -p $(BUILD_DIR)/$@
	cross build --release --target $@
	@if [ "$@" = "x86_64-pc-windows-gnu" ]; then \
		cp target/$@/release/$(BINARY_NAME).exe $(BUILD_DIR)/$@/; \
	else \
		cp target/$@/release/$(BINARY_NAME) $(BUILD_DIR)/$@/; \
	fi
	@echo "✓ Build complete for $@"

# macOS targets (native compilation only)
$(MACOS_TARGETS):
ifeq ($(UNAME_S),Darwin)
	@echo "Building for $@..."
	@mkdir -p $(BUILD_DIR)/$@
	cargo build --release --target $@
	cp target/$@/release/$(BINARY_NAME) $(BUILD_DIR)/$@/
	@echo "✓ Build complete for $@"
else
	@echo "Skipping $@ (requires macOS host)"
endif

# Create distribution archives
.PHONY: dist
dist: build-all
	@echo "Creating distribution archives..."
	@mkdir -p $(DIST_DIR)
	@rm -f $(DIST_DIR)/*
	$(foreach target,$(TARGETS), \
		if [ -d "$(BUILD_DIR)/$(target)" ]; then \
			echo "Creating archive for $(target)..."; \
			if echo "$(target)" | grep -q windows; then \
				cd $(BUILD_DIR)/$(target) && zip -r ../../$(DIST_DIR)/$(BINARY_NAME)-$(VERSION)-$(target).zip . && cd -; \
			else \
				cd $(BUILD_DIR)/$(target) && tar -czf ../../$(DIST_DIR)/$(BINARY_NAME)-$(VERSION)-$(target).tar.gz . && cd -; \
			fi; \
		fi; \
	)
	@echo "Distribution archives created in $(DIST_DIR)/"
	@ls -la $(DIST_DIR)/

# Install binary to system
.PHONY: install
install: build
	@echo "Installing $(BINARY_NAME) to /usr/local/bin..."
	sudo cp target/release/$(BINARY_NAME) /usr/local/bin/
	sudo chmod +x /usr/local/bin/$(BINARY_NAME)
	@echo "Installation complete!"
	@echo "Run '$(BINARY_NAME) --help' to verify installation"

# Install shell completions
.PHONY: install-completions
install-completions: build
	@echo "Installing shell completions..."
	@./scripts/install-completions.sh
	@echo "Shell completions installed!"

# Test shell completion generation
.PHONY: test-completions
test-completions: build
	@echo "Testing shell completion generation..."
	@./scripts/test-completions.sh

# Clean build artifacts
.PHONY: clean
clean:
	@echo "Cleaning build artifacts..."
	cargo clean
	rm -rf $(BUILD_DIR)
	rm -rf $(DIST_DIR)
	@echo "Clean complete!"

# Run tests
.PHONY: test
test:
	@echo "Running tests..."
	cargo test --release

# Format code
.PHONY: fmt
fmt:
	@echo "Formatting code..."
	cargo fmt --all

# Check formatting without modifying files
.PHONY: fmt-check
fmt-check:
	@echo "Checking code formatting..."
	cargo fmt --all -- --check

# Run clippy lints
.PHONY: clippy
clippy:
	@echo "Running clippy..."
	cargo clippy --all-targets --all-features -- -D warnings

# Run pre-commit hooks on all files
.PHONY: pre-commit
pre-commit:
	@echo "Running pre-commit hooks..."
	pre-commit run --all-files

# Install pre-commit hooks
.PHONY: pre-commit-install
pre-commit-install:
	@echo "Installing pre-commit hooks..."
	pre-commit install

# Security audit
.PHONY: audit
audit:
	@echo "Running security audit..."
	cargo audit

# Check for outdated dependencies
.PHONY: outdated
outdated:
	@echo "Checking for outdated dependencies..."
	cargo outdated

# Development workflow
.PHONY: dev
dev: fmt clippy test
	@echo "Development checks complete!"

# Development workflow with pre-commit
.PHONY: dev-full
dev-full: pre-commit test
	@echo "Full development checks complete!"

# CI workflow
.PHONY: ci
ci: fmt clippy test audit
	@echo "CI checks complete!"

# Quick build for development
.PHONY: quick
quick:
	@echo "Quick development build..."
	cargo build

# Show build information
.PHONY: info
info:
	@echo "Build Information:"
	@echo "  Binary: $(BINARY_NAME)"
	@echo "  Version: $(VERSION)"
	@echo "  Host: $(UNAME_S) $(UNAME_M)"
	@echo "  Targets: $(TARGETS)"
	@echo "  Build dir: $(BUILD_DIR)"
	@echo "  Dist dir: $(DIST_DIR)"
	@echo "  Rust flags: $(RUSTFLAGS)"

# Create Cross.toml for cross-compilation configuration
.PHONY: cross-config
cross-config:
	@echo "Creating Cross.toml configuration..."
	@echo "# Cross-compilation configuration for octomind" > Cross.toml
	@echo "" >> Cross.toml
	@echo "[build]" >> Cross.toml
	@echo "# Use newer images with better toolchains" >> Cross.toml
	@echo "[target.x86_64-unknown-linux-gnu]" >> Cross.toml
	@echo "image = \"ghcr.io/cross-rs/x86_64-unknown-linux-gnu:main\"" >> Cross.toml
	@echo "" >> Cross.toml
	@echo "[target.x86_64-unknown-linux-musl]" >> Cross.toml
	@echo "image = \"ghcr.io/cross-rs/x86_64-unknown-linux-musl:main\"" >> Cross.toml
	@echo "" >> Cross.toml
	@echo "[target.aarch64-unknown-linux-gnu]" >> Cross.toml
	@echo "image = \"ghcr.io/cross-rs/aarch64-unknown-linux-gnu:main\"" >> Cross.toml
	@echo "" >> Cross.toml
	@echo "[target.aarch64-unknown-linux-musl]" >> Cross.toml
	@echo "image = \"ghcr.io/cross-rs/aarch64-unknown-linux-musl:main\"" >> Cross.toml
	@echo "" >> Cross.toml
	@echo "[target.x86_64-pc-windows-gnu]" >> Cross.toml
	@echo "image = \"ghcr.io/cross-rs/x86_64-pc-windows-gnu:main\"" >> Cross.toml
	@echo "" >> Cross.toml
	@echo "# Environment variables for all targets" >> Cross.toml
	@echo "[build.env]" >> Cross.toml
	@echo "passthrough = [" >> Cross.toml
	@echo "    \"RUSTFLAGS\"," >> Cross.toml
	@echo "]" >> Cross.toml
	@echo "Cross.toml created!"

# Show available make targets
.PHONY: targets
targets:
	@echo "Available make targets:"
	@$(MAKE) -pRrq -f $(lastword $(MAKEFILE_LIST)) : 2>/dev/null | awk -v RS= -F: '/^# File/,/^# Finished Make data base/ {if ($$1 !~ "^[#.]") {print $$1}}' | sort | grep -E -v -e '^[^[:alnum:]]' -e '^$@$$'

# Special targets - declare as phony for proper make behavior
.PHONY: $(TARGETS) $(LINUX_TARGETS) $(WINDOWS_TARGETS) $(MACOS_TARGETS)
