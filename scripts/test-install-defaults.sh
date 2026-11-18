#!/usr/bin/env bash
# Tests for the install path's defaults: which path an invocation takes, and
# what that path enables.
#
# Run: scripts/test-install-defaults.sh
set -Eeuo pipefail

ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
WORK=$(mktemp -d "${TMPDIR:-/tmp}/mycelium-install-defaults.XXXXXX")
trap 'rm -rf "$WORK"' EXIT

fail() { printf 'FAIL: %s\n' "$1" >&2; exit 1; }
ok() { printf 'ok - %s\n' "$1"; }

MYCELIUM_INSTALL_LIB=1
export MYCELIUM_INSTALL_LIB
# shellcheck disable=SC1091
source "$ROOT/install.sh"

# A checkout can build, so it keeps the source build; a piped copy has nothing
# to build and takes the published release.
SCRIPT_DIR=$ROOT
SCRIPT_PATH=$ROOT/install.sh
in_checkout || fail "the repository checkout was not recognised"
# `curl | bash` has no script path even when the working directory is a checkout:
# the release is still the only path that needs no compiler.
SCRIPT_PATH=
in_checkout && fail "a piped installer was treated as a checkout"
SCRIPT_PATH=$ROOT/install.sh
SCRIPT_DIR=$WORK/nowhere
mkdir -p "$SCRIPT_DIR"
in_checkout && fail "an empty directory was treated as a checkout"
ok "only a source tree counts as a checkout"

ENABLE_UPDATES=ask
RELEASE_VERSION=
resolve_updates_default
[ "$ENABLE_UPDATES" = no ] || fail "a source install enabled updates by default"
ok "a source install leaves automatic updates off"

ENABLE_UPDATES=ask
RELEASE_VERSION=latest
resolve_updates_default
[ "$ENABLE_UPDATES" = yes ] || fail "a release install left updates off"
ok "a release install enables automatic updates"

ENABLE_UPDATES=no
RELEASE_VERSION=latest
resolve_updates_default
[ "$ENABLE_UPDATES" = no ] || fail "--no-enable-updates was overridden"
ok "an explicit opt-out survives the release default"

# apply_update_policy is what actually writes the policy, so check the command
# it builds rather than the boolean above.
LOCAL_BIN=$WORK/bin
mkdir -p "$LOCAL_BIN"
cat >"$LOCAL_BIN/mycelium" <<'STUB'
#!/bin/sh
printf '%s\n' "$*" >>"$STUB_LOG"
STUB
chmod 755 "$LOCAL_BIN/mycelium"
export STUB_LOG=$WORK/policy.log

DRY_RUN=0
ENABLE_UPDATES=yes
RELEASE_CHANNEL=stable
apply_update_policy >/dev/null
want='update policy enable --channel stable --write'
got=$(cat "$STUB_LOG")
[ "$got" = "$want" ] || fail "release install wrote '$got', wanted '$want'"
ok "a release install enables updates on the release channel"

: >"$STUB_LOG"
ENABLE_UPDATES=no
RELEASE_CHANNEL=
apply_update_policy >/dev/null
got=$(cat "$STUB_LOG")
[ "$got" = 'update policy disable --write' ] \
    || fail "source install wrote '$got'"
ok "a source install leaves the policy disabled"

# Components that only exist as source builds are off in a release install, but
# a profile or an explicit flag still speaks for them.
RELEASE_VERSION=latest
DECLARED=
VFS=yes
SOIL=ask
BOB=no
apply_release_defaults >/dev/null
[ "$LOCAL_COMPONENT_DEFAULT" = no ] || fail "release install asked for a compiler"
[ "$RELEASE_ONLY" = yes ] || fail "release install was not marked release-only"
[ "$VFS" = ask ] || fail "a profile's VFS=yes survived a release default"
[ "$SOIL" = ask ] || fail "release install did not ask about Soil"
[ "$BOB" = ask ] || fail "release install did not ask about bob"
ok "a release install defaults source-built components off"

RELEASE_VERSION=latest
DECLARED=" SOIL BOB "
SOIL=yes
VFS=no
BOB=no
apply_release_defaults >/dev/null
[ "$SOIL" = yes ] || fail "an explicit --soil was dropped in favour of the default"
[ "$BOB" = no ] || fail "an explicit --no-bob was overridden"
ok "an explicit component flag wins over the release default"

RELEASE_VERSION=
DECLARED=
SOIL=yes
VFS=yes
BOB=yes
apply_release_defaults >/dev/null
[ "$LOCAL_COMPONENT_DEFAULT" = yes ] || fail "a source install stopped offering components"
[ "$SOIL" = yes ] || fail "a source install dropped Soil"
ok "a source install keeps the component defaults"

# Channels: a canary install is a second install with its own state, binaries
# and label, and it never takes the machine's system components from stable.
CHANNEL=stable
apply_channel
[ "$MYCELIUM_HOME" = "$HOME/.mycelium" ] || fail "stable changed the state directory"
[ "$PREFIX" = "$HOME/.local" ] || fail "stable changed the prefix"
[ "$CHANNEL_SUFFIX" = "" ] || fail "stable grew a label suffix"
ok "the stable channel keeps the historic paths"

CHANNEL=canary
apply_channel
[ "$MYCELIUM_HOME" = "$HOME/.mycelium-canary" ] || fail "canary did not get its own state"
[ "$PREFIX" = "$HOME/.local/canary" ] || fail "canary did not get its own prefix"
[ "$LOCAL_BIN" = "$PREFIX/bin" ] || fail "canary LOCAL_BIN was not recomputed"
[ "$BIN_DIR" = "$MYCELIUM_HOME/bin" ] || fail "canary BIN_DIR was not recomputed"
[ "$CHANNEL_SUFFIX" = "-canary" ] || fail "canary label suffix missing"
[ "$MYCELIUM_CHANNEL" = canary ] || fail "canary did not export MYCELIUM_CHANNEL"
ok "the canary channel gets its own state, binaries and label"

if (CHANNEL=beta; apply_channel) >/dev/null 2>&1; then
    fail "an unknown channel was accepted"
fi
ok "an unknown channel is refused"

VPN=ask
VFS=ask
SOIL=ask
BOB=ask
OPERATOR=ask
FRONTDOOR=ask
GEODE=ask
ENROLL=ask
DECLARED=
CHANNEL=canary
apply_channel_defaults >/dev/null
for name in VPN VFS SOIL BOB OPERATOR FRONTDOOR GEODE; do
    eval "value=\${$name}"
    [ "$value" = no ] || fail "canary left $name on"
done
[ "$ENROLL" = no ] || fail "canary left enrollment on"
ok "canary leaves the system-wide components off"

VPN=yes
DECLARED=" VPN "
apply_channel_defaults >/dev/null
[ "$VPN" = yes ] || fail "an explicit --vpn was dropped on canary"
ok "an explicit flag still wins on canary"

printf 'all install default tests passed\n'
