#!/usr/bin/env bash
set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_DIR="${HOME}/Applications/Scrawl.app"
CONTENTS_DIR="${APP_DIR}/Contents"
MACOS_DIR="${CONTENTS_DIR}/MacOS"
RESOURCES_DIR="${CONTENTS_DIR}/Resources"
SIGN_ID="Developer ID Application: Christopher Langton (QC99C9JE59)"

cd "${REPO_DIR}"
taskpolicy -c utility cargo build -j 3

mkdir -p "${MACOS_DIR}" "${RESOURCES_DIR}" "${HOME}/bin"

cp -f "${REPO_DIR}/target/debug/scrawl" "${MACOS_DIR}/scrawl"
cp -f "${REPO_DIR}/target/debug/scrawl-helper" "${MACOS_DIR}/scrawl-helper"
chmod +x "${MACOS_DIR}/scrawl" "${MACOS_DIR}/scrawl-helper"

if [[ -f "${REPO_DIR}/assets/Scrawl.icns" ]]; then
  cp -f "${REPO_DIR}/assets/Scrawl.icns" "${RESOURCES_DIR}/Scrawl.icns"
fi

cat > "${CONTENTS_DIR}/Info.plist" <<'PLIST'
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
  <string>0.1.0</string>
  <key>CFBundleVersion</key>
  <string>1</string>
  <key>LSMinimumSystemVersion</key>
  <string>14.0</string>
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
</dict>
</plist>
PLIST

if security find-identity -v -p codesigning 2>/dev/null | grep -q "${SIGN_ID}"; then
  IDENTITY="${SIGN_ID}"
else
  IDENTITY="-"
fi
codesign --force --sign "${IDENTITY}" --identifier "dev.kitlangton.scrawl.helper" "${MACOS_DIR}/scrawl-helper"
codesign --force --sign "${IDENTITY}" --identifier "dev.kitlangton.scrawl" "${APP_DIR}"

ln -sf "${MACOS_DIR}/scrawl" "${HOME}/bin/scrawl"

pkill -f "Scrawl.app/Contents/MacOS/scrawl" 2>/dev/null || true
pkill -f "target/debug/scrawl" 2>/dev/null || true
nohup "${MACOS_DIR}/scrawl" app >/dev/null 2>&1 &
sleep 0.2
pgrep -fl "Scrawl.app/Contents/MacOS/scrawl"
