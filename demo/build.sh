#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
cargo run -q --release --manifest-path ../bd-build/Cargo.toml -- bd.yaml
