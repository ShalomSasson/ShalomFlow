#!/usr/bin/env bash
# Re-sign a locally built ShalomFlow.app for development on macOS.
#
# Why this exists: tauri.conf.json signs with the ad-hoc identity ("-") plus
# hardened runtime. Ad-hoc signatures pin the TCC (Accessibility) grant to the
# exact binary hash, so every rebuild silently invalidates the permission; and
# hardened-runtime library validation refuses Homebrew's libonnxruntime, which
# has no Team ID. Signing with a local self-signed identity and WITHOUT
# hardened runtime fixes both: the designated requirement becomes
# `identifier + certificate leaf`, which is stable across rebuilds.
#
# One-time setup (already done if `security find-identity -v -p codesigning`
# lists it): create a self-signed "ShalomFlow Dev" code-signing certificate in
# the login keychain and trust it for code signing.
#
# Usage: scripts/macos-dev-sign.sh [path/to/ShalomFlow.app]
set -euo pipefail

IDENTITY="ShalomFlow Dev"
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="${1:-$REPO_ROOT/src-tauri/target/release/bundle/macos/ShalomFlow.app}"
ENTITLEMENTS="$REPO_ROOT/src-tauri/Entitlements.plist"

if [[ ! -d "$APP" ]]; then
  echo "error: app bundle not found: $APP" >&2
  exit 1
fi

if security find-identity -v -p codesigning | grep -q "$IDENTITY"; then
  echo "Signing with '$IDENTITY' (stable TCC grants, no hardened runtime)"
  codesign --force --sign "$IDENTITY" --entitlements "$ENTITLEMENTS" "$APP"
else
  echo "warning: '$IDENTITY' identity not found; falling back to ad-hoc" >&2
  echo "         (Accessibility must be re-granted after every rebuild)" >&2
  codesign --force --sign - --entitlements "$ENTITLEMENTS" "$APP"
fi

codesign -d -r- "$APP" 2>&1 | grep "^designated" || true
echo "done: $APP"
