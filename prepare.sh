#!/usr/bin/env bash
# Build the prop-amm CLI from the vendored source and ensure `cargo-build-sbf`
# is installed (required by `prop-amm validate`, which the eval gates on).
#
# - `cargo-build-sbf` is published on crates.io as a single binary (~30s build,
#   no Solana CLI needed). Installed into ~/.cargo/bin via `cargo install`.
# - The release binary lives at vendor/prop-amm-challenge/target/release/prop-amm,
#   which eval/eval.sh runs directly.
set -euo pipefail
cd "$(dirname "$0")"

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo not found; install Rust via https://rustup.rs and re-run prepare.sh" >&2
  exit 1
fi

if ! command -v cargo-build-sbf >/dev/null 2>&1; then
  echo "Installing cargo-build-sbf (required for prop-amm validate)..."
  cargo install cargo-build-sbf --version 4.0.0 --locked
fi

cargo build --release -p prop-amm \
  --manifest-path vendor/prop-amm-challenge/Cargo.toml \
  --locked
