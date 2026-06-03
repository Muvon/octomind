# Building from Source

## Prerequisites

- **Rust** 1.95+ ([rustup.rs](https://rustup.rs))
- **Git**
- **C compiler** (for native dependencies)
  - Linux: `build-essential` / `gcc`
  - macOS: Xcode Command Line Tools (`xcode-select --install`)

## Build

```bash
git clone https://github.com/muvon/octomind.git
cd octomind

# Debug build
cargo build

# Release build (optimized)
cargo build --release

# Binary location
./target/release/octomind --version
```

## Development Workflow

Use the same flags the pre-commit hooks and CI use, so problems surface locally
instead of at commit time. The `--all-targets --all-features` flags make clippy
and check cover tests, examples, and feature-gated code — without them you run a
weaker check than the hooks.

```bash
# Check compilation (matches the hook / CI)
cargo check --all-targets --all-features

# Run clippy (linting; warnings treated as errors)
cargo clippy --all-targets --all-features -- -D warnings

# Format code (matches the fmt hook args)
cargo fmt --all

# Run tests
cargo test

# Run a specific test
cargo test test_name
```

### Make targets

The `Makefile` wraps these commands and adds cross-platform build helpers. The
most useful targets for building from source:

| Target | Action |
|--------|--------|
| `make build` | Release build for the current platform (`cargo build --release`) |
| `make quick` | Debug build (`cargo build`) |
| `make fmt` | Format code (`cargo fmt --all`) |
| `make fmt-check` | Check formatting without modifying files |
| `make clippy` | `cargo clippy --all-targets --all-features -- -D warnings` |
| `make test` | Run tests (`cargo test --release`) |
| `make dev` | Run `fmt`, `clippy`, then `test` in sequence |
| `make pre-commit` | Run all pre-commit hooks on all files |
| `make pre-commit-install` | Install the pre-commit Git hook |
| `make install` | Build release and copy the binary to `/usr/local/bin` (uses `sudo`) |
| `make install-completions` | Install shell completions |
| `make audit` | Run `cargo audit` (requires `cargo-audit`) |

Run `make help` to see the full list.

## Pre-commit Hooks

Pre-commit hooks enforce quality before each commit. Installation is **required**
and per-clone: the hook is not committed to the repo, so `.git/hooks/pre-commit`
does not exist until you install it. Hooks do not run out of the box after `git clone`.

```bash
# Install pre-commit (if not installed)
pip install pre-commit

# Install hooks (or: make pre-commit-install)
pre-commit install
```

### Checks Performed

| Check | Description |
|-------|-------------|
| `cargo fmt` | Rust formatting (`--all`) |
| `cargo clippy` | Linting, warnings as errors (`--all-targets --all-features -- -D warnings`) |
| `cargo check` | Compilation (`--all-targets --all-features`) |
| `check-merge-conflict` | No merge conflict markers |
| `check-toml` | Valid TOML files |
| `check-yaml` | Valid YAML files |
| `check-added-large-files` | No files > 1000 KB (~1 MB) |
| `trailing-whitespace` | No trailing whitespace |
| `end-of-file-fixer` | Files end with newline |

### Manual Execution

```bash
# Run all hooks
make pre-commit

# Or directly
pre-commit run --all-files
```

### Bypassing (discouraged)

```bash
git commit --no-verify
```

## Code Style

The hooks (and CI) enforce a few style rules. Code that violates them will fail
the commit, so set your editor up accordingly:

- **Tabs, not spaces** — `rustfmt.toml` sets `hard_tabs = true`.
- **LF line endings, UTF-8, final newline, 120-column limit** — defined in `.editorconfig`.
- **Apache 2.0 copyright header** — every new `.rs` file must begin with the
  standard Apache License header (see `CONTRIBUTING.md` for the exact block).

Running `cargo fmt --all` before committing handles formatting automatically.
See [CONTRIBUTING.md](../../CONTRIBUTING.md) for the full code-style, architecture,
and contribution guidelines.

## Release Build Optimizations

The `[profile.release]` section in `Cargo.toml`:
- LTO enabled (link-time optimization)
- Single codegen unit (`codegen-units = 1`)
- `opt-level = "z"` (optimize for size)
- `panic = "abort"` (smaller binary)
- Symbol stripping (`strip = true`)
- `overflow-checks = false` (disabled in release)

## Cross-Compilation

The `Makefile` can build static binaries for seven targets using
[`cross`](https://github.com/cross-rs/cross):

- `x86_64-unknown-linux-gnu`, `x86_64-unknown-linux-musl`
- `aarch64-unknown-linux-gnu`, `aarch64-unknown-linux-musl`
- `x86_64-pc-windows-gnu`
- `x86_64-apple-darwin`, `aarch64-apple-darwin` (built natively on macOS hosts only)

```bash
# Install Rust targets, cross, and audit tooling
make setup

# Generate Cross.toml (cross-compilation config)
make cross-config

# Build everything (Linux + Windows + macOS-on-macOS)
make build-all

# Or build a single platform group
make build-linux
make build-windows
make build-macos      # macOS host only

# Package release archives into dist/
make dist
```

Linux and Windows targets build inside `cross` containers (Docker or Podman, set
via `CROSS_CONTAINER_ENGINE`). macOS targets compile natively and require a macOS host.

## Cross-Platform Notes

- **Linux**: Landlock sandbox support (kernel 5.13+)
- **macOS**: Seatbelt sandbox support
- **Windows**: `%LOCALAPPDATA%` for data directory
