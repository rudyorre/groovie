#!/usr/bin/env bash
# Builds an unsigned iOS `.ipa` on macOS CI without an Apple Developer account.
# Users sign it on-device with their own free Apple ID via SideStore, AltStore, or LiveContainer.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT_DIR"

VERSION="$(jq -r '.version' apps/ios/src-tauri/tauri.conf.json)"
if [ $# -ge 1 ]; then
  case "$1" in
    /*) OUT_IPA="$1" ;;
    *) OUT_IPA="$ROOT_DIR/$1" ;;
  esac
else
  OUT_IPA="$ROOT_DIR/Groovie_${VERSION}_ios.ipa"
fi

# Tauri v2's generated Xcode project requires Xcode 16+.
XCODE_MAJOR="$(xcodebuild -version | awk '/^Xcode / { split($2, v, "."); print v[1] }')"
if [ -z "${XCODE_MAJOR:-}" ] || [ "$XCODE_MAJOR" -lt 16 ]; then
  LATEST_XCODE="$(ls -d /Applications/Xcode_1[6-9]*.app /Applications/Xcode_2[0-9]*.app 2>/dev/null | sort -V | tail -n1 || true)"
  if [ -n "$LATEST_XCODE" ]; then
    echo "Selecting Xcode 16+ at $LATEST_XCODE"
    sudo xcode-select -s "$LATEST_XCODE/Contents/Developer"
  fi
fi
xcodebuild -version

if ! command -v xcodegen >/dev/null 2>&1; then
  echo "Installing xcodegen via Homebrew..."
  HOMEBREW_NO_AUTO_UPDATE=1 brew install xcodegen
fi

rustup target add aarch64-apple-ios

(
  cd "$ROOT_DIR/apps/ios"
  rm -rf src-tauri/gen/apple
  npx tauri ios init --ci --skip-targets-install
  npx tauri ios build --archive-only --no-sign --ci
)

APP_BUNDLE="$(find "$ROOT_DIR/apps/ios/src-tauri/gen/apple/build" -maxdepth 6 -path '*.xcarchive/Products/Applications/*.app' | head -n1 || true)"
if [ -z "$APP_BUNDLE" ]; then
  APP_BUNDLE="$(find "$ROOT_DIR/apps/ios/src-tauri/gen/apple" -maxdepth 6 -path '*Release-iphoneos/*.app' | head -n1 || true)"
fi
if [ -z "$APP_BUNDLE" ] || [ ! -d "$APP_BUNDLE" ]; then
  echo "::error::Could not locate built iOS .app bundle under apps/ios/src-tauri/gen/apple"
  find "$ROOT_DIR/apps/ios/src-tauri/gen/apple" -maxdepth 5 || true
  exit 1
fi

echo "Packaging app bundle: $APP_BUNDLE"

STAGE_DIR="$(mktemp -d)"
trap 'rm -rf "$STAGE_DIR"' EXIT

mkdir -p "$STAGE_DIR/Payload"
STAGE_APP="$STAGE_DIR/Payload/$(basename "$APP_BUNDLE")"
ditto "$APP_BUNDLE" "$STAGE_APP"

PLIST="$STAGE_APP/Info.plist"
ensure_plist_string() {
  local key="$1"
  local val="$2"
  /usr/libexec/PlistBuddy -c "Set :${key} \"${val}\"" "$PLIST" 2>/dev/null \
    || /usr/libexec/PlistBuddy -c "Add :${key} string \"${val}\"" "$PLIST"
}
ensure_plist_bool() {
  local key="$1"
  local val="$2"
  /usr/libexec/PlistBuddy -c "Set :${key} ${val}" "$PLIST" 2>/dev/null \
    || /usr/libexec/PlistBuddy -c "Add :${key} bool ${val}" "$PLIST"
}

ensure_plist_string "NSLocalNetworkUsageDescription" \
  "Groovie discovers and controls your Govee lights directly over your local Wi-Fi network without cloud servers."
ensure_plist_bool "UIFileSharingEnabled" "true"
ensure_plist_bool "LSSupportsOpeningDocumentsInPlace" "true"
ensure_plist_bool "ITSAppUsesNonExemptEncryption" "false"

# Apply a clean ad-hoc signature over the updated Info.plist so sideloaders
# (SideStore / AltStore / LiveContainer) can re-sign cleanly with a free Apple ID.
codesign --force --sign - --timestamp=none "$STAGE_APP"

mkdir -p "$(dirname "$OUT_IPA")"
rm -f "$OUT_IPA"
ditto -c -k --sequesterRsrc --keepParent "$STAGE_DIR/Payload" "$OUT_IPA"

if ! unzip -l "$OUT_IPA" | grep -Eq 'Payload/[^/]+\.app/Info\.plist'; then
  echo "::error::Generated IPA is missing Payload/*.app/Info.plist"
  unzip -l "$OUT_IPA" || true
  exit 1
fi

ls -lh "$OUT_IPA"
echo "Built unsigned iOS IPA: $OUT_IPA"
