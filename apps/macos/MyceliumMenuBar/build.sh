#!/usr/bin/env bash
# Build the macOS menu bar app.
#
#   apps/macos/MyceliumMenuBar/build.sh                 # assemble into dist/macos
#   apps/macos/MyceliumMenuBar/build.sh --install       # and copy to ~/Applications
#   apps/macos/MyceliumMenuBar/build.sh --install --login-item
#
# This is a native, per-user app: it starts at login and talks to myceliumd
# through the same CLI a person would run. It holds no credentials and opens no
# privileged socket.
set -Eeuo pipefail

ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../../.." && pwd)
PKG=$ROOT/apps/macos/MyceliumMenuBar
OUT=$ROOT/dist/macos
INSTALL=0
LOGIN_ITEM=0
APP_NAME=Mycelium
BUNDLE="$OUT/$APP_NAME.app"

say() { printf '==> %s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

while [ $# -gt 0 ]; do
    case "$1" in
        --install) INSTALL=1; shift ;;
        --login-item) LOGIN_ITEM=1; shift ;;
        --out) OUT=${2:?--out needs a directory}; BUNDLE="$OUT/$APP_NAME.app"; shift 2 ;;
        -h|--help) sed -n '2,12p' "$0"; exit 0 ;;
        *) die "unknown option: $1" ;;
    esac
done

[ "$(uname -s)" = Darwin ] || die "the menu bar app is macOS-only"

say "Building $APP_NAME"
swift build -c release --package-path "$PKG"
BIN=$(swift build -c release --package-path "$PKG" --show-bin-path)/MyceliumMenuBar
[ -x "$BIN" ] || die "swift build produced no $BIN"

say "Assembling $BUNDLE"
rm -rf "$BUNDLE"
mkdir -p "$BUNDLE/Contents/MacOS" "$BUNDLE/Contents/Resources"
install -m 755 "$BIN" "$BUNDLE/Contents/MacOS/Mycelium"
install -m 644 "$PKG/Info.plist" "$BUNDLE/Contents/Info.plist"
# Ad-hoc is enough for a locally built menu bar app; a release sign/notarize is
# the packaging job's business, not the developer's.
codesign --force --deep --sign - "$BUNDLE" >/dev/null 2>&1 || true

if [ "$INSTALL" = 1 ]; then
    DEST="$HOME/Applications/$APP_NAME.app"
    say "Installing $DEST"
    mkdir -p "$HOME/Applications"
    rm -rf "$DEST"
    cp -R "$BUNDLE" "$DEST"
    xattr -dr com.apple.quarantine "$DEST" 2>/dev/null || true
    if [ "$LOGIN_ITEM" = 1 ]; then
        PLIST="$HOME/Library/LaunchAgents/dev.fpl.mycelium-menubar.plist"
        say "Installing login item $PLIST"
        mkdir -p "$HOME/Library/LaunchAgents"
        cat >"$PLIST" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>dev.fpl.mycelium-menubar</string>
    <key>ProgramArguments</key>
    <array><string>$DEST/Contents/MacOS/Mycelium</string></array>
    <key>RunAtLoad</key><true/>
    <key>KeepAlive</key><false/>
    <key>ProcessType</key><string>Interactive</string>
</dict>
</plist>
PLIST
        launchctl bootout "gui/$(id -u)" "$PLIST" 2>/dev/null || true
        launchctl bootstrap "gui/$(id -u)" "$PLIST" 2>/dev/null \
            || say "login item written; it starts at next login"
    fi
    say "Installed $DEST (open it once, or log in again)"
fi

say "Done: $BUNDLE"
