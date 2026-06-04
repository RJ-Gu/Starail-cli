#!/usr/bin/env bash
set -euo pipefail

: "${HOME:?HOME is required}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PREFIX="${PREFIX:-$HOME/.local}"
BIN_DIR="$PREFIX/bin"
TARGET="$BIN_DIR/starail"

if ! command -v cargo >/dev/null 2>&1; then
  printf 'install.sh: cargo was not found. Install Rust first: https://rustup.rs\n' >&2
  exit 1
fi

cargo build --release --manifest-path "$SCRIPT_DIR/Cargo.toml"
mkdir -p "$BIN_DIR"
install -m 0755 "$SCRIPT_DIR/target/release/starail" "$TARGET"

printf 'Installed starail to %s\n' "$TARGET"
printf 'Run it with: starail\n'
if [[ ":$PATH:" != *":$BIN_DIR:"* ]]; then
  printf 'Tip: add %s to PATH if your shell cannot find starail.\n' "$BIN_DIR"
fi
