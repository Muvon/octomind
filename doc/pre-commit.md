# Pre-commit Setup for Octomind

This project uses pre-commit hooks to ensure code quality and consistency.

## Installation

Pre-commit hooks are automatically installed when you run:

```bash
make pre-commit-install
```

Or manually:

```bash
pre-commit install
```

## What Gets Checked

The pre-commit hooks run the following checks on every commit:

### Rust-specific checks:
- **cargo fmt** - Formats Rust code according to rustfmt.toml
- **cargo clippy** - Runs linting with clippy (treats warnings as errors)
- **cargo check** - Ensures code compiles successfully

### General checks:
- **trailing-whitespace** - Removes trailing whitespace
- **end-of-file-fixer** - Ensures files end with newline
- **check-merge-conflict** - Prevents committing merge conflict markers
- **check-yaml** - Validates YAML syntax
- **check-toml** - Validates TOML syntax
- **check-added-large-files** - Prevents large files (>1MB) from being committed

## Manual Execution

You can run the hooks manually:

```bash
# Run all pre-commit hooks
make pre-commit

# Run only formatting
make fmt

# Check formatting without modifying files
make fmt-check

# Run only clippy
make clippy

# Run formatting script
./scripts/format.sh
```

## Development Workflow

```bash
# Standard development checks
make dev

# Full development checks including pre-commit
make dev-full
```

## Configuration

- Pre-commit configuration: `.pre-commit-config.yaml`
- Rust formatting rules: `rustfmt.toml`
- The hooks will automatically install on first run

## Bypassing Hooks (Not Recommended)

If you absolutely need to bypass hooks for a commit:

```bash
git commit --no-verify -m "commit message"
```

However, this is strongly discouraged as it bypasses code quality checks.
