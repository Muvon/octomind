#!/bin/bash
# Build script for Octodev development
# This script provides common build operations for development workflow

set -e

# Configuration
BINARY_NAME="octodev"
BUILD_MODE="release"
VERBOSE=""

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Helper functions
log_info() {
		echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
		echo -e "${GREEN}[SUCCESS]${NC} $1"
}

log_warning() {
		echo -e "${YELLOW}[WARNING]${NC} $1"
}

log_error() {
		echo -e "${RED}[ERROR]${NC} $1"
}

# Show help
show_help() {
		echo "Octodev Build Script"
		echo ""
		echo "Usage: $0 [command] [options]"
		echo ""
		echo "Commands:"
		echo "  build         Build the project (default)"
		echo "  test          Run tests"
		echo "  check         Run cargo check"
		echo "  fmt           Format code"
		echo "  clippy        Run clippy lints"
		echo "  clean         Clean build artifacts"
		echo "  run           Build and run the binary"
		echo "  watch         Watch for changes and rebuild"
		echo "  size          Show binary size information"
		echo "  deps          Show dependency tree"
		echo "  audit         Run security audit"
		echo "  all           Run fmt, clippy, test, and build"
		echo ""
		echo "Options:"
		echo "  --debug       Build in debug mode"
		echo "  --verbose     Verbose output"
		echo "  --help        Show this help"
		echo ""
		echo "Examples:"
		echo "  $0 build              # Build in release mode"
		echo "  $0 build --debug      # Build in debug mode"
		echo "  $0 test --verbose     # Run tests with verbose output"
		echo "  $0 run -- --help      # Build and run with --help flag"
}

# Parse arguments
parse_args() {
		while [[ $# -gt 0 ]]; do
				case $1 in
						--debug)
								BUILD_MODE="debug"
								shift
								;;
						--verbose)
								VERBOSE="--verbose"
								shift
								;;
						--help)
								show_help
								exit 0
								;;
						--)
								shift
								RUN_ARGS="$*"
								break
								;;
						*)
								if [ -z "$COMMAND" ]; then
										COMMAND="$1"
								else
										log_error "Unknown option: $1"
										exit 1
								fi
								shift
								;;
				esac
		done

		# Default command
		if [ -z "$COMMAND" ]; then
				COMMAND="build"
		fi
}

# Get cargo flags based on build mode
get_cargo_flags() {
		if [ "$BUILD_MODE" = "release" ]; then
				echo "--release"
		else
				echo ""
		fi
}

# Get binary path based on build mode
get_binary_path() {
		if [ "$BUILD_MODE" = "release" ]; then
				echo "target/release/$BINARY_NAME"
		else
				echo "target/debug/$BINARY_NAME"
		fi
}

# Build the project
cmd_build() {
		log_info "Building $BINARY_NAME in $BUILD_MODE mode..."
		cargo build $(get_cargo_flags) $VERBOSE

		local binary_path=$(get_binary_path)
		if [ -f "$binary_path" ]; then
				log_success "Build complete: $binary_path"

				# Show binary size
				local size=$(du -h "$binary_path" | cut -f1)
				log_info "Binary size: $size"
		else
				log_error "Build failed: binary not found"
				exit 1
		fi
}

# Run tests
cmd_test() {
		log_info "Running tests..."
		cargo test $(get_cargo_flags) $VERBOSE
		log_success "Tests passed!"
}

# Run cargo check
cmd_check() {
		log_info "Running cargo check..."
		cargo check $VERBOSE
		log_success "Check passed!"
}

# Format code
cmd_fmt() {
		log_info "Formatting code..."
		cargo fmt --all
		log_success "Code formatted!"
}

# Run clippy
cmd_clippy() {
		log_info "Running clippy..."
		cargo clippy --all-targets --all-features $VERBOSE -- -D warnings
		log_success "Clippy passed!"
}

# Clean build artifacts
cmd_clean() {
		log_info "Cleaning build artifacts..."
		cargo clean $VERBOSE
		log_success "Clean complete!"
}

# Build and run
cmd_run() {
		cmd_build
		local binary_path=$(get_binary_path)
		log_info "Running $binary_path $RUN_ARGS"
		"$binary_path" $RUN_ARGS
}

# Watch for changes
cmd_watch() {
		if ! command -v cargo-watch >/dev/null 2>&1; then
				log_warning "cargo-watch not found. Installing..."
				cargo install cargo-watch
		fi

		log_info "Watching for changes..."
		cargo watch -x "build $(get_cargo_flags)"
}

# Show binary size information
cmd_size() {
		cmd_build
		local binary_path=$(get_binary_path)

		log_info "Binary size information:"
		echo ""
		echo "File size:"
		ls -lh "$binary_path"
		echo ""

		if command -v bloaty >/dev/null 2>&1; then
				echo "Detailed size breakdown (bloaty):"
				bloaty "$binary_path"
		elif command -v size >/dev/null 2>&1; then
				echo "Size breakdown:"
				size "$binary_path"
		else
				log_warning "Install 'bloaty' or 'binutils' for detailed size analysis"
		fi
}

# Show dependency tree
cmd_deps() {
		log_info "Dependency tree:"
		cargo tree
}

# Run security audit
cmd_audit() {
		if ! command -v cargo-audit >/dev/null 2>&1; then
				log_warning "cargo-audit not found. Installing..."
				cargo install cargo-audit
		fi

		log_info "Running security audit..."
		cargo audit
		log_success "Security audit passed!"
}

# Run all checks
cmd_all() {
		log_info "Running complete development workflow..."
		cmd_fmt
		cmd_clippy
		cmd_test
		cmd_build
		log_success "All checks passed!"
}

# Main execution
main() {
		parse_args "$@"

		# Check if we're in a Rust project
		if [ ! -f "Cargo.toml" ]; then
				log_error "Not a Rust project (Cargo.toml not found)"
				exit 1
		fi

		# Execute command
		case "$COMMAND" in
				build)
						cmd_build
						;;
				test)
						cmd_test
						;;
				check)
						cmd_check
						;;
				fmt)
						cmd_fmt
						;;
				clippy)
						cmd_clippy
						;;
				clean)
						cmd_clean
						;;
				run)
						cmd_run
						;;
				watch)
						cmd_watch
						;;
				size)
						cmd_size
						;;
				deps)
						cmd_deps
						;;
				audit)
						cmd_audit
						;;
				all)
						cmd_all
						;;
				help)
						show_help
						;;
				*)
						log_error "Unknown command: $COMMAND"
						echo "Run '$0 --help' for usage information"
						exit 1
						;;
		esac
}

# Run main function
main "$@"
