#!/usr/bin/env bash
# Build a macOS installer for a Mycelium release.
#
#   packaging/macos/build-pkg.sh --release latest --out dist
#   packaging/macos/build-pkg.sh --archive mycelium-v0.1.3-aarch64-apple-darwin.tar.gz \
#       --claim MYC1-... --release-key <hex> --identity "Developer ID Installer: ..."
#
# The package installs the CLI under /usr/local/libexec/mycelium with symlinks in
# /usr/local/bin, optionally puts an app bundle in /Applications, and carries a
# config the postinstall uses to run the per-user install as the console user -
# the Mycelium daemon, tunnel and DNS are per-user services, so a root-only
# package is the wrong shape for them.
#
# Nothing here decides trust: the binaries come from a release whose manifests
# the installer verifies, and the key pinned here is what the host will accept
# for future updates. `--notarize` needs a `notarytool` keychain profile; without
# one the package is built unsigned/un-notarized and Gatekeeper will object.
set -Eeuo pipefail

ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
VERSION=
RELEASE=
ARCHIVE=
RELEASE_BASE=
OUT=$ROOT/dist/macos
IDENTITY=
NOTARY_PROFILE=
APP=
CLAIM=
RELEASE_KEY=
PKG_ID=dev.fpl.mycelium

say() { printf '==> %s\n' "$*"; }
note() { printf '    %s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

usage() {
    cat <<'USAGE'
usage: build-pkg.sh (--release TAG | --archive FILE) [options]

  --release TAG        fetch the target archive from GitHub releases (TAG or latest)
  --archive FILE       use an archive already on disk
  --release-base URL   override the download base (mirror or local fixture)
  --out DIR            output directory (default dist/macos)
  --identity NAME      "Developer ID Installer: ..." to sign the package
  --notarize PROFILE   notarytool keychain profile; submits and staples
  --app PATH           app bundle to install into /Applications (e.g. fungOS.app)
  --claim CODE         enrollment code to carry into the postinstall (optional)
  --release-key HEX    release key the host will trust for updates (repeatable)
  --version V          version for the package id and filename (default: archive tag)
  -h, --help           this text
USAGE
}

while [ $# -gt 0 ]; do
    case "$1" in
        --release) [ $# -ge 2 ] || die "--release needs a tag"; RELEASE=$2; shift 2 ;;
        --archive) [ $# -ge 2 ] || die "--archive needs a file"; ARCHIVE=$2; shift 2 ;;
        --release-base) [ $# -ge 2 ] || die "--release-base needs a URL"; RELEASE_BASE=$2; shift 2 ;;
        --out) [ $# -ge 2 ] || die "--out needs a directory"; OUT=$2; shift 2 ;;
        --identity) [ $# -ge 2 ] || die "--identity needs a name"; IDENTITY=$2; shift 2 ;;
        --notarize) [ $# -ge 2 ] || die "--notarize needs a keychain profile"; NOTARY_PROFILE=$2; shift 2 ;;
        --app) [ $# -ge 2 ] || die "--app needs a bundle"; APP=$2; shift 2 ;;
        --claim) [ $# -ge 2 ] || die "--claim needs a code"; CLAIM=$2; shift 2 ;;
        --release-key) [ $# -ge 2 ] || die "--release-key needs a hex key"; RELEASE_KEY="$RELEASE_KEY $2"; shift 2 ;;
        --version) [ $# -ge 2 ] || die "--version needs a value"; VERSION=$2; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) die "unknown option: $1 (try --help)" ;;
    esac
done

[ "$(uname -s)" = Darwin ] || die "a macOS package is built on macOS"
case "$(uname -m)" in
    arm64) TARGET=aarch64-apple-darwin ;;
    x86_64) TARGET=x86_64-apple-darwin ;;
    *) die "unsupported architecture $(uname -m)" ;;
esac
[ -n "$RELEASE" ] || [ -n "$ARCHIVE" ] || die "pass --release TAG or --archive FILE"

WORK=$(mktemp -d "${TMPDIR:-/tmp}/mycelium-pkg.XXXXXX")
trap 'rm -rf "$WORK"' EXIT

if [ -z "$ARCHIVE" ]; then
    case "$RELEASE" in
        latest) base=${RELEASE_BASE:-https://github.com/mycelium-linux/mycelium/releases/latest/download} ;;
        v*) base=${RELEASE_BASE:-https://github.com/mycelium-linux/mycelium/releases/download/$RELEASE} ;;
        *) base=${RELEASE_BASE:-https://github.com/mycelium-linux/mycelium/releases/download/v$RELEASE} ;;
    esac
    base=${base%/}
    ARCHIVE=$WORK/mycelium-$TARGET.tar.gz
    say "Downloading $base/mycelium-v$RELEASE-$TARGET.tar.gz"
    curl -fsSL "$base/mycelium-v${RELEASE#v}-$TARGET.tar.gz" -o "$ARCHIVE" \
        || die "could not download the $TARGET archive; pass --archive"
fi
[ -f "$ARCHIVE" ] || die "no such archive: $ARCHIVE"

if [ -z "$VERSION" ]; then
    case "$(basename "$ARCHIVE")" in
        mycelium-v*-$TARGET.tar.gz) VERSION=$(basename "$ARCHIVE" | sed "s/^mycelium-v//; s/-$TARGET.tar.gz$//") ;;
        *) VERSION=0.0.0 ;;
    esac
fi

# AppleDouble files (`._name`) come from resource forks on a copy; a package
# payload has no use for them.
export COPYFILE_DISABLE=1
stage=$WORK/root
unpack=$WORK/unpack
mkdir -p "$stage/usr/local/libexec/mycelium" "$stage/usr/local/bin" "$unpack"
tar -C "$unpack" -xzf "$ARCHIVE"
for binary in mycelium myceliumd mycelium-frontdoor mycelium-ingress mycelium-dns-resolver geoded mycelium-vfs iroh-tun; do
    [ -f "$unpack/$binary" ] || die "$ARCHIVE has no $binary"
    install -m 755 "$unpack/$binary" "$stage/usr/local/libexec/mycelium/$binary"
done
# /usr/local/bin gets the CLI only; the rest are resolved from libexec so a
# future package can move them without breaking PATH lookups.
ln -sf /usr/local/libexec/mycelium/mycelium "$stage/usr/local/bin/mycelium"
ln -sf /usr/local/libexec/mycelium/mycelium-vfs "$stage/usr/local/bin/mycelium-vfs"
# One command for the privileged half. The package cannot run sudo for a person,
# so this is the thing the postinstall points at and the README names.
cat >"$stage/usr/local/bin/mycelium-setup" <<'SETUP'
#!/bin/sh
# Finish a packaged Mycelium install: the overlay, the DNS resolver's system
# wiring and the :443 ingress need root, and root needs a person at the keyboard.
set -eu
CONFIG=/usr/local/etc/mycelium/package.env
ARGS="--yes"
if [ -r "$CONFIG" ]; then
    CLAIM=$(sed -n 's/^MYCELIUM_PKG_CLAIM=//p' "$CONFIG")
    KEYS=$(sed -n 's/^MYCELIUM_PKG_RELEASE_KEYS=//p' "$CONFIG")
    [ -n "$CLAIM" ] && ARGS="$ARGS --claim $CLAIM"
    for key in $KEYS; do ARGS="$ARGS --release-key $key"; done
fi
exec /usr/local/libexec/mycelium/install.sh $ARGS
SETUP
chmod 755 "$stage/usr/local/bin/mycelium-setup"
install -m 755 "$ROOT/install.sh" "$stage/usr/local/libexec/mycelium/install.sh"
install -m 644 "$ROOT/uninstall.sh" "$stage/usr/local/libexec/mycelium/uninstall.sh" 2>/dev/null \
    || die "uninstall.sh is missing from this checkout"
chmod 755 "$stage/usr/local/libexec/mycelium/uninstall.sh"

if [ -n "$APP" ]; then
    [ -d "$APP" ] || die "no app bundle at $APP"
    mkdir -p "$stage/Applications"
    cp -R "$APP" "$stage/Applications/$(basename "$APP")"
    note "installs $(basename "$APP") into /Applications"
fi

mkdir -p "$stage/usr/local/etc/mycelium"
{
    printf 'MYCELIUM_PKG_VERSION=%s\n' "$VERSION"
    printf 'MYCELIUM_PKG_TARGET=%s\n' "$TARGET"
    [ -n "$CLAIM" ] && printf 'MYCELIUM_PKG_CLAIM=%s\n' "$CLAIM"
    [ -n "$RELEASE_KEY" ] && printf 'MYCELIUM_PKG_RELEASE_KEYS=%s\n' "$RELEASE_KEY"
    true
} >"$stage/usr/local/etc/mycelium/package.env"
chmod 600 "$stage/usr/local/etc/mycelium/package.env"

mkdir -p "$WORK/scripts"
cat >"$WORK/scripts/postinstall" <<'POSTINSTALL'
#!/bin/sh
# Run as root by Installer. Mycelium's services are per-user by design, so the
# user-facing part is handed to the console user rather than installed for root.
# What runs here needs no privilege: binaries onto disk, the daemon service for
# that user, and a report. The pieces that do need root (the tunnel, the DNS
# resolver's system wiring, the :443 ingress) are what `mycelium-setup` finishes,
# because sudo needs a person at the keyboard.
set -eu
CONFIG=/usr/local/etc/mycelium/package.env
VERSION=$(sed -n 's/^MYCELIUM_PKG_VERSION=//p' "$CONFIG" 2>/dev/null || true)
USER_NAME=$(stat -f %Su /dev/console 2>/dev/null || true)
LOG=/var/log/mycelium-postinstall.log
exec >>"$LOG" 2>&1
echo "== $(date -u +%Y-%m-%dT%H:%M:%SZ) Mycelium $VERSION postinstall =="
if [ -z "$USER_NAME" ] || [ "$USER_NAME" = root ] || [ "$USER_NAME" = loginwindow ]; then
    echo "no console user; nothing per-user to install"
    echo "run 'mycelium-setup' after logging in"
    exit 0
fi
echo "console user: $USER_NAME"
su -l "$USER_NAME" -c     "/usr/local/libexec/mycelium/install.sh --yes --no-vpn --no-frontdoor --no-dns --no-soil --no-vfs --no-bob --no-geode --no-operator --no-enroll" \
    || echo "per-user install needs attention"
su -l "$USER_NAME" -c "/usr/local/bin/mycelium doctor" || true
echo "== done; run 'mycelium-setup' to finish the privileged parts =="
exit 0
POSTINSTALL
chmod 755 "$WORK/scripts/postinstall"

# pkgbuild turns every extended attribute into an AppleDouble sibling; the
# payload should be the files, not their metadata forks.
xattr -cr "$stage" 2>/dev/null || true
mkdir -p "$OUT"
component=$WORK/Mycelium.pkg
say "Building component package for $TARGET $VERSION"
pkgbuild --root "$stage" --identifier "$PKG_ID.pkg" --version "$VERSION" \
    --scripts "$WORK/scripts" --install-location / "$component" >/dev/null

final="$OUT/Mycelium-$VERSION-$TARGET.pkg"
rm -f "$final"
if [ -n "$IDENTITY" ]; then
    say "Signing with $IDENTITY"
    productbuild --package "$component" --identifier "$PKG_ID" --version "$VERSION" \
        --sign "$IDENTITY" "$final" >/dev/null
else
    warn "no --identity: the package is unsigned and Gatekeeper will refuse it on another Mac"
    productbuild --package "$component" --identifier "$PKG_ID" --version "$VERSION" \
        "$final" >/dev/null
fi

if [ -n "$NOTARY_PROFILE" ]; then
    say "Notarizing with keychain profile $NOTARY_PROFILE"
    xcrun notarytool submit "$final" --keychain-profile "$NOTARY_PROFILE" --wait \
        || die "notarization failed"
    xcrun stapler staple "$final" || warn "could not staple; the ticket is online-only"
fi

# A dmg is the container people expect to receive; the pkg is the installer.
dmg="$OUT/Mycelium-$VERSION-$TARGET.dmg"
rm -f "$dmg"
mkdir -p "$WORK/dmg"
cp "$final" "$WORK/dmg/"
cat >"$WORK/dmg/README.txt" <<EOF
Mycelium $VERSION

Double-click Mycelium-$VERSION-$TARGET.pkg to install.

The package installs the Mycelium tools and then runs the per-user install for
whoever is logged in: the daemon, DNS resolver, tunnel and store are that
person's services, not the machine's.

After it finishes, \`mycelium doctor\` says whether everything is running.
EOF
hdiutil create -quiet -volname "Mycelium $VERSION" -srcfolder "$WORK/dmg" \
    -ov -format UDZO "$dmg" || warn "could not build the dmg"

say "Wrote:"
note "$final"
[ -f "$dmg" ] && note "$dmg"
note "payload: /usr/local/libexec/mycelium (binaries + install.sh), /usr/local/bin/mycelium"
[ -n "$APP" ] && note "app bundle: /Applications/$(basename "$APP")"
