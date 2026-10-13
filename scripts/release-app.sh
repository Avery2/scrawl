#!/usr/bin/env bash
set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODE="${1:-prepare}"
VERSION="${SCRAWL_VERSION:-$(sed -n 's/^version = "\(.*\)"/\1/p' "${REPO_DIR}/Cargo.toml" | head -1)}"
BUILD_NUMBER="${SCRAWL_BUILD_NUMBER:-1}"
BASE_URL="${SCRAWL_RELEASE_BASE_URL:-https://scrawl.kitlangton.com}"
SIGN_ID="Developer ID Application: Christopher Langton (QC99C9JE59)"
SPARKLE_PUBLIC_KEY="mIek27lttJe8cIBqVZFhh6reRKjpTx1h9ZY9OKWPtuM="

DIST_DIR="${REPO_DIR}/dist"
UPDATES_DIR="${DIST_DIR}/updates"
APP_BUNDLE="${DIST_DIR}/Scrawl.app"
DMG_NAME="Scrawl-${VERSION}-arm64.dmg"
LATEST_DMG="Scrawl-latest-arm64.dmg"
ZIP_NAME="Scrawl-${VERSION}-arm64.zip"

cd "${REPO_DIR}"
SPARKLE_DIR="$("${REPO_DIR}/scripts/setup-sparkle.sh")"

taskpolicy -c utility cargo build --release -j 3 --offline

rm -rf "${APP_BUNDLE}" "${DIST_DIR}/staging" "${UPDATES_DIR}"
mkdir -p "${APP_BUNDLE}/Contents/MacOS" "${APP_BUNDLE}/Contents/Resources" "${APP_BUNDLE}/Contents/Frameworks" "${DIST_DIR}/staging" "${UPDATES_DIR}"

cp -f "${REPO_DIR}/target/release/scrawl" "${APP_BUNDLE}/Contents/MacOS/scrawl"
cp -f "${REPO_DIR}/target/release/scrawl-helper" "${APP_BUNDLE}/Contents/MacOS/scrawl-helper"
chmod +x "${APP_BUNDLE}/Contents/MacOS/scrawl" "${APP_BUNDLE}/Contents/MacOS/scrawl-helper"

if [[ -f "${REPO_DIR}/assets/Scrawl.icns" ]]; then
  cp -f "${REPO_DIR}/assets/Scrawl.icns" "${APP_BUNDLE}/Contents/Resources/Scrawl.icns"
fi

/usr/bin/ditto "${SPARKLE_DIR}/Sparkle.framework" "${APP_BUNDLE}/Contents/Frameworks/Sparkle.framework"

cat > "${APP_BUNDLE}/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key>
  <string>scrawl</string>
  <key>CFBundleIconFile</key>
  <string>Scrawl</string>
  <key>CFBundleIdentifier</key>
  <string>dev.kitlangton.scrawl</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>Scrawl</string>
  <key>CFBundleDisplayName</key>
  <string>Scrawl</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>${VERSION}</string>
  <key>CFBundleVersion</key>
  <string>${BUILD_NUMBER}</string>
  <key>LSMinimumSystemVersion</key>
  <string>26.0</string>
  <key>LSUIElement</key>
  <true/>
  <key>NSHighResolutionCapable</key>
  <true/>
  <key>NSScreenCaptureUsageDescription</key>
  <string>Scrawl captures your screen to build annotated agent bundles.</string>
  <key>NSMicrophoneUsageDescription</key>
  <string>Scrawl records your voice to align transcripts with keyframes.</string>
  <key>NSCameraUsageDescription</key>
  <string>Scrawl mirrors and records a cabled iPhone's screen.</string>
  <key>NSSpeechRecognitionUsageDescription</key>
  <string>Scrawl transcribes your narration on this Mac.</string>
  <key>SUEnableAutomaticChecks</key>
  <true/>
  <key>SUAutomaticallyUpdate</key>
  <false/>
  <key>SUAllowsAutomaticUpdates</key>
  <false/>
  <key>SUFeedURL</key>
  <string>${BASE_URL}/appcast.xml</string>
  <key>SUPublicEDKey</key>
  <string>${SPARKLE_PUBLIC_KEY}</string>
  <key>SUVerifyUpdateBeforeExtraction</key>
  <true/>
</dict>
</plist>
PLIST

if security find-identity -v -p codesigning 2>/dev/null | grep -q "${SIGN_ID}"; then
  IDENTITY="${SIGN_ID}"
else
  IDENTITY="-"
fi

SPARKLE_VER="${APP_BUNDLE}/Contents/Frameworks/Sparkle.framework/Versions/B"
codesign --force --sign "${IDENTITY}" "${SPARKLE_VER}/XPCServices/Installer.xpc"
codesign --force --sign "${IDENTITY}" --preserve-metadata=entitlements "${SPARKLE_VER}/XPCServices/Downloader.xpc"
codesign --force --sign "${IDENTITY}" "${SPARKLE_VER}/Autoupdate"
codesign --force --sign "${IDENTITY}" "${SPARKLE_VER}/Updater.app"
codesign --force --sign "${IDENTITY}" "${APP_BUNDLE}/Contents/Frameworks/Sparkle.framework"
codesign --force --sign "${IDENTITY}" --identifier "dev.kitlangton.scrawl.helper" "${APP_BUNDLE}/Contents/MacOS/scrawl-helper"
codesign --force --sign "${IDENTITY}" --identifier "dev.kitlangton.scrawl" "${APP_BUNDLE}"

/usr/bin/ditto "${APP_BUNDLE}" "${DIST_DIR}/staging/Scrawl.app"
ln -s /Applications "${DIST_DIR}/staging/Applications"
rm -f "${DIST_DIR}/${DMG_NAME}" "${DIST_DIR}/${LATEST_DMG}" "${DIST_DIR}/${ZIP_NAME}"
hdiutil create -quiet -fs APFS -format ULFO -volname Scrawl -srcfolder "${DIST_DIR}/staging" "${DIST_DIR}/${DMG_NAME}"
codesign --force --sign "${IDENTITY}" "${DIST_DIR}/${DMG_NAME}"
cp -f "${DIST_DIR}/${DMG_NAME}" "${DIST_DIR}/${LATEST_DMG}"

/usr/bin/ditto -c -k --sequesterRsrc --keepParent "${APP_BUNDLE}" "${DIST_DIR}/${ZIP_NAME}"
cp -f "${DIST_DIR}/${ZIP_NAME}" "${UPDATES_DIR}/${ZIP_NAME}"

if security find-generic-password -s 'https://sparkle-project.org' -a ed25519 -w >/dev/null 2>&1; then
  security find-generic-password -s 'https://sparkle-project.org' -a ed25519 -w \
    | "${SPARKLE_DIR}/bin/generate_appcast" \
      --ed-key-file - \
      --download-url-prefix "${BASE_URL}/releases/" \
      "${UPDATES_DIR}"
fi

echo "Prepared ${DIST_DIR}/${DMG_NAME} and ${DIST_DIR}/${ZIP_NAME}"

if [[ -d "${REPO_DIR}/site/public" ]]; then
  mkdir -p "${REPO_DIR}/site/public/releases"
  cp -f "${DIST_DIR}/${DMG_NAME}" "${REPO_DIR}/site/public/releases/${DMG_NAME}"
  cp -f "${DIST_DIR}/${LATEST_DMG}" "${REPO_DIR}/site/public/releases/${LATEST_DMG}"
  cp -f "${DIST_DIR}/${ZIP_NAME}" "${REPO_DIR}/site/public/releases/${ZIP_NAME}"
  cp -f "${DIST_DIR}/${ZIP_NAME}" "${REPO_DIR}/site/public/releases/Scrawl-latest-arm64.zip"
  if [[ -f "${UPDATES_DIR}/appcast.xml" ]]; then
    cp -f "${UPDATES_DIR}/appcast.xml" "${REPO_DIR}/site/public/appcast.xml"
  fi
fi

if [[ "${MODE}" == "publish" ]]; then
  (cd "${REPO_DIR}/site" && bun run build && bunx wrangler@latest deploy)
fi
