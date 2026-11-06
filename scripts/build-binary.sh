#!/usr/bin/env bash
# Builds the static bin/power-hub reproducibly and records its sha256.
#   scripts/build-binary.sh [--check]   --check rebuilds from scratch and fails if the hash differs
set -euo pipefail
cd "$(dirname "$0")/.."
mode=${1:-build}
[[ $mode == build || $mode == --check ]] || { echo "usage: $0 [--check]" >&2; exit 2; }

target=x86_64-unknown-linux-musl
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
export SOURCE_DATE_EPOCH=0 CARGO_TARGET_DIR=$tmp
export RUSTFLAGS="--remap-path-prefix=$PWD=/build --remap-path-prefix=$HOME/.cargo=/cargo --remap-path-prefix=$HOME/.rustup=/rustup -C link-arg=-Wl,--build-id=none"
cargo build --locked --release --target "$target"
built=$tmp/$target/release/power-hub

if [[ $mode == --check ]]; then
  cmp -s "$built" bin/power-hub && [[ -x bin/power-hub ]] && (cd bin && sha256sum -c --quiet power-hub.sha256) \
    || { echo "bin/power-hub is stale" >&2; exit 1; }
  echo "bin/power-hub matches source"
else
  install -m 755 "$built" bin/power-hub
  (cd bin && sha256sum power-hub > power-hub.sha256)
fi
