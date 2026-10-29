#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
export CMAKE_POLICY_VERSION_MINIMUM=3.5
command -v cargo >/dev/null || { echo 'Najprej namesti Rust: https://rustup.rs'; exit 1; }
command -v cmake >/dev/null || { echo 'Namesti CMake (macOS: brew install cmake; Debian/Ubuntu: sudo apt install cmake).'; exit 1; }
cargo build --release --workspace --locked
python3 scripts/package.py
