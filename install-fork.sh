#!/bin/sh
# Build this checkout and install it as a second, self-contained app: "Scrawl Fork.app".
#
#   ./install-fork.sh              build, install, sign, relaunch
#   SCRAWL_NO_LAUNCH=1 ./install-fork.sh   build and sign only
#
# Why it exists: scripts/install-app.sh is Kit's, and it installs over ~/Applications/Scrawl.app
# with Kit's Developer ID. This script never touches that app. It builds the same binaries into
# its own bundle, with its own bundle identifier, its own Developer identity (yours, so macOS
# keeps Screen Recording / Microphone / Accessibility granted across rebuilds - an ad-hoc "-"
# signature changes the cdhash every build and macOS reads that as a different app), its own
# config, its own captures folder and its own default shortcut, so both apps can be installed
# and launched at the same time without fighting.
#
# Everything it customizes is an environment variable:
#   SCRAWL_FORK_APP_NAME    bundle name              ("Scrawl Fork")
#   SCRAWL_FORK_APP_DIR     where it is installed    (~/Applications/Scrawl Fork.app)
#   SCRAWL_FORK_BUNDLE_ID   CFBundleIdentifier       (dev.averychan.scrawl.fork)
#   SCRAWL_SIGN_IDENTITY    signing identity         (see below)
#   SCRAWL_FORK_CONFIG      config.json              (~/.config/scrawl-fork/config.json)
#   SCRAWL_FORK_CAPTURES    captures folder          (~/Library/Application Support/scrawl-fork/captures)
#   SCRAWL_FORK_LINK        CLI symlink              (~/bin/scrawl-fork)
set -eu
# pgrep/pkill fail under some locales (C.UTF-8), which leaves the old app running.
export LC_ALL=C

REPO_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$REPO_DIR"

APP_NAME="${SCRAWL_FORK_APP_NAME:-Scrawl Fork}"
APP_DIR="${SCRAWL_FORK_APP_DIR:-${HOME}/Applications/${APP_NAME}.app}"
BUNDLE_ID="${SCRAWL_FORK_BUNDLE_ID:-dev.averychan.scrawl.fork}"
CONFIG="${SCRAWL_FORK_CONFIG:-${HOME}/.config/scrawl-fork/config.json}"
CAPTURES="${SCRAWL_FORK_CAPTURES:-${HOME}/Library/Application Support/scrawl-fork/captures}"
LINK="${SCRAWL_FORK_LINK:-${HOME}/bin/scrawl-fork}"
INGEST_PORT="${SCRAWL_FORK_INGEST_PORT:-47778}"
CLI_NAME="$(basename "$LINK")"

DEFAULT_IDENTITY="Apple Development: javerychan@outlook.com (7L8S7AGTK6)"
IDENTITY="${SCRAWL_SIGN_IDENTITY:-$DEFAULT_IDENTITY}"
if ! security find-identity -v -p codesigning 2>/dev/null | grep -qF "$IDENTITY"; then
  echo "note: '$IDENTITY' is not in the keychain; signing ad-hoc (permissions will be re-promted every build)" >&2
  IDENTITY="-"
fi

CONTENTS_DIR="${APP_DIR}/Contents"
MACOS_DIR="${CONTENTS_DIR}/MacOS"
RESOURCES_DIR="${CONTENTS_DIR}/Resources"

echo "==> Building"
if command -v taskpolicy >/dev/null 2>&1; then
  taskpolicy -c utility cargo build -j 3
else
  cargo build -j 3
fi

for binary in scrawl scrawl-helper; do
  [ -x "target/debug/$binary" ] || { echo "target/debug/$binary is missing" >&2; exit 1; }
done

# The app is stopped before its bundle is touched. Writing over a running Mach-O can kill it
# (the kernel faults mapped pages that changed underneath), so assembly happens after the quit,
# and the quit itself is never blind: a capture in flight is finished first.
in_flight() {
  running=0
  pgrep -f "${APP_DIR}/Contents/MacOS/scrawl" >/dev/null && running=1
  for dir in "$CAPTURES"/*.scrawl; do
    [ -d "$dir" ] || continue
    [ -f "$dir/scrawl.json" ] && continue
    # A live recording is the only thing that keeps the bundle changing.
    if [ "$running" = "1" ] && [ -n "$(find "$dir" -maxdepth 1 -mmin -2 2>/dev/null | head -1)" ]; then
      echo "$dir"
      return 0
    fi
  done
  return 1
}

echo "==> Stopping ${APP_NAME}"
if in_flight_dir=$(in_flight); then
  if [ "${SCRAWL_FORK_FORCE:-0}" != "1" ]; then
    echo "A capture is in flight: ${in_flight_dir}" >&2
    echo "Stop it (or let it finish), then run this again. Nothing was changed." >&2
    exit 1
  fi
  echo "note: SCRAWL_FORK_FORCE=1, restarting over a live capture anyway" >&2
fi

# Only this bundle's process: a running Scrawl.app from Kit is left alone. SIGTERM asks the app
# to finish a capture and quit, so this waits instead of killing it; if it doesn't go, nothing
# here is worth a lost recording.
pkill -TERM -f "${APP_DIR}/Contents/MacOS/scrawl" 2>/dev/null || true
for _ in $(seq 1 300); do
  pgrep -f "${APP_DIR}/Contents/MacOS/scrawl" >/dev/null || break
  sleep 0.1
done
if pgrep -f "${APP_DIR}/Contents/MacOS/scrawl" >/dev/null; then
  if [ "${SCRAWL_FORK_FORCE:-0}" != "1" ]; then
    echo "The old ${APP_NAME} is still running (it may be finishing a capture)." >&2
    echo "Quit it yourself, or rerun with SCRAWL_FORK_FORCE=1 to kill it. Nothing was changed." >&2
    exit 1
  fi
  pkill -KILL -f "${APP_DIR}/Contents/MacOS/scrawl" 2>/dev/null || true
  sleep 1
fi

# Left over from an earlier run: nothing is recording it now, so say that recovery owns it.
for dir in "$CAPTURES"/*.scrawl; do
  [ -d "$dir" ] || continue
  [ -f "$dir/scrawl.json" ] && continue
  echo "note: $(basename "$dir") has no bundle yet; \`scrawl recover $dir\` builds it" >&2
done

echo "==> Assembling ${APP_DIR}"
mkdir -p "$MACOS_DIR" "$RESOURCES_DIR"
cp -f target/debug/scrawl "$MACOS_DIR/scrawl"
cp -f target/debug/scrawl-helper "$MACOS_DIR/scrawl-helper"
chmod +x "$MACOS_DIR/scrawl" "$MACOS_DIR/scrawl-helper"
[ -f assets/Scrawl.icns ] && cp -f assets/Scrawl.icns "$RESOURCES_DIR/Scrawl.icns"

# LSEnvironment is how a LaunchServices launch (open, Finder, login) gets the fork's own config,
# CLI name and ingest port without a patch to the code. The captures folder is `"capturesDir"`
# in that config instead, so Settings › Storage keeps working; the CLI link below sets the same
# `SCRAWL_CONFIG`.
cat > "$CONTENTS_DIR/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleExecutable</key>
  <string>scrawl</string>
  <key>CFBundleIconFile</key>
  <string>Scrawl</string>
  <key>CFBundleIdentifier</key>
  <string>${BUNDLE_ID}</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>${APP_NAME}</string>
  <key>CFBundleDisplayName</key>
  <string>${APP_NAME}</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>0.0.0-fork</string>
  <key>CFBundleVersion</key>
  <string>1</string>
  <key>LSMinimumSystemVersion</key>
  <string>15.0</string>
  <key>LSUIElement</key>
  <true/>
  <key>NSHighResolutionCapable</key>
  <true/>
  <key>NSHumanReadableCopyright</key>
  <string>MIT. Fork of Scrawl by Christopher Langton.</string>
  <key>LSEnvironment</key>
  <dict>
    <key>SCRAWL_CONFIG</key>
    <string>${CONFIG}</string>
    <key>SCRAWL_CLI_NAME</key>
    <string>${CLI_NAME}</string>
    <key>SCRAWL_INGEST_PORT</key>
    <string>${INGEST_PORT}</string>
  </dict>
  <key>NSScreenCaptureUsageDescription</key>
  <string>Scrawl captures your screen to build annotated agent bundles.</string>
  <key>NSMicrophoneUsageDescription</key>
  <string>Scrawl records your voice to align transcripts with keyframes.</string>
  <key>NSCameraUsageDescription</key>
  <string>Scrawl mirrors and records a cabled iPhone's screen.</string>
  <key>NSSpeechRecognitionUsageDescription</key>
  <string>Scrawl transcribes your narration on this Mac.</string>
  <key>NSAppleEventsUsageDescription</key>
  <string>Scrawl reads the address of the browser tab you captured, to send a capture of a dev site to its project's session.</string>
  <key>SUEnableAutomaticChecks</key>
  <false/>
  <key>SUAutomaticallyUpdate</key>
  <false/>
</dict>
</plist>
PLIST

# Sparkle is deliberately absent: this build doesn't self-update. src/sparkle.rs dlopens it and
# copes with its absence (Settings › Updates greys out; the menu item logs and does nothing).

echo "==> Signing as ${IDENTITY}"
codesign --force --sign "$IDENTITY" --identifier "${BUNDLE_ID}.helper" "$MACOS_DIR/scrawl-helper"
codesign --force --sign "$IDENTITY" --identifier "$BUNDLE_ID" "$APP_DIR"

# Its own data, and shortcuts the installed Scrawl doesn't use, so both can run at once. Written
# only once: after this the fork owns its own settings, whatever you pick in Settings.
if [ ! -f "$CONFIG" ]; then
  mkdir -p "$(dirname "$CONFIG")" "$CAPTURES"
  cat > "$CONFIG" <<JSON
{
  "capturesDir": "${CAPTURES}",
  "shortcuts": {
    "record": { "keyCode": 2, "modifiers": 2560, "label": "⌥⇧D" },
    "paste": { "keyCode": 3, "modifiers": 2560, "label": "⌥⇧F" }
  }
}
JSON
fi

mkdir -p "$(dirname "$LINK")"
cat > "$LINK" <<LINKFILE
#!/bin/sh
# Runs the fork's CLI against the fork's own config and captures.
export SCRAWL_CONFIG="\${SCRAWL_CONFIG:-$CONFIG}"
exec "$MACOS_DIR/scrawl" "\$@"
LINKFILE
chmod +x "$LINK"

if [ "${SCRAWL_NO_LAUNCH:-0}" = "1" ]; then
  echo "==> Installed at ${APP_DIR} (not launched)"
  exit 0
fi

open -a "$APP_DIR" 2>/dev/null || open -n -a "$APP_DIR"
sleep 1
if pgrep -f "${APP_DIR}/Contents/MacOS/scrawl" >/dev/null; then
  echo "==> Running: ${APP_NAME} (${BUNDLE_ID})"
  echo "    shortcuts: ⌥⇧D record, ⌥⇧F paste   captures: ${CAPTURES}"
  echo "    CLI: ${LINK}"
else
  echo "==> Installed, but it did not stay running. Log: ~/Library/Logs/Scrawl/" >&2
  exit 1
fi
