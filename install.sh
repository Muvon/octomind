#!/bin/bash
# Octodev Installation Script
# This script downloads and installs the appropriate binary for your platform

set -e

# Configuration
REPO="muvon/octodev"
BINARY_NAME="octodev"
INSTALL_DIR="/usr/local/bin"

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

# Detect platform
detect_platform() {
		local os arch

		# Detect OS
		case "$(uname -s)" in
				Linux*)     os="linux" ;;
				Darwin*)    os="darwin" ;;
				CYGWIN*|MINGW*|MSYS*) os="windows" ;;
				*)          log_error "Unsupported operating system: $(uname -s)"; exit 1 ;;
		esac

		# Detect architecture
		case "$(uname -m)" in
				x86_64|amd64)   arch="x86_64" ;;
				aarch64|arm64)  arch="aarch64" ;;
				*)              log_error "Unsupported architecture: $(uname -m)"; exit 1 ;;
		esac

		# Determine target triple
		if [ "$os" = "linux" ]; then
				if command -v ldd >/dev/null 2>&1 && ldd --version 2>&1 | grep -q musl; then
						echo "${arch}-unknown-linux-musl"
				else
						echo "${arch}-unknown-linux-gnu"
				fi
		elif [ "$os" = "darwin" ]; then
				echo "${arch}-apple-darwin"
		elif [ "$os" = "windows" ]; then
				echo "${arch}-pc-windows-gnu"
		fi
}

# Get latest release version from GitHub
get_latest_version() {
		local version
		version=$(curl -s "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name"' | cut -d'"' -f4)
		if [ -z "$version" ]; then
				log_error "Failed to get latest version from GitHub API"
				exit 1
		fi
		echo "$version"
}

# Download and install binary
install_binary() {
		local target="$1"
		local version="$2"
		local archive_name filename download_url

		# Determine archive format and filename
		if [[ "$target" == *"windows"* ]]; then
				archive_name="${BINARY_NAME}-${version}-${target}.zip"
				filename="${BINARY_NAME}.exe"
		else
				archive_name="${BINARY_NAME}-${version}-${target}.tar.gz"
				filename="${BINARY_NAME}"
		fi

		download_url="https://github.com/${REPO}/releases/download/${version}/${archive_name}"

		log_info "Downloading ${archive_name}..."

		# Create temporary directory
		local temp_dir
		temp_dir=$(mktemp -d)
		cd "$temp_dir"

		# Download archive
		if command -v curl >/dev/null 2>&1; then
				curl -L -o "$archive_name" "$download_url"
		elif command -v wget >/dev/null 2>&1; then
				wget -O "$archive_name" "$download_url"
		else
				log_error "Neither curl nor wget is available"
				exit 1
		fi

		# Extract archive
		log_info "Extracting archive..."
		if [[ "$archive_name" == *.zip ]]; then
				if command -v unzip >/dev/null 2>&1; then
						unzip -q "$archive_name"
				else
						log_error "unzip is not available"
						exit 1
				fi
		else
				tar -xzf "$archive_name"
		fi

		# Install binary
		log_info "Installing binary to ${INSTALL_DIR}..."

		if [ -w "$INSTALL_DIR" ]; then
				cp "$filename" "${INSTALL_DIR}/${BINARY_NAME}"
				chmod +x "${INSTALL_DIR}/${BINARY_NAME}"
		else
				sudo cp "$filename" "${INSTALL_DIR}/${BINARY_NAME}"
				sudo chmod +x "${INSTALL_DIR}/${BINARY_NAME}"
		fi

		# Cleanup
		cd - >/dev/null
		rm -rf "$temp_dir"

		log_success "Installation complete!"
}

# Verify installation
verify_installation() {
		if command -v "$BINARY_NAME" >/dev/null 2>&1; then
				local installed_version
				installed_version=$("$BINARY_NAME" --version 2>/dev/null | head -n1 || echo "unknown")
				log_success "Octodev is installed: $installed_version"
				log_info "Run 'octodev --help' to get started"
				return 0
		else
				log_error "Installation failed - binary not found in PATH"
				return 1
		fi
}

# Main installation flow
main() {
		echo "🐙 Octodev Installation Script"
		echo "=============================="

		# Check if already installed
		if command -v "$BINARY_NAME" >/dev/null 2>&1; then
				local current_version
				current_version=$("$BINARY_NAME" --version 2>/dev/null | head -n1 || echo "unknown")
				log_warning "Octodev is already installed: $current_version"
				read -p "Do you want to update it? (y/N): " -r
				if [[ ! $REPLY =~ ^[Yy]$ ]]; then
						log_info "Installation cancelled"
						exit 0
				fi
		fi

		# Detect platform
		local target
		target=$(detect_platform)
		log_info "Detected platform: $target"

		# Get latest version
		local version
		version=$(get_latest_version)
		log_info "Latest version: $version"

		# Install binary
		install_binary "$target" "$version"

		# Verify installation
		verify_installation

		echo ""
		log_success "🎉 Octodev has been successfully installed!"
		echo ""
		echo "Next steps:"
		echo "  1. Index your codebase: octodev index"
		echo "  2. Search your code: octodev search 'your query'"
		echo "  3. Start an AI session: octodev session"
		echo ""
		echo "For more information, visit: https://github.com/${REPO}"
}

# Parse command line arguments
case "${1:-}" in
		--help|-h)
				echo "Octodev Installation Script"
				echo ""
				echo "Usage: $0 [options]"
				echo ""
				echo "Options:"
				echo "  --help, -h     Show this help message"
				echo "  --version, -v  Install specific version"
				echo ""
				echo "Environment variables:"
				echo "  INSTALL_DIR    Installation directory (default: /usr/local/bin)"
				echo ""
				exit 0
				;;
		--version|-v)
				if [ -z "${2:-}" ]; then
						log_error "Version not specified"
						exit 1
				fi
				VERSION="$2"
				;;
esac

# Run main function
main "$@"
