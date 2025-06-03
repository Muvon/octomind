# Installation Guide

## Overview

Octomind provides multiple installation methods to suit different needs, from quick installation scripts to building from source with cross-compilation support.

## Quick Installation (Recommended)

Use our installation script to automatically download the appropriate binary for your platform:

```bash
# Install latest version
curl -fsSL https://raw.githubusercontent.com/muvon/octomind/main/install.sh | bash

# Or download and inspect first
curl -fsSL https://raw.githubusercontent.com/muvon/octomind/main/install.sh -o install.sh
chmod +x install.sh
./install.sh
```

## Manual Installation

Download pre-compiled binaries from the [releases page](https://github.com/muvon/octomind/releases) for your platform:

- **Linux**: `octomind-{version}-x86_64-unknown-linux-{gnu,musl}.tar.gz`
- **macOS**: `octomind-{version}-{x86_64,aarch64}-apple-darwin.tar.gz`
- **Windows**: `octomind-{version}-x86_64-pc-windows-gnu.zip`

Extract and place the binary in your `PATH`.

## Package Managers

### Homebrew (macOS/Linux)
```bash
# Coming soon
brew install muvon/tap/octomind
```

### Cargo (Build from source)
```bash
cargo install --git https://github.com/muvon/octomind.git
```

## Building from Source

### Prerequisites
- Rust 1.70+ and Cargo installed on your system
- No additional runtime dependencies - embedded SurrealDB is used for storage

### Basic Build
```bash
# Clone the repository
git clone https://github.com/muvon/octomind.git
cd octomind

# Quick build for development
make build

# Or build manually
cargo build --release

# Install to system
make install
```

## Shell Completions

Octomind includes built-in shell completion support for bash and zsh to improve your command-line experience.

### Generating Completions

```bash
# Generate bash completion
octomind completion bash > octomind_completion.bash

# Generate zsh completion
octomind completion zsh > _octomind

# See all available shells
octomind completion --help
```

### Installing Completions

**Automatic Installation:**
```bash
# After building the release binary, run the install script
./scripts/install-completions.sh
```

**Manual Installation:**

For **Bash**:
```bash
# Install to user completion directory
octomind completion bash > ~/.local/share/bash-completion/completions/octomind

# Or source directly in your ~/.bashrc
echo 'source <(octomind completion bash)' >> ~/.bashrc
```

For **Zsh**:
```bash
# Install to user completion directory
mkdir -p ~/.config/zsh/completions
octomind completion zsh > ~/.config/zsh/completions/_octomind

# Add to your ~/.zshrc if not already present
echo 'fpath=(~/.config/zsh/completions $fpath)' >> ~/.zshrc
echo 'autoload -U compinit && compinit' >> ~/.zshrc
```

### Completion Features

Shell completions provide:
- **Command completion**: Tab-complete `octomind` subcommands (`session`, `ask`, `config`, etc.)
- **Option completion**: Complete flags and arguments for each command
- **File completion**: Automatic file path completion where appropriate
- **Shell selection**: Complete available shells for the `completion` command

The completions are automatically generated from your CLI structure, so they stay up-to-date with any command changes.

## Cross-Compilation

Octomind includes a comprehensive cross-compilation setup for building static binaries across multiple platforms.

### Supported Platforms

- **Linux**: x86_64 and aarch64 (glibc and musl)
- **macOS**: x86_64 and Apple Silicon (aarch64)
- **Windows**: x86_64

### Build System Setup

```bash
# Install cross-compilation tools
make setup

# Check your environment
make check

# Build for all platforms
make build-all

# Build for specific platforms
make build-linux      # All Linux targets
make build-windows    # Windows targets
make build-macos      # macOS targets (macOS host only)

# Build for specific target
make x86_64-unknown-linux-musl

# Create distribution archives
make dist
```

### Individual Platform Builds

```bash
# Linux targets (using cross)
make x86_64-unknown-linux-gnu
make x86_64-unknown-linux-musl
make aarch64-unknown-linux-gnu
make aarch64-unknown-linux-musl

# Windows target (using cross)
make x86_64-pc-windows-gnu

# macOS targets (native only, requires macOS)
make x86_64-apple-darwin
make aarch64-apple-darwin
```

### Requirements for Cross-Compilation

**All Platforms:**
- Rust toolchain with cross-compilation targets
- [cross](https://github.com/cross-rs/cross) tool for Linux/Windows builds
- Docker or Podman for containerized builds

**Installation:**
```bash
# Install targets
rustup target add x86_64-unknown-linux-gnu
rustup target add x86_64-unknown-linux-musl
rustup target add aarch64-unknown-linux-gnu
rustup target add aarch64-unknown-linux-musl
rustup target add x86_64-pc-windows-gnu
rustup target add x86_64-apple-darwin      # macOS only
rustup target add aarch64-apple-darwin     # macOS only

# Install cross tool
cargo install cross --git https://github.com/cross-rs/cross
```

### Static Linking Configuration

All builds use static linking by default for maximum compatibility:

- **Linux**: Uses musl targets for fully static binaries
- **Windows**: Uses static CRT linking
- **macOS**: Uses static linking where possible

The build configuration in `Cargo.toml` enables:
- Link Time Optimization (LTO)
- Single codegen unit for better optimization
- Panic abort for smaller binaries
- Symbol stripping

### GitHub Actions CI/CD

Automated builds are configured for:
- All platforms on every push/PR
- Release artifacts on git tags
- Docker images for containerized deployment
- Automated security audits and code quality checks

See `.github/workflows/cross-build.yml` for the complete CI configuration.

## Docker Support

Build and run in containers:

```bash
# Build Docker image
docker build -t octomind .

# Run in container
docker run --rm -v $(pwd):/workspace octomind index /workspace
```

### Build Configuration Files

- **`Makefile`**: Comprehensive build system with all targets
- **`Cross.toml`**: Configuration for cross-compilation tool
- **`Dockerfile`**: Multi-stage build for minimal container image
- **`.github/workflows/cross-build.yml`**: CI/CD pipeline

## Verification

After installation, verify Octomind is working:

```bash
# Check version
octomind --version

# Test configuration
octomind config --validate

# Start a test session
octomind session --role=assistant
```

## Troubleshooting Installation

### Common Issues

#### Permission Denied
```bash
# Make binary executable
chmod +x octomind

# Or install to user directory
mkdir -p ~/.local/bin
mv octomind ~/.local/bin/
export PATH="$HOME/.local/bin:$PATH"
```

#### Missing Dependencies
```bash
# On Linux, you might need:
sudo apt-get update
sudo apt-get install ca-certificates

# On macOS with Homebrew:
brew install ca-certificates
```

#### Cross-compilation Issues
```bash
# Install Docker/Podman for cross-compilation
# Ubuntu/Debian:
sudo apt-get install docker.io

# macOS:
brew install docker

# Check cross tool installation
cross --version
```

### Build Issues

#### Rust Version
```bash
# Update Rust to latest version
rustup update

# Check version (needs 1.70+)
rustc --version
```

#### Cargo Cache Issues
```bash
# Clear cargo cache if builds fail
cargo clean
rm -rf ~/.cargo/registry/cache
```

### Platform-Specific Notes

#### Linux
- Static musl builds are recommended for maximum compatibility
- glibc builds require compatible system libraries

#### macOS
- Universal binaries support both Intel and Apple Silicon
- Code signing may be required for distribution

#### Windows
- GNU toolchain is used for better compatibility
- MSVC builds are not currently supported

## Next Steps

After installation:

1. **[Configuration Guide](./02-configuration.md)** - Set up providers and roles
2. **[Provider Setup](./03-providers.md)** - Configure AI models
3. **[Session Guide](./05-sessions.md)** - Start using interactive sessions

## Getting Help

- **Installation Issues**: [GitHub Issues](https://github.com/muvon/octomind/issues)
- **Build Problems**: Check the troubleshooting section above
- **Platform Support**: Contact [opensource@muvon.io](mailto:opensource@muvon.io)
