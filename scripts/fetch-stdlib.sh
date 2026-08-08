#!/usr/bin/env bash
# Vendors the Zener stdlib (lib/std) and the upstream agent language skill
# from diodeinc/pcb, pinned to the same tag as the pcb-* crates in
# Cargo.toml. zen-build discovers the stdlib by walking up from the
# executable: <repo>/lib/std serves target/debug binaries. The skill is
# compiled into the MCP server (zener_reference tool), so it is committed.
set -euo pipefail

TAG="v0.4.25"
REPO="https://github.com/diodeinc/pcb.git"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SKILL="$ROOT/crates/mcp/assets/zener-language-skill.md"

have_stdlib=0
[ -f "$ROOT/lib/std/pcb.toml" ] && have_stdlib=1
have_skill=0
[ -s "$SKILL" ] && have_skill=1

if [ "$have_stdlib" = 1 ] && [ "$have_skill" = 1 ]; then
  echo "lib/std and the zener skill are already present; delete them to re-fetch."
  exit 0
fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

git clone --depth 1 --branch "$TAG" "$REPO" "$TMP/pcb"

if [ "$have_stdlib" = 0 ]; then
  mkdir -p "$ROOT/lib"
  cp -R "$TMP/pcb/lib/std" "$ROOT/lib/std"
  cp "$TMP/pcb/LICENSE" "$ROOT/lib/std/LICENSE.diodeinc"
  echo "vendored lib/std @ $TAG"
fi

if [ "$have_skill" = 0 ]; then
  mkdir -p "$(dirname "$SKILL")"
  cp "$TMP/pcb/skills/zener-language/SKILL.md" "$SKILL"
  echo "vendored zener-language skill @ $TAG"
fi
