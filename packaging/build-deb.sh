#!/usr/bin/env bash
# Builds dist/linshot_<version>_<arch>.deb for the machine it runs on.
# Run on the oldest Debian/Ubuntu release you want to support (glibc/GTK
# requirements come from the build host). Needs: cargo, libgtk-4-dev,
# libdbus-1-dev, pkg-config, dpkg-dev.
set -euo pipefail
cd "$(dirname "$0")/.."

NAME=linshot
VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
ARCH=$(dpkg --print-architecture)
MAINT="${DEB_MAINTAINER:-Aditya Telange <21258296+adityatelange@users.noreply.github.com>}"

cargo build --release --locked

PKG="dist/${NAME}_${VERSION}_${ARCH}"
rm -rf "$PKG"
install -Dm755 target/release/linshot "$PKG/usr/bin/linshot"
install -Dm644 data/io.github.linshot.Linshot.desktop "$PKG/usr/share/applications/io.github.linshot.Linshot.desktop"
install -Dm644 data/linshot.svg "$PKG/usr/share/icons/hicolor/scalable/apps/linshot.svg"
install -Dm644 README.md "$PKG/usr/share/doc/$NAME/README.md"
install -Dm644 LICENSE "$PKG/usr/share/doc/$NAME/copyright"
printf '%s (%s) unstable; urgency=medium\n\n  * Release %s.\n\n -- %s  %s\n' \
  "$NAME" "$VERSION" "$VERSION" "$MAINT" "$(date -R)" | gzip -9n > "$PKG/usr/share/doc/$NAME/changelog.Debian.gz"

# shared-library dependencies, computed from the binary
TMP=$(mktemp -d)
mkdir -p "$TMP/debian"
printf 'Source: %s\n\nPackage: %s\nArchitecture: any\n' "$NAME" "$NAME" > "$TMP/debian/control"
DEPS=$(cd "$TMP" && dpkg-shlibdeps -O -e "$OLDPWD/$PKG/usr/bin/linshot" 2>/dev/null | sed -n 's/^shlibs:Depends=//p')
rm -rf "$TMP"

SIZE=$(du -sk "$PKG/usr" | cut -f1)
mkdir -p "$PKG/DEBIAN"
cat > "$PKG/DEBIAN/control" <<CTRL
Package: $NAME
Version: $VERSION
Architecture: $ARCH
Maintainer: $MAINT
Installed-Size: $SIZE
Depends: $DEPS
Recommends: xdg-desktop-portal, adwaita-icon-theme, librsvg2-common
Suggests: grim, xdg-desktop-portal-gnome | xdg-desktop-portal-kde | xdg-desktop-portal-wlr
Section: graphics
Priority: optional
Homepage: https://github.com/adityatelange/linshot
Description: screenshot and annotation tool for X11 and Wayland
 Linshot captures regions, windows or full screens and opens them in an
 annotation editor with rectangles, ellipses, arrows, freehand, text,
 highlighter, pixelate/blur obfuscation, numbered step markers and crop.
 Results can be saved, copied to the clipboard or printed.
 .
 Works on X11 (with Print-key hotkeys and window snapping) and on Wayland
 (GNOME, KDE, wlroots) through xdg-desktop-portal or grim.
CTRL

find "$PKG" -type d -exec chmod 755 {} +
dpkg-deb --root-owner-group --build "$PKG" >/dev/null
echo "built $PKG.deb"
