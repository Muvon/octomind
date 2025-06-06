#!/bin/bash
# Script to format all Rust code in the project

echo "Running cargo fmt on all Rust files..."
cargo fmt --all

echo "Checking formatting with cargo fmt --check..."
if cargo fmt --all -- --check; then
    echo "✅ All files are properly formatted"
else
    echo "❌ Some files need formatting. Run 'cargo fmt --all' to fix."
    exit 1
fi
