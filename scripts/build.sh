#!/usr/bin/env bash
# Build the rustproxy release binary and run the quality gates.
set -euo pipefail

cd "$(dirname "$0")/.."

echo "==> cargo fmt --check"
cargo fmt --all -- --check

echo "==> cargo clippy"
cargo clippy --workspace --all-targets --all-features -- -D warnings

echo "==> cargo test"
cargo test --workspace

echo "==> cargo build --release"
cargo build --release -p rustproxy

echo "==> done"
echo "Binary: $(pwd)/target/release/rustproxy"
