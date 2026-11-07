#!/usr/bin/env bash
# Builds, signs and (when credentials are available) notarizes the macOS release.
#
# Environment:
#   APPLE_SIGNING_IDENTITY        codesign identity. A "Developer ID Application" identity is required
#                                 for notarization; any other identity builds a locally runnable app.
#   Notarization (optional, one of):
#     APPLE_API_KEY + APPLE_API_ISSUER + APPLE_API_KEY_PATH   App Store Connect API key
#     APPLE_ID + APPLE_PASSWORD (app-specific) + APPLE_TEAM_ID
#   Updater artifacts (optional):
#     TAURI_SIGNING_PRIVATE_KEY (or _PATH) [+ TAURI_SIGNING_PRIVATE_KEY_PASSWORD]
#     ANYTOOLS_UPDATE_URL         https URL of latest.json baked into the app
#   TARGET                        e.g. universal-apple-darwin (default: host architecture)
#
# Without notarization credentials the script still builds and signs, then says what is missing.
set -euo pipefail
cd "$(dirname "$0")/.."

say() { printf '\033[1m==> %s\033[0m\n' "$*"; }
warn() { printf '\033[33m!!  %s\033[0m\n' "$*" >&2; }

[[ "$(uname)" == Darwin ]] || { echo "macOS only" >&2; exit 1; }
: "${APPLE_SIGNING_IDENTITY:?set APPLE_SIGNING_IDENTITY (see: security find-identity -v -p codesigning)}"

notarize=false
if [[ "$APPLE_SIGNING_IDENTITY" == *"Developer ID Application"* ]] ||
  security find-identity -v -p codesigning | grep -F "$APPLE_SIGNING_IDENTITY" | grep -q "Developer ID Application"; then
  if [[ -n "${APPLE_API_KEY:-}" && -n "${APPLE_API_ISSUER:-}" && -n "${APPLE_API_KEY_PATH:-}" ]] ||
    [[ -n "${APPLE_ID:-}" && -n "${APPLE_PASSWORD:-}" && -n "${APPLE_TEAM_ID:-}" ]]; then
    notarize=true
  else
    warn "Developer ID identity found but no notarization credentials: building without notarization"
  fi
else
  warn "identity is not a Developer ID Application certificate: the app runs on this Mac, but Gatekeeper will block it elsewhere"
fi
if ! $notarize; then
  # Tauri notarizes automatically whenever these are set; make sure it doesn't try.
  unset APPLE_API_KEY APPLE_API_ISSUER APPLE_API_KEY_PATH APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID
fi

configs=()
if [[ -n "${TAURI_SIGNING_PRIVATE_KEY:-}${TAURI_SIGNING_PRIVATE_KEY_PATH:-}" ]]; then
  configs+=(--config ../app/tauri.release.conf.json) # relative to ui/, where the build runs
  if [[ -n "${TAURI_SIGNING_PRIVATE_KEY_PATH:-}" && -z "${TAURI_SIGNING_PRIVATE_KEY:-}" ]]; then
    export TAURI_SIGNING_PRIVATE_KEY="$(cat "$TAURI_SIGNING_PRIVATE_KEY_PATH")"
  fi
  export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}"
  say "updater artifacts: on"
else
  warn "no TAURI_SIGNING_PRIVATE_KEY: building without updater artifacts"
fi
[[ -n "${ANYTOOLS_UPDATE_URL:-}" ]] && say "update URL: $ANYTOOLS_UPDATE_URL" || warn "ANYTOOLS_UPDATE_URL not set: in-app updates disabled in this build"

target_args=()
[[ -n "${TARGET:-}" ]] && target_args+=(--target "$TARGET")

say "tests"
cargo test --workspace --quiet
(cd ui && npm ci --silent && npm run --silent typecheck)

say "build ($([[ $notarize == true ]] && echo 'sign + notarize' || echo 'sign'))"
# ${arr[@]+"${arr[@]}"}: macOS ships bash 3.2, where an empty array is "unbound" under set -u.
(cd ui && npm run tauri -- build --bundles app,dmg ${configs[@]+"${configs[@]}"} ${target_args[@]+"${target_args[@]}"})

bundle_dir="target/${TARGET:+$TARGET/}release/bundle"
app="$bundle_dir/macos/anyTools.app"
say "verify"
codesign --verify --deep --strict "$app"
codesign -dv "$app" 2>&1 | grep -E '^(Authority|TeamIdentifier|Identifier)=' | head -4
if $notarize; then
  xcrun stapler validate "$app"
  spctl --assess --type execute -v "$app"
fi
say "artifacts"
find "$bundle_dir" -maxdepth 2 \( -name '*.dmg' -o -name '*.app.tar.gz' -o -name '*.sig' \) -print
