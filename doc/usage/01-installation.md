# Installation

Install Octomind and authorize the recommended OctoHub gateway, or configure your own provider credentials.

## Recommended Setup

### 1. Install the binary

```bash
curl -fsSL https://octomind.run/install.sh | bash
```

The installer detects the current OS and architecture, downloads the matching GitHub release, and installs `octomind` in `~/.local/bin` by default. If that directory is not on `PATH`, the script prints the exact export to add to your shell profile.

### 2. Sign in

```bash
octomind login
```

The command displays an approval code, opens the approval page in your browser, and waits for confirmation. If the browser cannot open, it prints the URL instead. The completed login stores the OctoHub gateway credential in Octomind's user-scope `.env`, so you do not need to create API keys or accounts with individual model providers. Free models are included.

### 3. Start Octomind

```bash
octomind
```

Running `octomind` without a subcommand starts the same interactive session as `octomind run`. The default configuration uses `octohub:auto` for its main, supervisor, and compression model purposes.

## Installer Requirements and Targets

The install script requires `curl`; Windows archive extraction also requires `unzip`. It supports these release targets:

| Platform | Target |
|----------|--------|
| Linux x86_64 | `x86_64-unknown-linux-musl` |
| Linux ARM64 | `aarch64-unknown-linux-musl` |
| macOS Intel | `x86_64-apple-darwin` |
| macOS Apple Silicon | `aarch64-apple-darwin` |
| Windows x86_64 | `x86_64-pc-windows-msvc` |
| Windows ARM64 | `aarch64-pc-windows-msvc` |

Set `OCTOMIND_INSTALL_DIR` to choose another destination:

```bash
export OCTOMIND_INSTALL_DIR=/opt/bin
curl -fsSL https://octomind.run/install.sh | bash
```

## Bring Your Own API Key

Signing in is optional. To use a provider directly, export its credential and override the model at startup:

```bash
export OPENROUTER_API_KEY="your_key"
octomind run -m 'openrouter:<model>'
```

Replace `<model>` with a model identifier accepted by that provider. To make it permanent, change `[model].name` in `config.toml`. See [AI Providers](04-providers.md#bring-your-own-keys) for every supported prefix and credential variable.

## Other Installation Methods

### GitHub release archive

Download the archive for your target from [GitHub Releases](https://github.com/muvon/octomind/releases). Release assets use this naming scheme:

```text
octomind-<version>-<target>.tar.gz
octomind-<version>-<target>.zip
```

Unix archives contain `octomind`; Windows archives contain `octomind.exe`. Extract the binary and move it to a directory on `PATH`.

### Cargo

```bash
cargo install octomind
```

This builds Octomind from source and requires Rust 1.95 or newer. See [Building from Source](../dev/01-building-from-source.md) for the repository development setup.

## Automated Installation

The installer accepts these environment variables:

| Variable | Purpose |
|----------|---------|
| `GITHUB_TOKEN` | Authenticate GitHub API requests |
| `GH_TOKEN` | Alternative GitHub token variable |
| `OCTOMIND_INSTALL_DIR` | Override the destination directory |
| `OCTOMIND_VERSION` | Install a specific release version |

Flags passed to the piped script override the corresponding environment values:

```bash
curl -fsSL https://octomind.run/install.sh | \
  bash -s -- --version <version> --target aarch64-apple-darwin --install-dir /opt/bin
```

## Shell Completions

Generate a completion script for a supported shell:

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

For Zsh, add `~/.zfunc` to `fpath` and initialize completion in your shell configuration:

```zsh
fpath=(~/.zfunc $fpath)
autoload -Uz compinit && compinit
```

## Verify the Installation

```bash
octomind --version
octomind config --show
```

Continue with the [Quickstart](02-quickstart.md), or inspect the generated settings in [Configuration](03-configuration.md).
