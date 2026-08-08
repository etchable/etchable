#!/usr/bin/env bash
# Vendors the Zener stdlib (lib/std) from diodeinc/pcb, pinned to the same
# tag as the pcb-* crates in Cargo.toml. zen-build discovers it by walking
# up from the executable: <repo>/lib/std serves target/debug binaries.
set -euo pipefail

TAG="v0.4.25"
REPO="https://github.com/diodeinc/pcb.git"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

if [ -f "$ROOT/lib/std/pcb.toml" ]; then
  echo "lib/std already present; delete it to re-fetch."
  exit 0
fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

git clone --depth 1 --branch "$TAG" "$REPO" "$TMP/pcb"
mkdir -p "$ROOT/lib"
cp -R "$TMP/pcb/lib/std" "$ROOT/lib/std"
cp "$TMP/pcb/LICENSE" "$ROOT/lib/std/LICENSE.diodeinc"
echo "vendored lib/std @ $TAG"
