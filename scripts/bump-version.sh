#!/usr/bin/env bash
# Bumps the app version everywhere it's pinned (package.json, tauri.conf.json,
# Cargo.toml), refreshes the Cargo lockfile, and commits.
# Usage: scripts/bump-version.sh <patch|minor|major>
set -euo pipefail

type="${1:?usage: bump-version.sh <patch|minor|major>}"
cd "$(dirname "$0")/.."

current=$(perl -ne 'print $1 and exit if /"version": "([^"]+)"/' apps/desktop/package.json)
IFS=. read -r major minor patch <<<"$current"
case "$type" in
  major) version="$((major + 1)).0.0" ;;
  minor) version="${major}.$((minor + 1)).0" ;;
  patch) version="${major}.${minor}.$((patch + 1))" ;;
  *) echo "unknown bump type: $type" >&2; exit 1 ;;
esac

perl -pi -e "s/\"version\": \"\Q$current\E\"/\"version\": \"$version\"/" \
  apps/desktop/package.json \
  apps/desktop/src-tauri/tauri.conf.json
perl -pi -e "s/^version = \"\Q$current\E\"/version = \"$version\"/" \
  apps/desktop/src-tauri/Cargo.toml

# Sync the lockfile to the new workspace version without touching dependencies.
cargo update --workspace --manifest-path apps/desktop/src-tauri/Cargo.toml

git add apps/desktop/package.json \
  apps/desktop/src-tauri/tauri.conf.json \
  apps/desktop/src-tauri/Cargo.toml \
  apps/desktop/src-tauri/Cargo.lock
git commit -m "release v$version"

echo "$version"
