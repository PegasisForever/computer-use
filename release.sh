#!/usr/bin/env bash
set -euo pipefail

VERSION=$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')
TAG="v${VERSION}"
BINARY="computer-use"
ASSET="computer-use-linux-x86_64"

echo "Building release ${TAG}..."
cargo +nightly build --release

cp "target/release/${BINARY}" "${ASSET}"
strip "${ASSET}"

echo "Creating GitHub release ${TAG}..."
gh release create "${TAG}" \
  --title "${TAG}" \
  --notes "Release ${VERSION}" \
  "${ASSET}"

rm "${ASSET}"
echo "Done: https://github.com/PegasisForever/computer-use/releases/tag/${TAG}"
