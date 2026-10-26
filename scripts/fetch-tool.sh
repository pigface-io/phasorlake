#!/usr/bin/env bash
# Download a release archive (.tar.gz or .zip), check it against a SHA-256
# written beside the call, and install one binary from it. CI runs what it downloads with
# the repository checked out, and what it builds is committed and shipped:
# a tool that is not the one that was reviewed must not run.
#
#   scripts/fetch-tool.sh <url> <sha256> <path-in-archive> <install-dir>
set -euo pipefail

url="$1"
sha256="$2"
member="$3"
dir="$4"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

curl -sSfL --proto '=https' --tlsv1.2 -o "$work/archive" "$url"
# The digest alone, compared by hand: macOS's sha256sum has no --quiet.
actual="$(sha256sum "$work/archive" | cut -d' ' -f1)"
if [ "$actual" != "$sha256" ]; then
    echo "fetch-tool: $url has SHA-256 $actual, not $sha256" >&2
    exit 1
fi
case "$url" in
*.zip) unzip -q "$work/archive" "$member" -d "$work" ;;
*) tar -xzf "$work/archive" -C "$work" "$member" ;;
esac
mkdir -p "$dir"
install -m 0755 "$work/$member" "$dir/$(basename "$member")"
echo "installed $(basename "$member") from $url"
