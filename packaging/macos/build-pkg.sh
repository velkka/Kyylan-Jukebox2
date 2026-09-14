#!/bin/sh
# Builds the macOS installer from a built program.
#
#   packaging/macos/build-pkg.sh <kyylan-jukebox binary> <version> <output.pkg>
#
# The binary is normally universal (lipo of the aarch64 and x86_64 builds). The app bundle is
# ad-hoc signed; the pkg installs it with the LaunchAgent, not relocatable, for any user.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
binary=$1
version=$2
output=$3

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
root="$work/root"
app="$root/Applications/Kyylan Jukebox.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" "$root/Library/LaunchAgents"

cp "$binary" "$app/Contents/MacOS/kyylan-jukebox"
chmod 755 "$app/Contents/MacOS/kyylan-jukebox"
sed "s/@VERSION@/$version/g" "$here/Info.plist" >"$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist" >/dev/null

# The icon, from the same generated logo as the other platforms'.
icon="$repo/build/icon.png"
if [ ! -f "$icon" ]; then
    echo "build/icon.png is missing: run node scripts/gen-icons.cjs first" >&2
    exit 1
fi
iconset="$work/icon.iconset"
mkdir "$iconset"
for size in 16 32 128 256 512; do
    sips -z "$size" "$size" "$icon" --out "$iconset/icon_${size}x${size}.png" >/dev/null
    double=$((size * 2))
    if [ "$double" -le 512 ]; then
        sips -z "$double" "$double" "$icon" --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
    fi
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/icon.icns"

cp "$here/org.kyylan.jukebox.plist" "$root/Library/LaunchAgents/"
plutil -lint "$root/Library/LaunchAgents/org.kyylan.jukebox.plist" >/dev/null

# Extended attributes from wherever this was built would go into the payload as ._ files.
xattr -cr "$root"
export COPYFILE_DISABLE=1

# Ad hoc: no Developer ID, but a valid signature, which Apple Silicon requires to run it.
codesign --force --sign - --timestamp=none "$app" 2>/dev/null
codesign --verify --strict "$app"

# Installed where it's built for: a copy of the app elsewhere with the same identifier
# mustn't be upgraded instead.
pkgbuild --analyze --root "$root" "$work/component.plist" >/dev/null
plutil -replace 0.BundleIsRelocatable -bool NO "$work/component.plist"

pkgbuild --root "$root" \
    --component-plist "$work/component.plist" \
    --scripts "$here/scripts" \
    --identifier org.kyylan.jukebox \
    --version "$version" \
    --install-location / \
    --ownership recommended \
    "$work/kyylan-jukebox.pkg" >/dev/null

sed "s/@VERSION@/$version/g" "$here/distribution.xml" >"$work/distribution.xml"
mkdir -p "$(dirname "$output")"
productbuild --distribution "$work/distribution.xml" --package-path "$work" "$output" >/dev/null
echo "wrote $output"
