#!/bin/sh
# Assembles Kiled.app around the cargo binary, with the icon from packaging/app-icon.png.
# Usage: scripts/bundle.sh [debug|release]   → prints the .app path
set -eu

profile="${1:-debug}"
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

case "$profile" in
  debug) cargo build -p kiled ;;
  release) cargo build --release -p kiled ;;
  *) echo "usage: $0 [debug|release]" >&2; exit 2 ;;
esac

out="target/$profile"
app="$out/Kiled.app"
version="$(cargo pkgid -p kiled | sed 's/.*[#@]//')"

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$out/kiled" "$app/Contents/MacOS/kiled"
sed "s/__VERSION__/$version/g" packaging/Info.plist > "$app/Contents/Info.plist"

iconset="$out/AppIcon.iconset"
rm -rf "$iconset"
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" packaging/app-icon.png --out "$iconset/icon_${size}x${size}.png" >/dev/null
  double=$((size * 2))
  sips -z "$double" "$double" packaging/app-icon.png --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/AppIcon.icns"

# Ad-hoc seal for local runs; scripts/release.sh re-signs with Developer ID (DESIGN.md §11).
codesign --force --sign - "$app" >/dev/null 2>&1
echo "$app"
