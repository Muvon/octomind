# Installation

Install Octomind for terminal use, sign in to the default OctoHub gateway, and set up shell completions.

## Get Started

### 1. Install the binary

```bash
curl -fsSL https://octomind.run/install.sh | bash
```

The installer detects the current OS and architecture, downloads the matching GitHub release, and installs `octomind` in
`~/.local/bin` by default. If that directory is not on `PATH`, the script prints the exact export to add to your shell
profile.

### 2. Sign in

```bash
octomind login
```

The command displays an approval code, opens the approval page in your browser, and waits for confirmation. If the
browser cannot open, it prints the URL instead. The completed login stores `OCTOHUB_API_KEY` in
`<data-dir>/config/.env`, so you do not need separate credentials for models accessed through that gateway.

### 3. Start Octomind

```bash
octomind
```

Running `octomind` without a subcommand starts the same interactive session as `octomind run`. The default configuration
uses `octohub:auto` for its main, supervisor, and compression model purposes.

## Installer Requirements and Targets

The install script requires a Unix-style shell and `curl`, plus `tar` for Unix archives or `unzip` for Windows archives.
On Windows, run it from Git Bash or MSYS2. The script recognizes these targets; the chosen release must contain the
matching asset:

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
export OCTOMIND_INSTALL_DIR="$HOME/bin"
curl -fsSL https://octomind.run/install.sh | bash
```

## Bring Your Own API Key

Signing in is optional. See [AI Providers](04-providers.md#bring-your-own-keys) to configure direct-provider credentials
and the main, supervisor, and compression models.

## Other Installation Methods

### GitHub release archive

Download the archive for your target from [GitHub Releases](https://github.com/muvon/octomind/releases). Release assets
use this naming scheme:

```text
octomind-<version>-<target>.tar.gz
octomind-<version>-<target>.zip
```

Unix archives contain `octomind`; Windows archives contain `octomind.exe`. Extract the binary and move it to a directory
on `PATH`.

For a Unix archive, set `OCTOMIND_ARCHIVE` to the downloaded file's path, then run:

```bash
tar -xzf "$OCTOMIND_ARCHIVE" octomind
mkdir -p ~/.local/bin
install -m 755 octomind ~/.local/bin/octomind
```

For a Windows ZIP in Git Bash or MSYS2:

```bash
unzip "$OCTOMIND_ARCHIVE" octomind.exe
mkdir -p ~/.local/bin
cp octomind.exe ~/.local/bin/octomind.exe
```

### Cargo

```bash
cargo install octomind
```

This builds Octomind from source and requires Rust 1.95 or newer. See [Building from
Source](../dev/01-building-from-source.md) for the repository development setup.

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
  bash -s -- --target aarch64-apple-darwin --install-dir "$HOME/.local/bin"
```

To pin a release, set `OCTOMIND_VERSION` to its exact release tag and run:

```bash
curl -fsSL https://octomind.run/install.sh | bash -s -- --version "$OCTOMIND_VERSION"
```

## Shell Completions

Generate a completion script for a supported shell:

```bash
# Bash
mkdir -p ~/.local/share/bash-completion/completions
octomind completion bash > ~/.local/share/bash-completion/completions/octomind

# Zsh
mkdir -p ~/.zfunc
octomind completion zsh > ~/.zfunc/_octomind

# Fish
mkdir -p ~/.config/fish/completions
octomind completion fish > ~/.config/fish/completions/octomind.fish

# PowerShell
octomind completion powershell > octomind.ps1

# Elvish
mkdir -p ~/.config/elvish/lib
octomind completion elvish > ~/.config/elvish/lib/octomind.elv
```

For Zsh, add `~/.zfunc` to `fpath` and initialize completion in your shell configuration:

```bash
fpath=(~/.zfunc $fpath)
autoload -Uz compinit && compinit
```

## Verify the Installation

```bash
octomind --version
octomind config --show
```

## Troubleshooting

If your shell cannot find the default installation, add it to the current shell's path:

```bash
export PATH="$HOME/.local/bin:$PATH"
octomind --version
```

On a machine without a browser, print the approval URL explicitly. To replace an existing login, use `--force`:

```bash
octomind login --no-browser
octomind login --force --no-browser
```

## See also

- [Quickstart](02-quickstart.md)
- [Configuration](03-configuration.md)
- [AI Providers](04-providers.md)
