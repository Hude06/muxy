#!/bin/zsh
set -euo pipefail

ROOT=${0:A:h:h}
DIST="$ROOT/dist"
APP="$DIST/Pane.app"
VERSION=$(cargo metadata --no-deps --format-version 1 --manifest-path "$ROOT/Cargo.toml" \
    | python3 -c 'import json, sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "muxy-app"))')

cargo build --release --manifest-path "$ROOT/Cargo.toml" -p muxy-app -p muxy-server -p muxy-cli

rm -rf "$APP" "$DIST/AppIcon.iconset"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$DIST/AppIcon.iconset"
for binary in muxy-app muxy-server muxy; do
    cp "$ROOT/target/release/$binary" "$APP/Contents/MacOS/$binary"
done

for size in 16 32 128 256 512; do
    sips -z $size $size "$ROOT/packaging/macos/AppIconBeta.png" \
        --out "$DIST/AppIcon.iconset/icon_${size}x${size}.png" >/dev/null
    sips -z $((size * 2)) $((size * 2)) "$ROOT/packaging/macos/AppIconBeta.png" \
        --out "$DIST/AppIcon.iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil --convert icns --output "$APP/Contents/Resources/AppIcon.icns" "$DIST/AppIcon.iconset"
rm -rf "$DIST/AppIcon.iconset"

PYTHONPATH="$ROOT/scripts" python3 - "$APP/Contents/Info.plist" "$VERSION" <<'EOF'
import plistlib
import sys

from release import bundle_info

path, version = sys.argv[1:]
info = bundle_info("2.0.0-beta.1", "1")
info.update(CFBundleVersion="0", MuxyVersion=version, CFBundleGetInfoString=f"Pane {version}")
with open(path, "wb") as file:
    plistlib.dump(info, file)
EOF
printf 'APPL????' > "$APP/Contents/PkgInfo"

for binary in muxy-server muxy; do
    codesign --force --sign - "$APP/Contents/MacOS/$binary"
done
codesign --force --sign - --entitlements "$ROOT/packaging/macos/Muxy.entitlements" "$APP/Contents/MacOS/muxy-app"
codesign --force --sign - --entitlements "$ROOT/packaging/macos/Muxy.entitlements" "$APP"

echo "Built $APP"
