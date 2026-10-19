#!/usr/bin/env bash
# Points the Homebrew cask (kitlangton/homebrew-tap, Casks/scrawl.rb) at a release's DMG.
#   scripts/bump-cask.sh [--push] [VERSION]
# VERSION defaults to Cargo.toml's. The sha256 comes from dist/Scrawl-VERSION-arm64.dmg when it
# exists, else from the published DMG. Without --push it prints the change and leaves it unpushed.
set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PUSH=0
if [[ "${1:-}" == "--push" ]]; then
  PUSH=1
  shift
fi
VERSION="${1:-$(sed -n 's/^version = "\(.*\)"/\1/p' "${REPO_DIR}/Cargo.toml" | head -1)}"
BASE_URL="${SCRAWL_RELEASE_BASE_URL:-https://scrawl.kitlangton.com}"
DMG_NAME="Scrawl-${VERSION}-arm64.dmg"

WORK="$(mktemp -d)"
trap 'rm -rf "${WORK}"' EXIT

DMG="${REPO_DIR}/dist/${DMG_NAME}"
if [[ ! -f "${DMG}" ]]; then
  DMG="${WORK}/${DMG_NAME}"
  curl -fsSL -o "${DMG}" "${BASE_URL}/releases/${DMG_NAME}"
fi
SHA="$(shasum -a 256 "${DMG}" | cut -d' ' -f1)"

gh repo clone kitlangton/homebrew-tap "${WORK}/tap" -- --quiet --depth 1
CASK="${WORK}/tap/Casks/scrawl.rb"
sed -i '' -E \
  -e "s/^  version \".*\"/  version \"${VERSION}\"/" \
  -e "s/^  sha256 \".*\"/  sha256 \"${SHA}\"/" \
  "${CASK}"

cd "${WORK}/tap"
if git diff --quiet; then
  echo "Cask already at ${VERSION} (${SHA})"
  exit 0
fi
git diff
if [[ "${PUSH}" == 1 ]]; then
  git commit -qam "Update scrawl to ${VERSION}"
  git push -q origin HEAD
  echo "Pushed the Scrawl ${VERSION} cask"
else
  echo "Not pushed; run scripts/bump-cask.sh --push ${VERSION} once the DMG is live"
fi
