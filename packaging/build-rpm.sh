#!/usr/bin/env bash
# Builds dist/linshot-<version>-1.<arch>.rpm for the machine it runs on.
# Like the .deb, run it on the oldest release you want to support: the
# glibc requirement comes from the build host (CI uses Ubuntu 22.04, whose
# build installs on Fedora 36+ and other RPM distributions with GTK >= 4.6).
# Needs: a recent Rust toolchain (rustup), libgtk-4-dev, libdbus-1-dev,
# pkg-config and rpmbuild (`apt install rpm` on Debian/Ubuntu).
set -euo pipefail
cd "$(dirname "$0")/.."
ROOT=$(pwd)

NAME=linshot
VERSION=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
ARCH=$(rpm --eval '%{_arch}')

cargo build --release --locked
BIN="${CARGO_TARGET_DIR:-target}/release/linshot"

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
STAGE="$TMP/stage"
install -Dm755 "$BIN" "$STAGE/usr/bin/linshot"
install -Dm644 data/io.github.linshot.Linshot.desktop "$STAGE/usr/share/applications/io.github.linshot.Linshot.desktop"
install -Dm644 data/linshot.svg "$STAGE/usr/share/icons/hicolor/scalable/apps/linshot.svg"
for i in data/icons/*.svg; do install -Dm644 "$i" "$STAGE/usr/share/icons/hicolor/scalable/apps/$(basename "$i")"; done
install -Dm644 README.md "$STAGE/usr/share/doc/$NAME/README.md"
install -Dm644 LICENSE "$STAGE/usr/share/licenses/$NAME/LICENSE"
# notices for the Rust crates compiled into the binary (MIT/Apache/MPL require them)
python3 packaging/third-party-licenses.py "$(rustc -vV | sed -n 's/^host: //p')" \
  | gzip -9n > "$STAGE/usr/share/doc/$NAME/THIRD-PARTY-LICENSES.gz"

mkdir -p "$TMP/rpm"
# xz payload: readable by every rpm in use; no build-host paths in the package
rpmbuild -bb \
  --define "_topdir $TMP/rpm" \
  --define "_binary_payload w9.xzdio" \
  --define "_build_id_links none" \
  --define "version $VERSION" \
  --define "stage $STAGE" \
  packaging/linshot.spec >"$TMP/rpmbuild.log" 2>&1 || { cat "$TMP/rpmbuild.log" >&2; exit 1; }

mkdir -p dist
OUT="dist/$NAME-$VERSION-1.$ARCH.rpm"
cp "$TMP/rpm/RPMS/$ARCH/$NAME-$VERSION-1.$ARCH.rpm" "$OUT"

# the library requirements must have been found in the binary
if ! rpm -qp --requires "$OUT" | grep -q 'libgtk-4.so.1'; then
  rpm -qp --requires "$OUT" >&2
  echo "rpmbuild did not generate the shared-library requirements" >&2
  exit 1
fi
echo "built $OUT"
