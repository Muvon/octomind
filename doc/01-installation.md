# Installation Guide

This guide covers all installation methods for Octomind, from quick setup to development builds.

## Prerequisites

### For Users
- **API Key** from supported AI provider (OpenRouter, OpenAI, Anthropic, etc.)
- **Operating System**: Linux, macOS, or Windows

### For Developers
- **Rust 1.82+** and Cargo
- **Git** for version control
- **API Key** from supported AI provider

## Quick Installation (Recommended)

The fastest way to get started with Octomind:

```bash
# One-line install
curl -fsSL https://raw.githubusercontent.com/muvon/octomind/master/install.sh | bash

# Or download and inspect first
curl -fsSL https://raw.githubusercontent.com/muvon/octomind/master/install.sh -o install.sh
chmod +x install.sh
./install.sh
```

This script automatically:
- Detects your platform (Linux, macOS, Windows)
- Downloads the appropriate binary
- Installs to `/usr/local/bin` (or equivalent)
- Sets up shell completions (optional)

## Manual Installation

### Download Pre-built Binaries

Download from [GitHub Releases](https://github.com/muvon/octomind/releases):

**Linux:**
```bash
# x86_64 GNU libc
wget https://github.com/muvon/octomind/releases/latest/download/octomind-x86_64-unknown-linux-gnu.tar.gz
tar -xzf octomind-x86_64-unknown-linux-gnu.tar.gz
sudo mv octomind /usr/local/bin/

# x86_64 musl (static)
wget https://github.com/muvon/octomind/releases/latest/download/octomind-x86_64-unknown-linux-musl.tar.gz
```

**macOS:**
```bash
# Intel Macs
wget https://github.com/muvon/octomind/releases/latest/download/octomind-x86_64-apple-darwin.tar.gz

# Apple Silicon Macs
wget https://github.com/muvon/octomind/releases/latest/download/octomind-aarch64-apple-darwin.tar.gz
```

**Windows:**
```bash
# Download and extract
wget https://github.com/muvon/octomind/releases/latest/download/octomind-x86_64-pc-windows-gnu.zip
```

## Package Managers

### Cargo (Rust Package Manager)
```bash
# Install from crates.io (when published)
cargo install octomind

# Install from Git repository
cargo install --git https://github.com/muvon/octomind.git
```

### Homebrew (Coming Soon)
```bash
# Future release
brew install muvon/tap/octomind
```
## Development Setup

### Building from Source

For developers who want to build Octomind from source:

```bash
# Clone the repository
git clone https://github.com/muvon/octomind.git
cd octomind

# Fast compilation check (recommended for development)
cargo check --message-format=short

# Fix code quality issues (treat warnings as errors)
cargo clippy --all-features --all-targets -- -D warnings

# Build debug version (when you need the binary)
cargo build

# Build release version (for production)
cargo build --release
```

### Development Workflow

**Daily Development Cycle:**
```bash
# 1. Fast syntax/compilation check (PREFERRED)
cargo check --message-format=short

# 2. Fix all code quality issues
cargo clippy --all-features --all-targets -- -D warnings

# 3. Build only when needed
cargo build

# 4. Test your changes
./target/debug/octomind --version
```

**Important Development Rules:**
- **ALWAYS** use `cargo check --message-format=short` for fast validation
- **NEVER** use `cargo build --release` during development (extremely slow)
- **ALWAYS** fix clippy warnings (treat as errors)
- **NEVER** modify system-wide configs or run tests that affect global configuration

### Cross-Platform Building

Octomind supports multiple platforms. For cross-compilation:

```bash
# Install cross-compilation targets
rustup target add x86_64-unknown-linux-musl
rustup target add aarch64-apple-darwin
rustup target add x86_64-pc-windows-gnu

# Build for specific targets
cargo build --release --target x86_64-unknown-linux-musl
cargo build --release --target aarch64-apple-darwin
cargo build --release --target x86_64-pc-windows-gnu
```

## API Key Setup

After installation, you need to configure an AI provider API key:

### Supported Providers

Set one or more API keys for the providers you want to use:

```bash
# Multi-provider access (recommended)
export OPENROUTER_API_KEY="sk-or-v1-..."

# Direct provider access
export OPENAI_API_KEY="sk-..."
export ANTHROPIC_API_KEY="sk-ant-..."
export GOOGLE_API_KEY="AIza..."
export AMAZON_ACCESS_KEY_ID="AKIA..."
export AMAZON_SECRET_ACCESS_KEY="..."
export CLOUDFLARE_API_TOKEN="..."
export DEEPSEEK_API_KEY="sk-..."

# Optional: Web search capability
export BRAVE_API_KEY="BSA..."
```

### Provider Details

| Provider | API Key Format | Features |
|----------|----------------|----------|
| **OpenRouter** | `sk-or-v1-...` | Multi-provider access, caching, vision models |
| **OpenAI** | `sk-...` | Direct API, GPT-4o vision, cost calculation |
| **Anthropic** | `sk-ant-...` | Claude models, caching, Claude 3+ vision |
| **Google** | `AIza...` | Vertex AI, Gemini 1.5+ vision support |
| **Amazon** | Access Key + Secret | Bedrock models, AWS integration |
| **Cloudflare** | Token | Edge AI, fast inference, Llama 3.2 vision |
| **DeepSeek** | `sk-...` | Cost-effective models, competitive performance |

### Persistent Configuration

Add API keys to your shell profile for persistence:

```bash
# Add to ~/.bashrc, ~/.zshrc, or ~/.profile
echo 'export OPENROUTER_API_KEY="your_key_here"' >> ~/.bashrc
source ~/.bashrc
```

## Shell Completions

Octomind provides built-in shell completion support for all major shells:

### Initial Setup

**Bash:**
```bash
# User installation
mkdir -p ~/.bash_completion.d
octomind completion bash > ~/.bash_completion.d/octomind
echo 'source ~/.bash_completion.d/octomind' >> ~/.bashrc

# System-wide installation
sudo octomind completion bash > /etc/bash_completion.d/octomind
```

**Zsh:**
```bash
# User installation
mkdir -p ~/.zsh/completions
octomind completion zsh > ~/.zsh/completions/_octomind
echo 'fpath=(~/.zsh/completions $fpath)' >> ~/.zshrc
echo 'autoload -Uz compinit && compinit' >> ~/.zshrc

# System-wide installation
sudo octomind completion zsh > /usr/local/share/zsh/site-functions/_octomind
```

**Fish:**
```bash
# User installation
mkdir -p ~/.config/fish/completions
octomind completion fish > ~/.config/fish/completions/octomind.fish

# System-wide installation
sudo octomind completion fish > /usr/share/fish/vendor_completions.d/octomind.fish
```

**PowerShell (Windows):**
```powershell
# User installation
octomind completion powershell | Out-File -FilePath $PROFILE -Append
```

### Updating Completions

After updating Octomind, regenerate completions to get new commands:

```bash
# Bash
octomind completion bash > ~/.bash_completion.d/octomind
source ~/.bash_completion.d/octomind

# Zsh
octomind completion zsh > ~/.zsh/completions/_octomind
rm -f ~/.zcompdump  # Clear completion cache
exec zsh            # Reload shell

# Fish
octomind completion fish > ~/.config/fish/completions/octomind.fish
# Fish auto-reloads completions
```

**Supported Shells:**
- ✅ Bash
- ✅ Zsh
- ✅ Fish
- ✅ PowerShell (Windows)
- ✅ Elvish (via generic completion)

## Verification

After installation, verify Octomind is working correctly:

```bash
# Check version
octomind --version

# Verify API key is set
octomind vars

# Test basic functionality
octomind config --show

# Start a test session
octomind run assistant
```

### First Run

```bash
# Generate default configuration (optional)
octomind config

# Start your first session
octomind run

# Within the session, try:
/help                    # Show available commands
/info                    # Check token usage and costs
/mcp info               # Check MCP server status
```

## Configuration

Octomind uses a template-based configuration system with smart defaults:

### Configuration Files

```
~/.config/octomind/config.toml    # Main user configuration
~/.config/octomind/*.toml         # Additional config files (merged)
~/.local/share/octomind/sessions/ # Session history
~/.local/share/octomind/logs/     # Debug logs
```

**Multi-File Configuration:** Octomind supports loading multiple `.toml` files from the config directory. Files are loaded in alphabetical order and merged together, with later files overriding earlier ones. This allows you to:
- Split configuration into logical files (e.g., `roles.toml`, `layers.toml`)
- Override settings without modifying the main `config.toml`
- Share common configurations across projects

### Environment Variables

Any configuration setting can be overridden with environment variables:

```bash
# System-wide settings
export OCTOMIND_LOG_LEVEL="debug"
export OCTOMIND_MODEL="openrouter:anthropic/claude-sonnet-4"
export OCTOMIND_MAX_TOKENS="16384"

# Role-specific overrides (use double underscores for nested settings)
export OCTOMIND_ROLES__DEVELOPER__MODEL="openai:gpt-4o"
export OCTOMIND_ROLES__DEVELOPER__TEMPERATURE="0.1"
```

### Custom Instructions and Constraints

Octomind supports automatic loading of custom instructions and constraints:

- **`custom_instructions_file_name`** (default: `"INSTRUCTIONS.md"`): Content loaded as a user message in new sessions
- **`custom_constraints_file_name`** (default: `"CONSTRAINTS.md"`): Content appended to each user request in `<constraints>...</constraints>` tags

Set to empty string to disable: `custom_constraints_file_name = ""`

### Context Compression

Octomind includes intelligent context compression to manage long sessions. When enabled, the system automatically compresses session history when context pressure exceeds configured thresholds:

```toml
[compression]
hints_enabled = true                    # Show compression hints
hints_pressure_threshold = 0.7          # Context pressure threshold for hints
adaptive_threshold = true               # Enable token-based compression

[[compression.pressure_levels]]
threshold = 50000
target_ratio = 2.0  # Compress to 50% at 50k tokens

[[compression.pressure_levels]]
threshold = 100000
target_ratio = 4.0  # Compress to 25% at 100k tokens

[[compression.pressure_levels]]
threshold = 150000
target_ratio = 8.0  # Compress to 12.5% at 150k tokens

[compression.decision]
model = "anthropic:claude-haiku-4-5"    # Model for compression decisions
max_tokens = 16000
```

Compression preserves architectural information, file references, and key technical details while reducing token usage. See [Advanced Configuration](./06-advanced.md) for details.

## Troubleshooting

### Common Installation Issues

**1. Binary Not Found**
```bash
# Check if binary is in PATH
which octomind

# Add to PATH if needed
export PATH="$PATH:/usr/local/bin"
```

**2. Permission Denied**
```bash
# Make binary executable
chmod +x /usr/local/bin/octomind

# Or install to user directory
mkdir -p ~/.local/bin
mv octomind ~/.local/bin/
export PATH="$PATH:~/.local/bin"
```

**3. API Key Issues**
```bash
# Verify API key is set
echo $OPENROUTER_API_KEY

```bash
# Test API key validity
octomind run developer "Hello" --model "openrouter:anthropic/claude-haiku"
```

### Development Issues

**1. Rust/Cargo Problems**
```bash
# Update Rust toolchain
rustup update

# Check Rust version (need 1.82+)
rustc --version

# Clean build cache
cargo clean
```

**2. Build Failures**
```bash
# Fast compilation check
cargo check --message-format=short

# Fix code quality issues
cargo clippy --all-features --all-targets -- -D warnings

# Check for missing dependencies
cargo tree
```

**3. Configuration Issues**
```bash
# Validate configuration
octomind config --validate

# Reset to defaults
rm ~/.config/octomind/config.toml
octomind config

# Check environment variables
octomind vars
```

### Platform-Specific Issues

**Linux:**
- Install `build-essential` for compilation
- Use musl target for static binaries
- Check glibc version compatibility

**macOS:**
- Install Xcode command line tools: `xcode-select --install`
- Use appropriate target for your architecture (Intel vs Apple Silicon)

**Windows:**
- Install Visual Studio Build Tools
- Use WSL for better compatibility
- Consider using the Linux binary in WSL

### Getting Help

If you encounter issues not covered here:

1. **Check Logs**: Use `/loglevel debug` in sessions for detailed logging
2. **GitHub Issues**: [Report bugs](https://github.com/muvon/octomind/issues)
3. **Discussions**: [Community support](https://github.com/muvon/octomind/discussions)
4. **Documentation**: Review other guides in this manual

---

**Next Steps**: After installation, see the [Overview](./02-overview.md) for core concepts and [Sessions Guide](./05-sessions.md) for usage instructions.
