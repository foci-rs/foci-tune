#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: $0 <artifacts-dir> <out-dir>" >&2
  exit 2
fi

artifacts="$1"
out="$2"

targets=(
  "aarch64-unknown-linux-musl:aarch64"
  "armv7-unknown-linux-musleabihf:armv7"
  "x86_64-unknown-linux-musl:x86_64"
)

for entry in "${targets[@]}"; do
  src="${artifacts}/foci-tune-${entry%%:*}/foci-tune"
  if [ ! -f "$src" ]; then
    echo "missing artifact: $src" >&2
    exit 1
  fi
done

if command -v sha256sum >/dev/null 2>&1; then
  digest() { sha256sum "$1" | cut -d' ' -f1; }
else
  digest() { shasum -a 256 "$1" | cut -d' ' -f1; }
fi

mkdir -p "$out"
for entry in "${targets[@]}"; do
  cp "${artifacts}/foci-tune-${entry%%:*}/foci-tune" "${out}/foci-tune-${entry##*:}"
done

sums="${out}/SHA256SUMS"
: > "$sums"
for name in foci-tune-aarch64 foci-tune-armv7 foci-tune-x86_64; do
  printf '%s  %s\n' "$(digest "${out}/${name}")" "$name" >> "$sums"
done
