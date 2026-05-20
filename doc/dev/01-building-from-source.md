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

```bash
# Check compilation (fast)
cargo check

# Run clippy (linting)
cargo clippy -- -D warnings

# Format code
cargo fmt

# Run tests
cargo test

# Run specific test
cargo test test_name
```

## Pre-commit Hooks

Install pre-commit hooks to enforce quality before committing:

```bash
# Install pre-commit (if not installed)
pip install pre-commit

# Install hooks
pre-commit install
```

### Checks Performed

| Check | Description |
|-------|-------------|
| `cargo fmt` | Rust formatting |
| `cargo clippy` | Linting (warnings as errors) |
| `cargo check` | Compilation |
| `check-merge-conflict` | No merge conflict markers |
| `check-toml` | Valid TOML files |
| `check-yaml` | Valid YAML files |
| `check-added-large-files` | No files > 1MB |
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

## Release Build Optimizations

The release profile in `Cargo.toml`:
- LTO enabled (link-time optimization)
- Single codegen unit
- `opt-level = "z"` (optimize for size)
- `panic = "abort"` (smaller binary)
- Symbol stripping

## Cross-Platform Notes

- **Linux**: Landlock sandbox support (kernel 5.13+)
- **macOS**: Seatbelt sandbox support
- **Windows**: `%LOCALAPPDATA%` for data directory
