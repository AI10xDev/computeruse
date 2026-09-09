#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
cargo_home="${CARGO_HOME:-$HOME/.cargo}"

cargo build --manifest-path "$project_dir/Cargo.toml" --release --bin computeruse
install -Dm755 "$project_dir/target/release/computeruse" "$cargo_home/bin/computeruse"

echo "Installed computeruse to $cargo_home/bin/computeruse"
