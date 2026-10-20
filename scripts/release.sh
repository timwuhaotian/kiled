#!/bin/sh
# Builds Kiled, signs it with Developer ID, notarizes and staples it, and packs it in a DMG under
# releases/, which git tracks (DESIGN.md §11). Run locally; CI never builds releases.
# Usage: scripts/release.sh   → releases/Kiled-arm64.dmg (+ .sha256, latest.toml), replacing the last
# release: downloads are always the latest, and the version is only for us (Info.plist, git
# history) and for the updater Kiled will get, which will read latest.toml (DESIGN.md §11).
#
# Signing: KILED_SIGN_IDENTITY, default "Developer ID Application" (the one such identity in the
# login keychain).
# Notarization credentials, the first of:
#   KILED_NOTARY_KEY, KILED_NOTARY_KEY_ID, KILED_NOTARY_ISSUER   an App Store Connect API key
#   KILED_NOTARY_PROFILE   a `xcrun notarytool store-credentials` profile
#   the `asc` CLI's stored App Store Connect key, if `asc` is installed
#   the notarytool profile `kil-notary` (shared with Kil: same team)
set -eu

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

identity="${KILED_SIGN_IDENTITY:-Developer ID Application}"
work="target/release/dist"

# Submits $1 and waits; fails unless Apple accepts it.
notarize() {
  if [ -n "${KILED_NOTARY_KEY:-}" ]; then
    result="$(xcrun notarytool submit "$1" --key "$KILED_NOTARY_KEY" --key-id "$KILED_NOTARY_KEY_ID" \
      --issuer "$KILED_NOTARY_ISSUER" --wait --output-format json)"
  elif [ -z "${KILED_NOTARY_PROFILE:-}" ] && command -v asc >/dev/null; then
    result="$(asc notarization submit --file "$1" --wait)"
  else
    result="$(xcrun notarytool submit "$1" --keychain-profile "${KILED_NOTARY_PROFILE:-kil-notary}" \
      --wait --output-format json)"
  fi
  echo "$result"
  case "$result" in
    *'"status":"Accepted"'*) ;;
    *) echo "notarization of $1 was not accepted" >&2; exit 1 ;;
  esac
}

dmg="releases/Kiled-arm64.dmg"
# README's download link must give this file, checked before the long build and notarization.
link="https://github.com/timwuhaotian/kiled/raw/main/$dmg"
if ! grep -qF "($link)" README.md; then
  echo "README.md must link the release as $link" >&2
  exit 1
fi

app="$(sh scripts/bundle.sh release | tail -n 1)"
# After the build: before it, `cargo pkgid` still reports the version in Cargo.lock, and a
# bumped Cargo.toml only reaches the lock there. Read it late, or the last line reports the
# version it replaces.
version="$(cargo pkgid -p kiled | sed 's/.*[#@]//')"
codesign --force --timestamp --options runtime \
  --entitlements packaging/kiled.entitlements \
  --sign "$identity" "$app"
codesign --verify --strict --verbose=2 "$app"

# The app first, so a copy dragged out of the DMG carries its own ticket, even offline.
rm -rf "$work"
mkdir -p "$work/dmg" releases
ditto -c -k --keepParent "$app" "$work/Kiled.zip"
notarize "$work/Kiled.zip"
xcrun stapler staple "$app"

ditto "$app" "$work/dmg/Kiled.app"
ln -s /Applications "$work/dmg/Applications"
rm -f "$dmg"
hdiutil create -volname "Kiled" -srcfolder "$work/dmg" -fs HFS+ -format UDZO "$dmg"
codesign --force --timestamp --sign "$identity" "$dmg"
notarize "$dmg"
xcrun stapler staple "$dmg"

spctl --assess --type execute --verbose=2 "$app"
spctl --assess --type open --context context:primary-signature --verbose=2 "$dmg"
(cd releases && shasum -a 256 "$(basename "$dmg")" > "$(basename "$dmg").sha256")
printf 'version = "%s"\nsha256 = "%s"\n' "$version" "$(awk '{print $1}' "$dmg.sha256")" \
  > releases/latest.toml
echo "$dmg ($version)"
