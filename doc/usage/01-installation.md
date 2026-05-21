# Installation

## Prerequisites

- A terminal (macOS Terminal, Linux shell, Windows PowerShell)
- An API key from at least one supported provider (see [Providers](04-providers.md))

## Quick Installation

```bash
curl -fsSL https://raw.githubusercontent.com/muvon/octomind/master/install.sh | bash
```

This detects your OS and architecture, downloads the latest release, and installs to `/usr/local/bin/`.

## Manual Installation

Download the appropriate binary from [GitHub Releases](https://github.com/muvon/octomind/releases):

| Platform | Binary |
|----------|--------|
| Linux x86_64 | `octomind-linux-amd64` |
| Linux ARM64 | `octomind-linux-arm64` |
| macOS Intel | `octomind-darwin-amd64` |
| macOS Apple Silicon | `octomind-darwin-arm64` |
| Windows x86_64 | `octomind-windows-amd64.exe` |

```bash
# Example: macOS Apple Silicon
chmod +x octomind-darwin-arm64
sudo mv octomind-darwin-arm64 /usr/local/bin/octomind
```

## Package Managers

### Cargo (Rust)

```bash
cargo install octomind
```

Requires Rust 1.95+. See [Building from Source](../dev/01-building-from-source.md) for development setup.

## API Key Setup

Set at least one provider API key as an environment variable:

```bash
# OpenRouter (recommended -- access to many providers)
export OPENROUTER_API_KEY="your_key"

# Or use a specific provider
export OPENAI_API_KEY="your_key"
export ANTHROPIC_API_KEY="your_key"
export DEEPSEEK_API_KEY="your_key"
```

Add to your shell profile (`~/.bashrc`, `~/.zshrc`, etc.) for persistence. Or use a `.env` file in your project directory.

See [Environment Variables](../reference/04-environment-variables.md) for all supported variables.

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
```

For Zsh, ensure `~/.zfunc` is in your `fpath`:
```bash
echo 'fpath=(~/.zfunc $fpath)' >> ~/.zshrc
echo 'autoload -Uz compinit && compinit' >> ~/.zshrc
```

## CI/CD Installation

The install script supports environment variables for automated environments where GitHub API rate limits may apply:

| Variable | Description |
|----------|-------------|
| `GITHUB_TOKEN` or `GH_TOKEN` | Authenticate GitHub API requests to avoid rate limits |
| `OCTOMIND_INSTALL_DIR` | Override installation directory (default: `~/.local/bin/`) |
| `OCTOMIND_VERSION` | Install a specific version instead of latest |

```bash
# CI example
GITHUB_TOKEN="${{ secrets.GITHUB_TOKEN }}" \
  OCTOMIND_VERSION="0.23.1" \
  curl -fsSL https://raw.githubusercontent.com/muvon/octomind/master/install.sh | bash
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

Configuration is stored at `~/.local/share/octomind/config/config.toml`. See [Configuration](03-configuration.md) for details.
