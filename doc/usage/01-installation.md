# Installation

## Prerequisites

- A terminal (macOS Terminal, Linux shell, Windows PowerShell)
- An API key from at least one supported provider (see [Providers](04-providers.md))

## Quick Installation

```bash
curl -fsSL https://raw.githubusercontent.com/muvon/octomind/master/install.sh | bash
```

This detects your OS and architecture, downloads the latest release, and installs to `~/.local/bin/` (override with `OCTOMIND_INSTALL_DIR`). If `~/.local/bin` is not on your `PATH`, the script prints a warning and tells you to add it:

```bash
export PATH="$HOME/.local/bin:$PATH"
```

Add that line to your shell profile (`~/.bashrc`, `~/.zshrc`, `~/.profile`, etc.) so it persists.

## Manual Installation

Download the archive for your platform from [GitHub Releases](https://github.com/muvon/octomind/releases). Assets are versioned and named by Rust target triple (replace `<version>` with the release you want, e.g. `0.29.0`):

| Platform | Asset |
|----------|-------|
| Linux x86_64 | `octomind-<version>-x86_64-unknown-linux-musl.tar.gz` |
| Linux ARM64 | `octomind-<version>-aarch64-unknown-linux-musl.tar.gz` |
| macOS Intel | `octomind-<version>-x86_64-apple-darwin.tar.gz` |
| macOS Apple Silicon | `octomind-<version>-aarch64-apple-darwin.tar.gz` |
| Windows x86_64 | `octomind-<version>-x86_64-pc-windows-msvc.zip` |
| Windows ARM64 | `octomind-<version>-aarch64-pc-windows-msvc.zip` |

The `.tar.gz`/`.zip` archive contains a single `octomind` binary (`octomind.exe` on Windows). Extract it, then move it onto your `PATH`:

```bash
# Example: macOS Apple Silicon
tar xzf octomind-0.29.0-aarch64-apple-darwin.tar.gz
chmod +x octomind
mv octomind ~/.local/bin/octomind        # or: sudo mv octomind /usr/local/bin/octomind
```

```powershell
# Example: Windows x86_64 (PowerShell)
Expand-Archive octomind-0.29.0-x86_64-pc-windows-msvc.zip -DestinationPath .
# Then move octomind.exe to a directory on your PATH
```

## Package Managers

### Cargo (Rust)

```bash
cargo install octomind
```

This builds from source and requires the Rust toolchain (Rust 1.95+). It is the path for Rust users; the recommended install for everyone else is the install script or the GitHub release archives above. See [Building from Source](../dev/01-building-from-source.md) for development setup.

## API Key Setup

API keys are read **only** from environment variables (or a `.env` file in your project directory). There is no config field for them — `octomind config --api-key provider:key` is intentionally rejected and points you to the corresponding env var instead.

Set at least one provider API key. Common providers:

```bash
# OpenRouter (recommended -- one key, access to many models)
export OPENROUTER_API_KEY="your_key"

# Or use a specific provider
export OPENAI_API_KEY="your_key"
export ANTHROPIC_API_KEY="your_key"
export DEEPSEEK_API_KEY="your_key"
```

Some providers use differently named credentials rather than `<PROVIDER>_API_KEY`:

```bash
# Google Vertex AI -- path to a service-account JSON file
export GOOGLE_APPLICATION_CREDENTIALS="/path/to/credentials.json"

# Amazon Bedrock
export AWS_BEARER_TOKEN_BEDROCK="your_token"

# Cloudflare Workers AI
export CLOUDFLARE_API_TOKEN="your_token"
```

Add the relevant exports to your shell profile (`~/.bashrc`, `~/.zshrc`, etc.) for persistence, or put them in a `.env` file in your project directory.

See [Environment Variables](../reference/04-environment-variables.md) for the complete list of supported providers and variables.

## Shell Completions

Generate completions for your shell:

```bash
# Bash
octomind completion bash > ~/.local/share/bash-completion/completions/octomind

# Zsh
octomind completion zsh > ~/.zfunc/_octomind

# Fish
octomind completion fish > ~/.config/fish/completions/octomind.fish

# PowerShell
octomind completion powershell > octomind.ps1

# Elvish
octomind completion elvish > ~/.config/elvish/lib/octomind.elv
```

For Zsh, ensure `~/.zfunc` is in your `fpath`:
```bash
echo 'fpath=(~/.zfunc $fpath)' >> ~/.zshrc
echo 'autoload -Uz compinit && compinit' >> ~/.zshrc
```

> Note: `bash`, `zsh`, and `fish` completions include dynamic agent/role TAG completion for `octomind run` (driven by `octomind complete run` at runtime). `powershell` and `elvish` completions are static — they complete subcommands and flags but not agent/role tags.

## CI/CD Installation

The install script supports environment variables for automated environments where GitHub API rate limits may apply:

| Variable | Description |
|----------|-------------|
| `GITHUB_TOKEN` or `GH_TOKEN` | Authenticate GitHub API requests to avoid rate limits |
| `OCTOMIND_INSTALL_DIR` | Override installation directory (default: `~/.local/bin/`) |
| `OCTOMIND_VERSION` | Install a specific version instead of latest |

`GH_TOKEN` is accepted as an alternative to `GITHUB_TOKEN`.

```bash
# CI example
GITHUB_TOKEN="${{ secrets.GITHUB_TOKEN }}" \
  OCTOMIND_VERSION="0.29.0" \
  curl -fsSL https://raw.githubusercontent.com/muvon/octomind/master/install.sh | bash
```

The script also accepts flags when piped, which override the environment variables (`--target` is useful for cross-platform installs):

```bash
curl -fsSL https://raw.githubusercontent.com/muvon/octomind/master/install.sh | \
  bash -s -- --version 0.29.0 --target aarch64-apple-darwin --install-dir /opt/bin
```

## Verification

```bash
# Check installation
octomind --version

# Generate default config
octomind config

# Start your first session
octomind run
```

Configuration is stored at `~/.local/share/octomind/config/config.toml` on macOS and Linux (on Windows: `%LOCALAPPDATA%\octomind\config\config.toml`). See [Configuration](03-configuration.md) for details.
