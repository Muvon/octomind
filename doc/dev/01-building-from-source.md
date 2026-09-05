# Building from Source

Build Octomind from source and prepare the local checks used when contributing Rust changes. This guide is for
contributors who need a development binary or platform-specific build.

## Prerequisites

- **Rust** 1.95+ ([rustup.rs](https://rustup.rs))
- **Git**
- **C/C++ toolchain** (for native dependencies)
  - Linux: `build-essential` / `gcc`
  - macOS: Xcode Command Line Tools (`xcode-select --install`)
- **Protocol Buffers compiler (`protoc`)**, **ripgrep**, and **ast-grep** for the CI test environment.

`Cargo.toml` declares Rust 1.95 as the minimum; the checked-in CI test matrix uses 1.98.0, plus beta and nightly on
Linux. Check your tools before building:

```bash
rustc --version
cargo --version
protoc --version
rg --version
ast-grep --version
```

For native test-library setup, see [Common build problems](#common-build-problems).

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

## Code Style

Configure your editor from `rustfmt.toml` and `.editorconfig`. Formatting and whitespace hooks enforce part of this
policy; the listed hooks do not validate copyright headers or every editor setting.

- **Tabs, not spaces** — `rustfmt.toml` sets `hard_tabs = true`.
- **LF line endings, UTF-8, final newline, 120-column limit** — defined in `.editorconfig`.
- **Apache 2.0 copyright header** — every new `.rs` file must begin with the
  standard Apache License header (see `CONTRIBUTING.md` for the exact block).

Running `cargo fmt --all` before committing handles formatting automatically. Keep unit test bodies in sibling
`*_tests.rs` files as required by `AGENTS.md`.

## Development Workflow

Use the flags declared in `.pre-commit-config.yaml`. The `--all-targets --all-features` flags make clippy and check
cover tests, examples, and feature-gated code — without them you run a weaker check than the hooks.

```bash
# Check compilation (matches the hook)
cargo check --all-targets --all-features

# Run clippy (linting; warnings treated as errors)
cargo clippy --all-targets --all-features -- -D warnings

# Format code (matches the fmt hook args)
cargo fmt --all

# Run tests
cargo test

# Run a specific test
cargo test directories::tests::test_config_file_path
```

### Make targets

The `Makefile` wraps these commands and adds cross-platform build helpers. It also exports `RUSTFLAGS=-C
target-feature=+crt-static`, so a Make build can differ from a direct Cargo build. The most useful targets for building
from source:

| Target | Action |
|--------|--------|
| `make build` | Release build for the current platform (`cargo build --release`) |
| `make quick` | Debug build (`cargo build`) |
| `make fmt` | Format code (`cargo fmt --all`) |
| `make fmt-check` | Check formatting without modifying files |
| `make clippy` | `cargo clippy --all-targets --all-features -- -D warnings` |
| `make test` | Run tests (`cargo test --release`) |
| `make dev` | Run `fmt`, `clippy`, and `test` prerequisites (serial unless you enable parallel Make) |
| `make pre-commit` | Run all pre-commit hooks on all files |
| `make pre-commit-install` | Install the pre-commit Git hook |
| `make install` | Build release and copy the binary to `/usr/local/bin` (uses `sudo`) |
| `make install-completions` | Install shell completions |
| `make audit` | Run `cargo audit` (requires `cargo-audit`) |

```bash
make help
make quick
make fmt-check
```

## Pre-commit Hooks

Pre-commit hooks enforce quality before each commit. Installation is **required** and per-clone: the hook is not
committed to the repo, so `.git/hooks/pre-commit` does not exist until you install it. Hooks do not run out of the box
after `git clone`.

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

## Cross-Compilation

The `Makefile` defines seven build targets and requests static CRT linking using
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

Linux and Windows targets build inside `cross` containers (Docker or Podman, set via `CROSS_CONTAINER_ENGINE`). macOS
targets compile natively and require a macOS host.

## Common build problems

### Tests fail to link ONNX Runtime

The Linux x86-64 CI job downloads ONNX Runtime 1.24.2 and points `ORT_LIB_LOCATION` at its static library. To reproduce
that setup in a Bash shell:

```bash
ort_workdir=$(mktemp -d)
ort_asset=onnxruntime-linux-x64-static_lib-1.24.2-glibc2_17
curl -fL "https://github.com/csukuangfj/onnxruntime-libs/releases/download/v1.24.2/${ort_asset}.zip" \
  -o "$ort_workdir/ort.zip"
unzip -q "$ort_workdir/ort.zip" -d "$ort_workdir"
export ORT_LIB_LOCATION="$ort_workdir/$ort_asset/lib"
cargo test --verbose
```

For Windows MSVC, CI uses dynamic CRT flags to match the downloaded ONNX libraries. Run the following from Bash with the
MSVC toolchain; these are different from the Makefile's Windows GNU target:

```bash
RUSTFLAGS='-C target-feature=-crt-static' CXXFLAGS_x86_64_pc_windows_msvc=-MD cargo test --verbose
```

Embedding smoke tests can download model weights on first use. The CI workflow caches those weights separately from
`target/`; see [ci.yml](../../.github/workflows/ci.yml) for platform cache paths and exact setup.

### Which binary or configuration am I using?

Use the explicit binary path to avoid an older installed release. `OCTOMIND_DATA_DIR` selects a separate data root,
including config, sessions, and learning records:

```bash
./target/debug/octomind --version
OCTOMIND_DATA_DIR="$PWD/target/octomind-dev-data" ./target/debug/octomind config --show
```

## Release Build Optimizations

The `[profile.release]` section in `Cargo.toml` applies to `cargo build --release` above:

- LTO enabled (link-time optimization)
- Single codegen unit (`codegen-units = 1`)
- `opt-level = "s"` (optimize for size while retaining vectorization)
- `panic = "abort"` (smaller binary)
- Symbol stripping (`strip = true`)
- `overflow-checks = false` (disabled in release)

## Platform reference

- **Linux**: Landlock sandbox support (kernel 5.13+); enforcement depends on kernel support
- **macOS**: Seatbelt sandbox support
- **Windows**: `%LOCALAPPDATA%/octomind` is the default data directory

These sandbox backends are selected when sandboxing is enabled, for example:

```bash
./target/debug/octomind run --sandbox
```

## See also

- [Architecture](02-architecture.md)
- [MCP Server Development](03-mcp-server-development.md)
- [Contributing](../../CONTRIBUTING.md)
- [Configuration Reference](../reference/03-config-reference.md)
