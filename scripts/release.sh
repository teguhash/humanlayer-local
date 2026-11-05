#!/usr/bin/env bash
# Build a release and the files to attach to a GitHub Release:
#   - HumanLayer-Local_<version>_<arch>.dmg          (for people installing)
#   - HumanLayer-Local_<version>_<arch>.app.tar.gz   (what the updater downloads)
#   - latest.json                                    (the update feed)
#
# Needs the updater signing key (made with `npx tauri signer generate`).
# It is read from $TAURI_SIGNING_PRIVATE_KEY_FILE (default
# ~/.tauri/humanlayer-local.key) and never written into the repo.
#
# Usage: scripts/release.sh [owner/repo]   (default teguhash/humanlayer-local)
# Then: create a GitHub Release tagged v<version> and attach everything in
# the printed output folder. The app's feed URL is
# https://github.com/<owner/repo>/releases/latest/download/latest.json
set -euo pipefail

repo="${1:-teguhash/humanlayer-local}"
root="$(cd "$(dirname "$0")/.." && pwd)"
key_file="${TAURI_SIGNING_PRIVATE_KEY_FILE:-$HOME/.tauri/humanlayer-local.key}"
[ -f "$key_file" ] || { echo "Signing key not found: $key_file" >&2; exit 1; }

version="$(node -p "require('$root/src-tauri/tauri.conf.json').version")"
arch="$(uname -m)"   # arm64 or x86_64
case "$arch" in
  arm64) platform="darwin-aarch64"; tarch="aarch64" ;;
  x86_64) platform="darwin-x86_64"; tarch="x64" ;;
  *) echo "Unsupported arch $arch" >&2; exit 1 ;;
esac

source "$HOME/.cargo/env" 2>/dev/null || true
cd "$root"
TAURI_SIGNING_PRIVATE_KEY="$(cat "$key_file")" \
TAURI_SIGNING_PRIVATE_KEY_PASSWORD="${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}" \
  npm run tauri build

bundle="$root/src-tauri/target/release/bundle"
out="$root/src-tauri/target/release/github-release"
rm -rf "$out" && mkdir -p "$out"

name="HumanLayer-Local_${version}_${tarch}"
cp "$bundle/macos/HumanLayer Local.app.tar.gz" "$out/$name.app.tar.gz"
cp "$bundle/macos/HumanLayer Local.app.tar.gz.sig" "$out/$name.app.tar.gz.sig"
cp "$bundle/dmg/"*.dmg "$out/$name.dmg"

sig="$(cat "$out/$name.app.tar.gz.sig")"
url="https://github.com/$repo/releases/download/v$version/$name.app.tar.gz"
node -e '
const [version, platform, signature, url, file] = process.argv.slice(1);
const feed = {
  version,
  notes: `HumanLayer Local ${version}`,
  pub_date: new Date().toISOString(),
  platforms: { [platform]: { signature, url } },
};
require("fs").writeFileSync(file, JSON.stringify(feed, null, 2) + "\n");
' "$version" "$platform" "$sig" "$url" "$out/latest.json"

echo
echo "Release files for v$version in: $out"
ls -1 "$out"
