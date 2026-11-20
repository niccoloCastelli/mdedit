#!/bin/sh
# Builds target/macos/mdedit.app (and, with --dmg, target/macos/mdedit-<version>.dmg)
# from the release binary. Run from the repository root: `make app` / `make dmg`.
#
# The bundle is ad-hoc signed, which is enough to run it on the machine that
# built it. A bundle downloaded from elsewhere needs a Developer ID signature
# and notarisation, or Gatekeeper will refuse to open it.
set -eu

CARGO=${CARGO:-cargo}
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)
OUT=target/macos
APP=$OUT/mdedit.app

"$CARGO" build --release -p mdedit-app

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/mdedit "$APP/Contents/MacOS/mdedit"
sed "s/@VERSION@/$VERSION/g" packaging/macos/Info.plist > "$APP/Contents/Info.plist"
cp assets/icons/mdedit.icns assets/icons/mdedit-file-md.icns "$APP/Contents/Resources/"
printf 'APPL????' > "$APP/Contents/PkgInfo"
plutil -lint "$APP/Contents/Info.plist" >/dev/null
codesign --force --sign - "$APP"
echo "$APP"

if [ "${1:-}" = "--dmg" ]; then
    DMG=$OUT/mdedit-$VERSION.dmg
    STAGE=$OUT/dmg
    rm -rf "$STAGE" "$DMG"
    mkdir -p "$STAGE"
    cp -R "$APP" "$STAGE/"
    ln -s /Applications "$STAGE/Applications"
    hdiutil create -volname "mdedit $VERSION" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
    rm -rf "$STAGE"
    echo "$DMG"
fi
