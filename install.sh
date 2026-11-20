#!/usr/bin/env bash
# Mycelium workstation installer.
#
# One command, then prompts:
#
#   curl -fsSL https://raw.githubusercontent.com/mycelium-linux/mycelium/master/install.sh | bash
#
# Components are asked for in order. Every prompt can be answered up front:
#
#   ./install.sh --yes
#   ./install.sh --code MYC1-...
#   ./install.sh --no-vpn --no-bob
#   ./install.sh --source . --bin-dir target/release --no-enroll
#
# The VPN component installs a root-owned iroh-tun service. The user daemon owns
# the identity and the published observation; root owns only the tunnel child.
# A source build is the default because the VPN supervisor and installer changes
# are not in a signed release yet.

set -Eeuo pipefail

# The script's own path, when it has one: `curl | bash` runs with an empty
# BASH_SOURCE and a working directory that is not a source tree. Where this
# script came from decides whether the install can build in place.
SCRIPT_PATH=${BASH_SOURCE[0]:-}
SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]:-$0}")" 2>/dev/null && pwd || pwd)
TS=$(date +%Y%m%dT%H%M%S)
MYCELIUM_PREFIX_SET=${MYCELIUM_PREFIX:+1}
PREFIX=${MYCELIUM_PREFIX:-$HOME/.local}
MYCELIUM_HOME_SET=${MYCELIUM_HOME:+1}
MYCELIUM_HOME=${MYCELIUM_HOME:-$HOME/.mycelium}
BIN_DIR=$MYCELIUM_HOME/bin
LOCAL_BIN=$PREFIX/bin
GATEWAY=${MYCELIUM_GATEWAY:-https://sso.fpl.dev}
SUFFIX=${MYCELIUM_DNS_SUFFIX:-myc}
SITE=${MYCELIUM_SITE:-}
ROOT_LIBEXEC=${MYCELIUM_ROOT_LIBEXEC:-/usr/local/libexec/mycelium}

MYCELIUM_REPO=${MYCELIUM_REPO:-https://github.com/mycelium-linux/mycelium.git}
# Install channel. A canary install is a second, independent install on the same
# machine: its own state, its own binaries, its own service label. The stable
# install owns the machine's system-wide ingress (DNS, tunnel, frontdoor);
# canary never does, so it cannot take those away from stable.
CHANNEL=${MYCELIUM_CHANNEL:-stable}
CHANNEL_SUFFIX=
RELEASE_REPO=${MYCELIUM_RELEASE_REPO:-mycelium-linux/mycelium}
RELEASE_VERSION=
RELEASE_BASE=
RELEASE_DIR=
RELEASE_CHANNEL=
# Release public keys this host will accept, hex. Repeatable; seeded from the
# environment so a fleet installer can pin the authority's key once.
RELEASE_KEYS=${MYCELIUM_RELEASE_KEYS:-}
IROH_TUN_REPO=${IROH_TUN_REPO:-https://github.com/ajmwagar/iroh-tun.git}
VFS_REPO=${VFS_REPO:-https://github.com/mycelium-linux/fungos-vfs.git}
SOIL_REPO=${SOIL_REPO:-https://github.com/mycelium-linux/soil.git}
BOB_REPO=${BOB_REPO:-https://github.com/FuturePresentLabs/bob.git}

DRY_RUN=0
YES=0
FORCE=0
START=1
# Components that build from their own repositories. A release archive does not
# carry them, so a release install defaults them off; see apply_release_defaults.
LOCAL_COMPONENT_DEFAULT=yes
RELEASE_ONLY=no
# Automatic updates are on by default for a release install and off for a
# source build; `ask` is resolved once the install path is known. See
# apply_update_policy.
ENABLE_UPDATES=ask
ENROLL=ask
VPN=ask
VFS=ask
SOIL=ask
BOB=ask
OPERATOR=ask
GEODE=ask
DNS=yes
FRONTDOOR=ask
TRUST_CERT=ask
PROFILE=full
VFS_ROLE=full
DECLARED=
CLAIM=
CLAIM_FILE=
LOCAL_FRONTDOOR=0
SRC=
BIN_SRC=
OVERLAY=
TUN=
STATUS_PATH=
APP_BUNDLE=
APP_URL=
MENUBAR_APP=
SOIL_RUNTIME=
SOIL_SOURCE=
SPORES_SOURCE=
TERMINAL_HOST_BIN=
BOB_SRC=
SCHEDULER_URL=${FPL_BOB_SCHEDULER_URL:-}
CLIENT_TOKEN=${FPL_BOB_CLIENT_TOKEN:-}
REMOTE_MAC_HOST=${FPL_BOB_REMOTE_MAC_HOST:-}
REMOTE_LINUX_HOST=${FPL_BOB_REMOTE_LINUX_HOST:-}
NO_DIST=0
REACHABLE=
VFS_CONFIG=
VFS_ROOT=${MYCELIUM_VFS_ROOT:-$HOME/fungos-node}
VFS_MOUNT=${MYCELIUM_VFS_MOUNT:-$HOME/fungOS}
FAILED=
UNSTARTED=
SUDO=
LOG=
UPDATER_PAUSED=0

usage() {
    cat <<'USAGE'
usage: install.sh [options]

Prompts for components on a terminal. Use --yes or explicit component flags
when no terminal is available; the installer will not assume defaults silently.

Core:
  --gateway URL          enrollment gateway (default https://sso.fpl.dev)
  --claim CODE           redeem a pairing or enrollment code
  --code CODE            alias for --claim
  --claim-file PATH      redeem a claim from a file
  --local-frontdoor      install the loopback HTTPS frontdoor
  --trust-cert           trust the generated frontdoor CA (prompted by default)
  --no-enroll            install/update only; do not enroll
  --no-start             write service definitions but do not start them
  --enable-updates       enable automatic updates (default: enabled for --release,
                         disabled for a source or --bin-dir build)
  --no-enable-updates    leave automatic updates disabled
  --source DIR           build Mycelium from this checkout
  --bin-dir DIR          install prebuilt binaries from DIR instead of building
  --release TAG          download published release binaries (TAG, or latest)
                         (default when this script is not running in a checkout)
  --release-base URL     override the release download base (mirror or local test dir)
  --release-key HEX      release key to trust for automatic updates (repeatable;
                         default: MYCELIUM_RELEASE_KEYS)
  --prefix DIR           install user binaries under DIR/bin (default ~/.local)
  --force                rebuild iroh-tun instead of reusing the installed binary

Components:
  --vpn / --no-vpn       iroh-tun IPv6 overlay (default yes, needs sudo)
  --vfs / --no-vfs       fungos-vfs content service and FUSE mount (default yes;
                         no in a release install, where it would need a compiler)
  --soil / --no-soil     Soil CLI/runtime and fungOS.app (default yes; no in a
                         release install, where it would need a compiler)
  --bob / --no-bob       fpl-bob build offload (default yes; no in a release install)
  --operator / --no-operator
                         frontdoor, skills, completions, update policy (default yes)
  --geode / --no-geode   optional geode credential broker (default no)
  --dns / --no-dns       local *.myc resolver and its system wiring (default yes)
  --frontdoor / --no-frontdoor
                         loopback HTTPS frontdoor plus its ingress services
                         (default yes; runs on its own service so a node profile
                         keeps the ingress without the operator surface)

Profiles:
  --profile NAME         preset components: full (default), canvas, node, flash,
                         minimal. Explicit component flags always win; a profile
                         only fills the ones you did not pass.
                           canvas   Soil + VFS for a desktop launcher
                           node     Soil; no build offload, operator or geode
                           flash    node with a read-mostly VFS cache
                           minimal  Mycelium only
  --vfs-role full|cache  VFS mount role (default full). cache keeps writes on the
                         authority and pulls content into a local cache, which is
                         what a flash-backed node wants.

Component inputs:
  --overlay CIDR         IPv6 prefix this node owns (detected when possible)
  --tun IFACE            tunnel interface (default utun220 on macOS, tun0 on Linux)
  --status PATH          root-owned tunnel status file
  --app PATH             fungOS.app bundle to install
  --app-url URL          .zip containing fungOS.app
  --menubar-app PATH     Mycelium.app menu bar bundle (apps/macos/MyceliumMenuBar/build.sh)
  --soil-runtime DIR     soil-home/soil-store/soil-spore/files-serve binaries
  --spores-src DIR       Spores checkout (contains spores/)
  --terminal-host PATH   prebuilt terminal-host binary (built from the Soil checkout if omitted)
  --vfs-config PATH      Unibus client config for the VFS content service
  --vfs-root PATH        VFS content root (default ~/fungos-node)
  --vfs-mount PATH       FUSE mount point (default ~/fungOS)
  --bob-src DIR          bob checkout to build
  --scheduler-url URL    sccache-dist scheduler (blank = cache-only)
  --client-token TOKEN   sccache-dist client token (hidden prompt if omitted)
  --remote-mac-host HOST bob remote host for darwin builds
  --remote-linux-host HOST
                         bob remote host for Linux builds
  --no-dist              keep sccache cache-only
  --reachable CIDR       repeatable DNS resolver scope (defaults to existing config)

Other:
  -y, --yes              accept recommended defaults
  --dry-run              print the plan; change nothing
  -h, --help             this text
USAGE
}

say() { printf '==> %s\n' "$*"; if [ -n "$LOG" ]; then printf '==> %s\n' "$*" >>"$LOG"; fi; }
note() { printf '    %s\n' "$*"; if [ -n "$LOG" ]; then printf '    %s\n' "$*" >>"$LOG"; fi; }
warn() { printf 'warning: %s\n' "$*" >&2; if [ -n "$LOG" ]; then printf 'warning: %s\n' "$*" >>"$LOG"; fi; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

run() {
    printf '+ %s\n' "$*"
    if [ -n "$LOG" ]; then printf '+ %s\n' "$*" >>"$LOG"; fi
    if [ "$DRY_RUN" = 1 ]; then return 0; fi
    "$@"
}

run_sudo() {
    if [ "$(id -u)" -eq 0 ]; then run "$@"; else run "$SUDO" "$@"; fi
}

root_own() {
    if [ "$(uname -s)" = Darwin ]; then
        run_sudo chown root:wheel "$@"
    else
        run_sudo chown root:root "$@"
    fi
}

ensure_root_dir() {
    if [ -L "$1" ]; then die "refusing symlinked root directory: $1"; fi
    run_sudo install -d -m 755 "$1"
    root_own "$1"
    run_sudo chmod 755 "$1"
}

mkdir_p() {
    if [ "$DRY_RUN" = 1 ]; then note "would mkdir -p $*"; else mkdir -p "$@"; fi
}

need_root() {
    if [ "$(id -u)" -eq 0 ]; then return 0; fi
    if [ "$DRY_RUN" = 1 ]; then note "would request sudo to $1"; return 0; fi
    say "Root is required to $1"
    # `sudo -v` is a validation request, not a command, so a host that allows
    # every command with NOPASSWD can still refuse it against a password rule.
    # Probe with a real command first, and only fall back to interactive -v.
    "$SUDO" -n true 2>/dev/null || "$SUDO" -v \
        || die "sudo was not granted; privileged setup was not started"
}

have_tty() {
    ( : </dev/tty ) 2>/dev/null
}

ask_yes() {
    question=$1
    default=$2
    if [ "$YES" = 1 ]; then [ "$default" = yes ]; return; fi
    if have_tty; then
        if [ "$default" = yes ]; then suffix="Y/n"; else suffix="y/N"; fi
        printf '%s [%s] ' "$question" "$suffix" >&2
        read -r answer </dev/tty || answer=""
        case "${answer:-$default}" in
            [Yy]*) return 0 ;;
            *) return 1 ;;
        esac
    fi
    [ "$default" = yes ]
}

ask_text() {
    question=$1
    default=$2
    if [ "$YES" = 1 ]; then printf '%s' "$default"; return; fi
    if have_tty; then
        if [ -n "$default" ]; then
            printf '%s [%s]: ' "$question" "$default" >&2
        else
            printf '%s: ' "$question" >&2
        fi
        read -r answer </dev/tty || answer=""
        printf '%s' "${answer:-$default}"
    else
        printf '%s' "$default"
    fi
}

ask_secret() {
    question=$1
    if [ "$YES" = 1 ]; then printf '%s' "${2:-}"; return; fi
    if have_tty; then
        printf '%s: ' "$question" >&2
        read -rs answer </dev/tty || answer=""
        printf '\n' >&2
        printf '%s' "$answer"
    else
        printf '%s' "${2:-}"
    fi
}

choose() {
    name=$1
    default=$2
    label=$3
    eval "value=\${$name}"
    case "$value" in
        yes|no) return 0 ;;
    esac
    if ask_yes "$label" "$default"; then eval "$name=yes"; else eval "$name=no"; fi
}

run_component() {
    name=$1
    fn=$2
    set +e
    ( set -e; "$fn" )
    rc=$?
    set -e
    if [ "$rc" -ne 0 ]; then
        FAILED="$FAILED $name"
        warn "$name needs attention"
    fi
}

install_atomic() {
    # `local`: this function is called from functions that hold their own `src`
    # (build_iroh_tun, install_bob_from, the release fetcher). Without it the
    # second call inside such a caller computes its source from the first
    # call's source, looks for `<binary>/target/release/iroh-tun` and fails with
    # "missing binary" after the build that produced it succeeded.
    local src=$1
    local dest=$2
    local mode=${3:-755}
    if [ "$DRY_RUN" = 1 ]; then
        note "would install $src -> $dest (mode $mode)"
        return 0
    fi
    [ -f "$src" ] || { warn "missing binary: $src"; return 1; }
    mkdir -p "$(dirname "$dest")"
    if [ -e "$dest" ] || [ -L "$dest" ]; then cp -p "$dest" "$dest.pre-$TS"; fi
    install -m "$mode" "$src" "$dest.new.$$"
    mv -f "$dest.new.$$" "$dest"
    note "installed $dest"
}

ensure_rust() {
    export PATH="$HOME/.cargo/bin:$PATH"
    if command -v cargo >/dev/null 2>&1; then
        return 0
    fi
    if [ "$DRY_RUN" = 1 ]; then
        note "would install the Rust toolchain"
        return 0
    fi
    command -v curl >/dev/null 2>&1 || { warn "curl is required to install Rust"; return 1; }
    say "Installing the Rust toolchain"
    if ! curl --proto '=https' --tlsv1.2 -fsS https://sh.rustup.rs | sh -s -- -y --profile minimal; then
        warn "Rust toolchain installation failed"
        return 1
    fi
    export PATH="$HOME/.cargo/bin:$PATH"
    command -v cargo >/dev/null 2>&1 || { warn "cargo is still unavailable"; return 1; }
}

mycelium_source() {
    if [ -n "$SRC" ]; then printf '%s' "$SRC"; return; fi
    if in_checkout; then
        printf '%s' "$SCRIPT_DIR"
        return
    fi
    cache="${XDG_CACHE_HOME:-$HOME/.cache}/mycelium/install-src/mycelium"
    if [ "$DRY_RUN" = 1 ]; then printf '%s' "$cache"; return; fi
    mkdir -p "$(dirname "$cache")"
    # stdout is this function's return value, so every trace goes to stderr.
    # `run` prints the command it is about to execute, and a caller that does
    # `src=$(mycelium_source)` otherwise gets `+ git clone ...` glued to the
    # path and then fails with "not a Mycelium checkout: + git clone ...".
    if [ -d "$cache/.git" ]; then
        # The cache is derived state: keep its origin pointed at the repository
        # this run was told to use. Otherwise a moved or renamed default keeps
        # fetching the old one forever, and the source builds stale code.
        run git -C "$cache" remote set-url origin "$MYCELIUM_REPO" >&2
        run git -C "$cache" fetch --depth 1 origin >&2
        run git -C "$cache" reset --hard FETCH_HEAD >&2
    else
        run git clone --depth 1 "$MYCELIUM_REPO" "$cache" >&2
    fi
    printf '%s' "$cache"
}

sha256_file() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

# A checkout can build; a piped installer has no source tree, and the release
# archive is the only path that does not need a compiler there.
in_checkout() {
    [ -n "$SCRIPT_PATH" ] || return 1
    [ -f "$SCRIPT_DIR/Cargo.toml" ] && [ -d "$SCRIPT_DIR/crates/mycelium-cli" ]
}

# Release targets to try, most portable first. Releases may carry musl or gnu
# Linux archives; pick whichever the metadata actually offers.
release_targets() {
    case "$(uname -s)/$(uname -m)" in
        Darwin/arm64) printf 'aarch64-apple-darwin\n' ;;
        Darwin/x86_64) printf 'x86_64-apple-darwin\n' ;;
        Linux/x86_64) printf 'x86_64-unknown-linux-musl\nx86_64-unknown-linux-gnu\n' ;;
        Linux/aarch64|Linux/arm64) printf 'aarch64-unknown-linux-musl\naarch64-unknown-linux-gnu\n' ;;
        *) return 1 ;;
    esac
}

# Download a published release and verify it against the checksum in its
# release-metadata.json and SHA256SUMS before any binary is installed.
fetch_release() {
    command -v curl >/dev/null 2>&1 || die "curl is required to download a release"
    [ -n "$RELEASE_VERSION" ] || die "--release needs a tag"
    targets=$(release_targets) || die "no prebuilt Mycelium release for $(uname -s)/$(uname -m)"
    base=${RELEASE_BASE%/}
    if [ -z "$base" ]; then
        case "$RELEASE_VERSION" in
            latest) base="https://github.com/$RELEASE_REPO/releases/latest/download" ;;
            v*) base="https://github.com/$RELEASE_REPO/releases/download/$RELEASE_VERSION" ;;
            *) base="https://github.com/$RELEASE_REPO/releases/download/v$RELEASE_VERSION" ;;
        esac
    fi
    tmp=$(mktemp -d "${TMPDIR:-/tmp}/mycelium-release.XXXXXX")
    RELEASE_DIR=$(mktemp -d "${TMPDIR:-/tmp}/mycelium-release.XXXXXX")
    meta=$tmp/release-metadata.json
    curl -fsSL "$base/release-metadata.json" -o "$meta" \
        || die "no release metadata at $base"
    # The metadata is JSON, and a published release is free to re-indent it:
    # v0.1.3 is one field per line while scripts/release.sh writes one target
    # object per line. Parse the flattened text and match whole target objects,
    # so both layouts and a reformat in between keep working.
    flat=$(tr '\n' ' ' <"$meta" | tr -s ' ')
    version=$(sed -n 's/.*"version": *"\([^"]*\)".*/\1/p' <<<"$flat" | head -1)
    # The channel the release was published on. The update policy has to track
    # it: a policy left on the default `canary` never sees a `stable` release,
    # so a member who installed one never updates.
    RELEASE_CHANNEL=$(sed -n 's/.*"channel": *"\([^"]*\)".*/\1/p' <<<"$flat" | head -1)
    line=
    for target in $targets; do
        line=$(grep -o '{[^{}]*}' <<<"$flat" | grep -F "\"target\": \"$target\"" | head -1)
        [ -z "$line" ] || break
    done
    archive=$(printf '%s' "$line" | sed -n 's/.*"archive": "\([^"]*\)".*/\1/p')
    expected=$(printf '%s' "$line" | sed -n 's/.*"sha256": "\([^"]*\)".*/\1/p')
    [ -n "$version" ] && [ -n "$archive" ] && [ -n "$expected" ] \
        || die "release metadata has no archive for $(uname -s)/$(uname -m)"
    say "Downloading Mycelium $version for $target"
    curl -fsSL "$base/$archive" -o "$tmp/$archive" \
        || die "could not download $archive"
    curl -fsSL "$base/SHA256SUMS" -o "$tmp/SHA256SUMS" \
        || die "release is missing SHA256SUMS"
    sums=$(awk -v name="$archive" '$2 == name { print $1 }' "$tmp/SHA256SUMS" | head -1)
    [ -n "$sums" ] || die "SHA256SUMS has no entry for $archive"
    [ "$sums" = "$expected" ] \
        || die "release metadata and SHA256SUMS disagree for $archive"
    actual=$(sha256_file "$tmp/$archive")
    [ "$actual" = "$expected" ] \
        || die "checksum mismatch for $archive (expected $expected, got $actual)"
    tar -C "$RELEASE_DIR" -xzf "$tmp/$archive" || die "could not unpack $archive"
    rm -rf "$tmp"
    BIN_SRC=$RELEASE_DIR
    note "verified $archive (sha256 ${expected:0:12})"
}

install_mycelium() {
    if [ "$DRY_RUN" = 1 ]; then
        if [ -n "$RELEASE_VERSION" ]; then
            note "would download Mycelium release $RELEASE_VERSION for $(uname -s)/$(uname -m)"
        elif [ -n "$BIN_SRC" ]; then
            note "would install prebuilt binaries from $BIN_SRC"
        else
            note "would build Mycelium from ${SRC:-the checkout or $MYCELIUM_REPO}"
        fi
        note "would install to $BIN_DIR and $LOCAL_BIN"
        return 0
    fi
    if [ -n "$RELEASE_VERSION" ]; then
        fetch_release
    elif [ -n "$BIN_SRC" ]; then
        [ -d "$BIN_SRC" ] || die "prebuilt directory does not exist: $BIN_SRC"
        note "using prebuilt binaries from $BIN_SRC"
    else
        ensure_rust
        src=$(mycelium_source)
        [ -d "$src/crates/mycelium-cli" ] || die "not a Mycelium checkout: $src"
        say "Building Mycelium (first build can take a while)"
        if ! ( cd "$src" && run cargo build --release --locked \
            -p mycelium-cli --features iroh-sync ); then
            die "cargo build failed"
        fi
        if ! ( cd "$src" && run cargo build --release --locked \
            -p myceliumd -p mycelium-frontdoor -p mycelium-driver-dnsmasq -p geoded ); then
            die "cargo build failed"
        fi
        BIN_SRC="$src/target/release"
    fi
    mkdir -p "$BIN_DIR" "$LOCAL_BIN"
    for name in mycelium myceliumd mycelium-frontdoor mycelium-ingress mycelium-dns-resolver geoded; do
        if [ -f "$BIN_SRC/$name" ]; then
            install_atomic "$BIN_SRC/$name" "$BIN_DIR/$name"
        else
            warn "prebuilt $name missing; that service will be unavailable"
        fi
    done
    for name in mycelium myceliumd mycelium-frontdoor mycelium-ingress geoded; do
        if [ -f "$BIN_SRC/$name" ]; then install_atomic "$BIN_SRC/$name" "$LOCAL_BIN/$name"; fi
    done
    if [ -f "$BIN_SRC/mycelium-dns-resolver" ] && [ ! -L "$LOCAL_BIN/mycelium-dns-resolver" ]; then
        install_atomic "$BIN_SRC/mycelium-dns-resolver" "$LOCAL_BIN/mycelium-dns-resolver"
    fi
    if [ -n "$RELEASE_DIR" ]; then
        rm -rf "$RELEASE_DIR"
        RELEASE_DIR=
    fi
}

managed_peer_service() {
    if [ "$(uname -s)" = Darwin ]; then
        out=$(launchctl list dev.fpl.mycelium 2>/dev/null) || return 1
        pid=${out%% *}
        [ -n "$pid" ] && [ "$pid" != "-" ]
    elif command -v systemctl >/dev/null 2>&1; then
        systemctl --user is-active --quiet mycelium.service
    else
        return 1
    fi
}

wait_for_unmanaged_daemon_stop() {
    i=0
    while [ "$i" -lt 50 ]; do
        if ! env MYCELIUM_HOME="$MYCELIUM_HOME" MYCELIUM_NO_AUTOSTART=1 "$MYC" daemon status 2>/dev/null | grep -q ' pid '; then
            return 0
        fi
        i=$((i + 1))
        sleep 0.1
    done
    return 1
}

repair_peer_service() {
    [ "$START" = 1 ] || return 0
    if managed_peer_service && env MYCELIUM_HOME="$MYCELIUM_HOME" MYCELIUM_NO_AUTOSTART=1 "$MYC" daemon status 2>/dev/null | grep -q ' pid '; then
        return 0
    fi
    say "Repairing the managed mycelium daemon service"
    if [ "$(uname -s)" = Darwin ]; then
        plist="$HOME/Library/LaunchAgents/dev.fpl.mycelium.plist"
        [ -f "$plist" ] || { warn "no LaunchAgent at $plist"; return 1; }
        run launchctl bootout "gui/$(id -u)/dev.fpl.mycelium" || true
    elif [ -f "$HOME/.config/systemd/user/mycelium.service" ]; then
        run systemctl --user stop mycelium.service || true
    else
        warn "no mycelium systemd user unit"
        return 1
    fi
    run env MYCELIUM_HOME="$MYCELIUM_HOME" MYCELIUM_NO_AUTOSTART=1 "$MYC" daemon stop || true
    if ! wait_for_unmanaged_daemon_stop; then
        warn "an unmanaged myceliumd still owns the socket; stop it before re-running"
        return 1
    fi
    if [ "$(uname -s)" = Darwin ]; then
        run launchctl bootstrap "gui/$(id -u)" "$plist"
    else
        run systemctl --user daemon-reload
        run systemctl --user enable --now mycelium.service
    fi
    i=0
    while [ "$i" -lt 30 ]; do
        if managed_peer_service && env MYCELIUM_HOME="$MYCELIUM_HOME" MYCELIUM_NO_AUTOSTART=1 "$MYC" daemon status 2>/dev/null | grep -q ' pid '; then
            return 0
        fi
        i=$((i + 1))
        sleep 1
    done
    warn "managed mycelium service did not become ready; see $MYCELIUM_HOME/daemon.log"
    return 1
}

setup_node() {
    MYC="$LOCAL_BIN/mycelium"
    if [ "$DRY_RUN" = 1 ]; then
        if [ "$ENROLL" = no ]; then
            note "would skip enrollment and repair the daemon service"
        else
            note "would enroll/repair the node with $MYC setup"
        fi
        return 0
    fi
    [ -x "$MYC" ] || die "mycelium was not installed"
    if [ "$ENROLL" = no ]; then
        note "skipping enrollment (--no-enroll)"
        if [ "$START" = 1 ]; then
            if [ -f "$MYCELIUM_HOME/pki/node.pem" ] || [ -f "$MYCELIUM_HOME/node.env" ]; then
                set -- setup --repair --path "$MYCELIUM_HOME"
                if [ "$LOCAL_FRONTDOOR" = 1 ]; then set -- "$@" --local-frontdoor; fi
                run "$MYC" "$@"
            else
                run env MYCELIUM_HOME="$MYCELIUM_HOME" MYCELIUM_NO_AUTOSTART=1 "$MYC" daemon stop || true
                run env MYCELIUM_HOME="$MYCELIUM_HOME" "$MYC" daemon start
                note "no enrollment found; started the daemon without a managed service"
                return 0
            fi
        fi
    elif [ -f "$MYCELIUM_HOME/pki/node.pem" ] || [ -f "$MYCELIUM_HOME/node.env" ]; then
        say "Existing enrollment found; repairing the service definition"
        set -- setup --repair --path "$MYCELIUM_HOME"
        if [ "$LOCAL_FRONTDOOR" = 1 ]; then set -- "$@" --local-frontdoor; fi
        run "$MYC" "$@"
    else
        say "Starting enrollment"
        set -- setup --gateway "$GATEWAY"
        if [ "$LOCAL_FRONTDOOR" = 1 ]; then set -- "$@" --local-frontdoor; fi
        if [ -n "$CLAIM" ]; then
            set -- "$@" --claim "$CLAIM"
        elif [ -n "$CLAIM_FILE" ]; then
            set -- "$@" --claim-file "$CLAIM_FILE"
        fi
        run "$MYC" "$@"
    fi
    repair_peer_service
}

retire_legacy_dns_agents() {
    if [ "$DRY_RUN" = 1 ]; then
        note "would retire legacy mycelium-dns-resolver user agents"
        return 0
    fi
    if [ "$(uname -s)" = Darwin ]; then
        for label in dev.fpl.mycelium-dns dev.fpl.mycelium-dns-client dev.fpl.mycelium-doh; do
            plist="$HOME/Library/LaunchAgents/$label.plist"
            [ -f "$plist" ] || continue
            if grep -q 'mycelium-dns-resolver' "$plist" 2>/dev/null; then
                launchctl bootout "gui/$(id -u)/$label" >/dev/null 2>&1 || true
                mv "$plist" "$plist.pre-$TS"
                note "retired legacy DNS agent $label"
            fi
        done
    else
        for unit in mycelium-dns mycelium-dns-client mycelium-doh; do
            path="$HOME/.config/systemd/user/$unit.service"
            [ -f "$path" ] || continue
            if grep -q 'mycelium-dns-resolver' "$path" 2>/dev/null; then
                systemctl --user disable --now "$unit.service" >/dev/null 2>&1 || true
                mv "$path" "$path.pre-$TS"
                note "retired legacy DNS service $unit"
            fi
        done
    fi
}

install_dns() {
    cfg="$MYCELIUM_HOME/dns/active-settings.json"
    MYC="$LOCAL_BIN/mycelium"
    if [ "$DRY_RUN" = 1 ]; then
        note "would apply DNS resolver settings and install the system service"
        return 0
    fi
    retire_legacy_dns_agents
    if [ ! -f "$cfg" ]; then
        if [ -z "$REACHABLE" ]; then
            warn "no DNS resolver config at $cfg; skipping the DNS service"
            warn "pass --reachable CIDR (repeatable) or run: mycelium dns resolver init --config $cfg --site ${SITE:-auto} --suffix $SUFFIX ..."
            return 1
        fi
        say "Creating the DNS resolver settings"
        set -- init --config "$cfg" --site "${SITE:-auto}" --suffix "$SUFFIX"
        set -- "$@" --mycelium "$BIN_DIR/mycelium"
        for cidr in $REACHABLE; do set -- "$@" --reachable "$cidr"; done
        set -- "$@" --write
        run "$MYC" dns resolver "$@"
    fi
    run "$MYC" dns resolver apply --config "$cfg" --write
    if [ "$(uname -s)" = Linux ] \
        && ! systemctl is-active --quiet systemd-resolved 2>/dev/null; then
        warn "systemd-resolved is not active; DNS activation will use the root stub and rewrite /etc/resolv.conf (the previous file is backed up)"
    fi
    need_root "install the DNS resolver service"
    ensure_root_dir "$ROOT_LIBEXEC"
    run_sudo install -m 755 "$BIN_DIR/mycelium" "$ROOT_LIBEXEC/mycelium"
    root_own "$ROOT_LIBEXEC/mycelium"
    run_sudo chmod 755 "$ROOT_LIBEXEC/mycelium"
    run_sudo env MYCELIUM_HOME="$MYCELIUM_HOME" MYCELIUM_NO_AUTOSTART=1 \
        "$ROOT_LIBEXEC/mycelium" dns resolver system-install --config "$cfg" --write
}

install_frontdoor() {
    MYC="$LOCAL_BIN/mycelium"
    if [ "$DRY_RUN" = 1 ]; then
        note "would install the loopback frontdoor and its ingress services"
        return 0
    fi
    [ -x "$MYC" ] || die "mycelium was not installed"
    # `frontdoor install` writes the frontdoor plus the ingress HTTPS/HTTP
    # service definitions and loads them; the ingress is what publishes the
    # .myc names, so it is not optional once this component runs.
    if [ "$TRUST_CERT" = yes ] \
        || { [ "$TRUST_CERT" = ask ] && ask_yes "Trust the generated frontdoor CA in your login keychain?" yes; }; then
        run "$MYC" frontdoor install --write --trust-cert
    else
        run "$MYC" frontdoor install --write
    fi
}

install_operator() {
    MYC="$LOCAL_BIN/mycelium"
    if [ "$DRY_RUN" = 1 ]; then
        note "would install agent skills, shell completions, and the update policy"
        return 0
    fi
    if ask_yes "Install Mycelium agent skills?" yes; then
        target=""
        if [ -d "$HOME/.agents" ]; then
            target=agents
        elif [ -d "$HOME/.claude" ]; then
            target=claude
        elif [ -d "$HOME/.codex" ]; then
            target=codex
        fi
        if [ -n "$target" ]; then
            run "$MYC" skills install --target "$target" --write
        else
            note "no agent skill directory found; skipping skills"
        fi
    fi
    if ask_yes "Add Mycelium shell completion to your shell rc?" yes; then
        case "${SHELL:-}" in
            */zsh) rc="$HOME/.zshrc"; shell_name=zsh ;;
            */bash) rc="$HOME/.bashrc"; shell_name=bash ;;
            *) rc=""; shell_name="" ;;
        esac
        if [ -n "$rc" ]; then
            ensure_shell_block "$rc" "$shell_name"
        else
            note "unrecognized shell; add completion manually"
        fi
    fi
    if [ "$ENABLE_UPDATES" != yes ]; then
        note "automatic updates are disabled; re-enable when a signed release is newer"
    fi
}

ensure_shell_block() {
    rc=$1
    shell_name=$2
    start="# >>> mycelium installer >>>"
    finish="# <<< mycelium installer <<<"
    if [ -f "$rc" ]; then cp -p "$rc" "$rc.pre-$TS"; else touch "$rc"; fi
    if grep -qF "$start" "$rc" 2>/dev/null; then
        note "$rc already has the Mycelium block"
        return 0
    fi
    {
        printf '\n%s\n' "$start"
        printf '%s\n' 'case ":$PATH:" in *":$HOME/.local/bin:"*) ;; *) export PATH="$HOME/.local/bin:$PATH" ;; esac'
        printf 'if command -v mycelium >/dev/null 2>&1; then source <(mycelium completions %s); fi\n' "$shell_name"
        printf '%s\n' "$finish"
    } >>"$rc"
    note "updated $rc"
}

detect_host_label() {
    raw=$(scutil --get LocalHostName 2>/dev/null || hostname -s 2>/dev/null || hostname 2>/dev/null || echo node)
    printf '%s' "$raw" | tr '[:upper:]' '[:lower:]' | sed 's/[^a-z0-9-]/-/g; s/^-*//; s/-*$//'
}

detect_overlay() {
    # The overlay address derives from the node identity, so it is knowable
    # before any interface exists. Scraping the published zone used to return
    # whatever mesh happened to be installed -- on this fleet, Tailscale's
    # address -- and the tunnel then tried to own an address it could not have.
    "$LOCAL_BIN/mycelium" iroh-tun address 2>/dev/null || true
}

detect_tun() {
    if [ "${1:-}" = Darwin ]; then
        for n in 220 221 222 223 224 225 226 227 228 229 230; do
            if ! ifconfig "utun$n" >/dev/null 2>&1; then printf 'utun%s' "$n"; return; fi
        done
        printf 'utun220'
    else
        # One name on every host: the daemon publishes whatever address the
        # kernel holds on `mycelium0`, so a per-host `tunN` would never be
        # published and `.myc` would keep pointing somewhere else.
        printf 'mycelium0'
    fi
}

build_iroh_tun() {
    if [ -x "$BIN_DIR/iroh-tun" ] && [ "$FORCE" != 1 ]; then
        existing_help=$("$BIN_DIR/iroh-tun" --help 2>&1 || true)
        if grep -q -- '--status-file' <<<"$existing_help" \
            && grep -q -- '--mycelium' <<<"$existing_help"; then
            note "using existing $BIN_DIR/iroh-tun"
            install_atomic "$BIN_DIR/iroh-tun" "$LOCAL_BIN/iroh-tun"
            return 0
        fi
        note "existing iroh-tun is missing the supervisor flags; rebuilding"
    fi
    if [ -f "$BIN_SRC/iroh-tun" ]; then
        install_atomic "$BIN_SRC/iroh-tun" "$BIN_DIR/iroh-tun"
        install_atomic "$BIN_SRC/iroh-tun" "$LOCAL_BIN/iroh-tun"
        return 0
    fi
    ensure_rust
    if [ -n "${IROH_TUN_SRC:-}" ]; then
        src=$IROH_TUN_SRC
    else
        # Outside `$HOME` on purpose: cargo walks up from a manifest until it
        # finds a workspace root, so one stray `Cargo.toml` above the checkout -
        # a home-directory crate, say - makes the build refuse instead of
        # building.
        src="${TMPDIR:-/tmp}/mycelium-iroh-tun-src"
        mkdir_p "$(dirname "$src")"
        if [ -d "$src/.git" ]; then
            run git -C "$src" fetch --depth 1 origin
            run git -C "$src" reset --hard FETCH_HEAD
        else
            run git clone --depth 1 "$IROH_TUN_REPO" "$src"
        fi
    fi
    say "Building iroh-tun"
    ( cd "$src" && run cargo build --release --locked )
    install_atomic "$src/target/release/iroh-tun" "$BIN_DIR/iroh-tun"
    install_atomic "$src/target/release/iroh-tun" "$LOCAL_BIN/iroh-tun"
}

xml_escape() {
    printf '%s' "$1" | sed 's/&/\&amp;/g; s/</\&lt;/g; s/>/\&gt;/g; s/"/\&quot;/g'
}

install_launch_agent() {
    label=$1
    program=$2
    log_path=$3
    shift 3
    plist="$HOME/Library/LaunchAgents/$label.plist"
    if [ "$DRY_RUN" = 1 ]; then note "would write $plist"; return 0; fi
    mkdir -p "$HOME/Library/LaunchAgents" "$(dirname "$log_path")"
    {
        printf '%s\n' '<?xml version="1.0" encoding="UTF-8"?>'
        printf '%s\n' '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">'
        printf '%s\n' '<plist version="1.0"><dict>'
        printf '<key>Label</key><string>%s</string>\n' "$(xml_escape "$label")"
        printf '%s\n' '<key>ProgramArguments</key><array>'
        printf '<string>%s</string>\n' "$(xml_escape "$program")"
        for arg in "$@"; do printf '<string>%s</string>\n' "$(xml_escape "$arg")"; done
        printf '%s\n' '</array>'
        printf '%s\n' '<key>RunAtLoad</key><true/><key>KeepAlive</key><true/>'
        printf '<key>ThrottleInterval</key><integer>10</integer>\n'
        printf '<key>StandardOutPath</key><string>%s</string>\n' "$(xml_escape "$log_path")"
        printf '<key>StandardErrorPath</key><string>%s</string>\n' "$(xml_escape "$log_path")"
        printf '%s\n' '</dict></plist>'
    } >"$plist"
    launchctl bootout "gui/$(id -u)/$label" >/dev/null 2>&1 || true
    if [ "$START" = 1 ]; then
        launchctl bootstrap "gui/$(id -u)" "$plist"
    fi
}

write_root_file() {
    dest=$1
    mode=$2
    if [ "$DRY_RUN" = 1 ]; then
        note "would write $dest"
        cat >/dev/null
        return 0
    fi
    run_sudo tee "$dest" >/dev/null
    root_own "$dest"
    run_sudo chmod "$mode" "$dest"
}

install_vpn() {
    if [ "$DRY_RUN" = 1 ]; then
        note "would build/install iroh-tun and a root-owned tunnel service"
        return 0
    fi
    build_iroh_tun
    [ -x "$BIN_DIR/iroh-tun" ] || die "iroh-tun is unavailable"
    [ -x "$LOCAL_BIN/mycelium" ] || die "mycelium is unavailable"

    overlay=$OVERLAY
    if [ -z "$overlay" ]; then
        overlay=$(detect_overlay)
    fi
    if [ -z "$overlay" ]; then
        overlay=$(ask_text "IPv6 overlay CIDR for this node (blank to skip the VPN)" "")
    fi
    if [ -z "$overlay" ]; then
        warn "no IPv6 overlay selected; skipping the VPN"
        return 1
    fi
    case "$overlay" in
        *:*) ;;
        *) die "overlay must be an IPv6 CIDR, got $overlay" ;;
    esac
    case "$overlay" in */*) ;; *) overlay="$overlay/128" ;; esac

    tun=$TUN
    [ -n "$tun" ] || tun=$(detect_tun "$(uname -s)")

    key="$MYCELIUM_HOME/iroh-tun.key"
    if [ -L "$key" ]; then die "refusing a symlinked tunnel key: $key"; fi
    if [ ! -f "$key" ]; then
        say "Creating the tunnel identity"
        ( umask 077; openssl rand -hex 32 >"$key" )
    fi
    chmod 600 "$key"

    status_dir=${STATUS_PATH:+$(dirname "$STATUS_PATH")}
    if [ -z "$status_dir" ]; then
        if [ "$(uname -s)" = Darwin ]; then status_dir=/var/run/mycelium; else status_dir=/run/mycelium; fi
    fi
    [ -n "$STATUS_PATH" ] || STATUS_PATH="$status_dir/iroh-tun-status.json"
    case "$STATUS_PATH" in
        /*) ;;
        *) die "--status must be an absolute path" ;;
    esac
    if ! "$LOCAL_BIN/mycelium" iroh-tun config --tun "$tun" --overlay "$overlay" \
        --mtu 1280 --external true --enabled true --status "$STATUS_PATH" >/dev/null; then
        die "installed mycelium rejected the external iroh-tun config; use a checkout with the unified-install change (--source <dir>)"
    fi

    need_root "install the root-owned iroh-tun service"
    ensure_root_dir "$ROOT_LIBEXEC"
    ensure_root_dir "$status_dir"
    if [ -L "$STATUS_PATH" ]; then die "refusing symlinked status file: $STATUS_PATH"; fi
    if [ -e "$STATUS_PATH" ] && [ ! -f "$STATUS_PATH" ]; then
        die "status path is not a regular file: $STATUS_PATH"
    fi
    run_sudo install -m 755 "$BIN_DIR/mycelium" "$ROOT_LIBEXEC/mycelium"
    run_sudo install -m 755 "$BIN_DIR/iroh-tun" "$ROOT_LIBEXEC/iroh-tun"
    root_own "$ROOT_LIBEXEC/mycelium" "$ROOT_LIBEXEC/iroh-tun"
    run_sudo chmod 755 "$ROOT_LIBEXEC/mycelium" "$ROOT_LIBEXEC/iroh-tun"
    if [ -f "$STATUS_PATH" ]; then
        root_own "$STATUS_PATH"
        run_sudo chmod 644 "$STATUS_PATH"
    fi

    run_sudo tee "$ROOT_LIBEXEC/iroh-tun-root" >/dev/null <<EOF
#!/bin/sh
set -eu
export MYCELIUM_HOME='$MYCELIUM_HOME'
export MYCELIUM_NO_AUTOSTART=1
export PATH=/usr/bin:/bin:/usr/sbin:/sbin
'$ROOT_LIBEXEC/iroh-tun' \\
  --tun '$tun' --mtu 1280 --refresh-secs 15 \\
  --secret "\$(cat '$key')" \\
  --status-file '$STATUS_PATH' \\
  --mycelium '$ROOT_LIBEXEC/mycelium' &
child=\$!
# The tunnel owns the device; the address is this node's identity, so give it
# to the interface as soon as it exists, then stay in the foreground so the
# service manager still sees one process tree.
if [ '$(uname -s)' = Linux ]; then
  ipbin=/usr/sbin/ip
  [ -x "\$ipbin" ] || ipbin=/sbin/ip
  assigned=0
  i=0
  while [ \$i -lt 150 ]; do
    if "\$ipbin" link show '$tun' >/dev/null 2>&1 \\
      && "\$ipbin" -6 addr replace '$overlay' dev '$tun' >/dev/null 2>&1; then
      assigned=1
      break
    fi
    i=\$((i+1)); sleep 0.1
  done
  if [ \$assigned -ne 1 ]; then
    # Running without the identity address would publish nothing and the zone
    # would quietly lose this node, so fail and let the service manager retry.
    echo "iroh-tun: could not assign $overlay to $tun" >&2
    kill \$child 2>/dev/null || true
    exit 1
  fi
fi
wait \$child
EOF
    root_own "$ROOT_LIBEXEC/iroh-tun-root"
    run_sudo chmod 755 "$ROOT_LIBEXEC/iroh-tun-root"

    if [ "$(uname -s)" = Darwin ]; then
        plist=/Library/LaunchDaemons/dev.fpl.iroh-tun.plist
        write_root_file "$plist" 644 <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>dev.fpl.iroh-tun</string>
<key>ProgramArguments</key><array><string>$(xml_escape "$ROOT_LIBEXEC/iroh-tun-root")</string></array>
<key>RunAtLoad</key><true/><key>KeepAlive</key><true/>
<key>ThrottleInterval</key><integer>10</integer>
<key>StandardOutPath</key><string>/var/log/mycelium-iroh-tun.log</string>
<key>StandardErrorPath</key><string>/var/log/mycelium-iroh-tun.log</string>
</dict></plist>
EOF
        run_sudo launchctl bootout system/dev.fpl.iroh-tun >/dev/null 2>&1 || true
        if [ "$START" = 1 ]; then
            run_sudo launchctl bootstrap system "$plist"
            run_sudo launchctl kickstart -k system/dev.fpl.iroh-tun
        fi
    else
        unit=/etc/systemd/system/mycelium-iroh-tun.service
        write_root_file "$unit" 644 <<EOF
[Unit]
Description=Mycelium iroh-tun IPv6 overlay
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=$(systemd_quote "$ROOT_LIBEXEC/iroh-tun-root")
Restart=always
RestartSec=10
User=root
Group=root
RuntimeDirectory=mycelium
RuntimeDirectoryMode=0755

[Install]
WantedBy=multi-user.target
EOF
        run_sudo systemctl daemon-reload
        if [ "$START" = 1 ]; then run_sudo systemctl enable --now mycelium-iroh-tun; fi
    fi

    say "Configuring the IPv6 tunnel"
    run "$LOCAL_BIN/mycelium" iroh-tun config --tun "$tun" --overlay "$overlay" \
        --mtu 1280 --external true --enabled true --status "$STATUS_PATH" --write

    if [ "$START" = 1 ]; then
        say "Waiting for the privileged tunnel to report"
        i=0
        publishing=0
        while [ "$i" -lt 30 ]; do
            if "$LOCAL_BIN/mycelium" iroh-tun status --json 2>/dev/null \
                | grep -Eq '"publishing"[[:space:]]*:[[:space:]]*true'; then
                publishing=1
                break
            fi
            i=$((i + 1))
            sleep 1
        done
        if [ "$publishing" != 1 ]; then
            warn "the privileged tunnel is not publishing yet"
            [ -f /var/log/mycelium-iroh-tun.log ] && tail -5 /var/log/mycelium-iroh-tun.log || true
            return 1
        fi
    fi
    note "iroh-tun is configured on $tun with $overlay"
}

install_geode() {
    if [ "$DRY_RUN" = 1 ]; then
        note "would install geoded and its user service"
        return 0
    fi
    [ -x "$LOCAL_BIN/geoded" ] || { warn "geoded is unavailable; rebuild with the core component"; return 1; }
    mkdir_p "$HOME/.geode"
    chmod 700 "$HOME/.geode"
    if [ "$(uname -s)" = Darwin ]; then
        install_launch_agent dev.fpl.geoded "$LOCAL_BIN/geoded" "$HOME/.geode/geoded.log" \
            --socket "$HOME/.geode/geode.sock" \
            --store "$HOME/.geode/store.db" \
            --key-file "$HOME/.geode/store.key" \
            --init
    else
        install_user_unit dev.fpl.geoded "$LOCAL_BIN/geoded" \
            --socket "$HOME/.geode/geode.sock" \
            --store "$HOME/.geode/store.db" \
            --key-file "$HOME/.geode/store.key" \
            --init
    fi
    if [ "$START" = 0 ]; then
        note "wrote the geode service; not started (--no-start)"
        return 0
    fi
    i=0
    while [ "$i" -lt 50 ]; do
        [ -S "$HOME/.geode/geode.sock" ] && return 0
        i=$((i + 1))
        sleep 0.1
    done
    warn "geoded did not create $HOME/.geode/geode.sock; check $HOME/.geode/geoded.log"
    return 1
}

install_vfs() {
    if [ "$DRY_RUN" = 1 ]; then
        note "would build/install fungos-vfs and its user services (role $VFS_ROLE)"
        return 0
    fi
    vfs_bin=""
    if [ -x "$BIN_SRC/fungos-vfs" ]; then
        vfs_bin=$BIN_SRC
    else
        ensure_rust
        src=${VFS_SRC:-${XDG_CACHE_HOME:-$HOME/.cache}/mycelium/install-src/fungos-vfs}
        mkdir_p "$(dirname "$src")"
        if [ -d "$src/.git" ]; then
            run git -C "$src" fetch --depth 1 origin
            run git -C "$src" reset --hard FETCH_HEAD
        else
            run git clone --depth 1 "$VFS_REPO" "$src"
        fi
        say "Building fungos-vfs"
        ( cd "$src" && run cargo build --release --locked -p fungos-vfs -p fungos-vfs-unibus )
        if { [ "$(uname -s)" = Darwin ] && [ -d /Library/Filesystems/macfuse.fs ]; } \
            || { [ "$(uname -s)" = Linux ] && [ -e /dev/fuse ]; }; then
            ( cd "$src" && run cargo build --release --locked -p fungos-vfs-fuse ) || true
        fi
        vfs_bin="$src/target/release"
    fi
    for name in fungos-vfs fungos-vfs-provider fungos-vfs-fetch fungos-vfs-fuse; do
        if [ -f "$vfs_bin/$name" ]; then install_atomic "$vfs_bin/$name" "$LOCAL_BIN/$name"; fi
    done
    mkdir_p "$VFS_ROOT" "$VFS_MOUNT" "$HOME/.local/share/fungos/vfs"
    if [ "$(uname -s)" = Darwin ]; then
        if [ ! -d /Library/Filesystems/macfuse.fs ]; then
            warn "macFUSE is not installed; the VFS CLI is installed but the mount is skipped"
        elif [ -x "$LOCAL_BIN/fungos-vfs-fuse" ]; then
            install_launch_agent dev.fungos.vfs-fuse "$LOCAL_BIN/fungos-vfs-fuse" "$HOME/Library/Logs/fungOS/fuse.log" \
                --root "$VFS_ROOT" --mount "$VFS_MOUNT" --role "$VFS_ROLE"
        else
            warn "fungos-vfs-fuse is unavailable; the mount service is skipped"
        fi
        if [ -n "$VFS_CONFIG" ] && [ -f "$VFS_CONFIG" ] && [ -x "$LOCAL_BIN/fungos-vfs-provider" ]; then
            install_launch_agent dev.fungos.vfs-content "$LOCAL_BIN/fungos-vfs-provider" "$HOME/Library/Logs/fungOS/content.log" \
                --root "$VFS_ROOT" --config "$VFS_CONFIG"
        elif [ -z "$VFS_CONFIG" ]; then
            note "set --vfs-config to a Unibus client config to run the content provider"
        else
            warn "fungos-vfs-provider is unavailable; the content service is skipped"
        fi
    else
        if [ ! -e /dev/fuse ]; then
            warn "/dev/fuse is unavailable; the VFS CLI is installed but the mount is skipped"
        elif [ -x "$LOCAL_BIN/fungos-vfs-fuse" ]; then
            install_user_unit fungos-vfs-fuse "$LOCAL_BIN/fungos-vfs-fuse" \
                --root "$VFS_ROOT" --mount "$VFS_MOUNT" --role "$VFS_ROLE"
        else
            warn "fungos-vfs-fuse is unavailable; the mount service is skipped"
        fi
        if [ -n "$VFS_CONFIG" ] && [ -f "$VFS_CONFIG" ] && [ -x "$LOCAL_BIN/fungos-vfs-provider" ]; then
            install_user_unit fungos-vfs-content "$LOCAL_BIN/fungos-vfs-provider" \
                --root "$VFS_ROOT" --config "$VFS_CONFIG"
        elif [ -z "$VFS_CONFIG" ]; then
            note "set --vfs-config to a Unibus client config to run the content provider"
        elif [ -n "$VFS_CONFIG" ]; then
            warn "fungos-vfs-provider is unavailable; the content service is skipped"
        fi
    fi
}

systemd_quote() {
    printf '"%s"' "$(printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g; s/%/%%/g')"
}

install_user_unit() {
    name=$1
    program=$2
    shift 2
    unit="$HOME/.config/systemd/user/$name.service"
    mkdir -p "$HOME/.config/systemd/user"
    {
        printf '[Unit]\nDescription=%s\nAfter=default.target\n\n' "$name"
        printf '[Service]\nExecStart='
        systemd_quote "$program"
        for arg in "$@"; do
            printf ' '
            systemd_quote "$arg"
        done
        printf '\nRestart=always\nRestartSec=10\n\n[Install]\nWantedBy=default.target\n'
    } >"$unit"
    if ! command -v systemctl >/dev/null 2>&1; then
        warn "systemctl is unavailable; wrote $unit but did not enable it"
        return 0
    fi
    systemctl --user daemon-reload
    if [ "$START" = 1 ]; then
        systemctl --user enable "$name.service"
        systemctl --user restart "$name.service"
    fi
}

build_soil_cli() {
    if [ -x "$BIN_SRC/soil" ]; then
        install_atomic "$BIN_SRC/soil" "$LOCAL_BIN/soil"
        # Spores still come from a source tree; a prebuilt CLI does not carry
        # them. Prefer the checkout this machine already has.
        SOIL_SOURCE=${SOIL_SRC:-$HOME/Documents/src/fungOS-linux/soil}
        return
    fi
    ensure_rust || return 1
    src=${SOIL_SRC:-${XDG_CACHE_HOME:-$HOME/.cache}/mycelium/install-src/soil}
    SOIL_SOURCE=$src
    mkdir_p "$(dirname "$src")"
    if [ -d "$src/.git" ]; then
        run git -C "$src" fetch --depth 1 origin || return 1
        run git -C "$src" reset --hard FETCH_HEAD || return 1
    else
        run git clone --depth 1 "$SOIL_REPO" "$src" || return 1
    fi
    say "Building Soil"
    ( cd "$src" && run cargo build --release --locked -p soil-shell ) || return 1
    install_atomic "$src/target/release/soil" "$LOCAL_BIN/soil"
}

install_soil_spores() {
    root=$HOME/.local/share/fungos/spores
    src=$SOIL_SOURCE
    if [ -z "$src" ] || [ ! -d "$src" ]; then
        src=$HOME/Documents/src/fungOS-linux/soil
    fi
    if [ ! -d "$src" ]; then
        src=${XDG_CACHE_HOME:-$HOME/.cache}/mycelium/install-src/soil
    fi
    if [ -d "$src/.git" ] && [ ! -d "$src/spores-public/spores" ]; then
        if [ "$DRY_RUN" = 1 ]; then
            note "would initialize the Soil spores submodule"
        else
            run git -C "$src" submodule update --init --depth 1 spores-public \
                || warn "could not initialize the Soil spores submodule"
        fi
    fi
    # A standalone Spores checkout is newer than the submodule pinned in Soil;
    # prefer it when the caller points at one.
    SPORES_SRC=
    SPORES_ROOT=
    if [ -n "$SPORES_SOURCE" ]; then
        if [ -d "$SPORES_SOURCE/spores" ]; then
            SPORES_ROOT=$SPORES_SOURCE
            SPORES_SRC=$SPORES_SOURCE/spores
        elif [ -d "$SPORES_SOURCE/terminal" ]; then
            SPORES_SRC=$SPORES_SOURCE
            SPORES_ROOT=$(dirname "$SPORES_SOURCE")
        else
            warn "--spores-src $SPORES_SOURCE has neither spores/ nor terminal/"
        fi
    fi
    if [ -z "$SPORES_SRC" ]; then
        SPORES_ROOT=$src/spores-public
        SPORES_SRC=$SPORES_ROOT/spores
    fi
    if [ "$DRY_RUN" = 1 ]; then
        note "would install baseline Spores under $root"
        if [ "$(uname -s)" = Darwin ]; then
            note "would link $HOME/Library/Application Support/fungOS/spores to $root"
        fi
        install_terminal_host "$src"
        return 0
    fi
    mkdir_p "$root"
    if [ "$(uname -s)" = Darwin ]; then
        app_support="$HOME/Library/Application Support/fungOS"
        app_root=$app_support/spores
        mkdir_p "$app_support"
        if [ -L "$app_root" ]; then
            :
        elif [ -d "$app_root" ]; then
            # Preserve a catalog created by an older app, then make the
            # installer-managed store the single source of truth.
            for entry in "$app_root"/*; do
                [ -e "$entry" ] || continue
                cp -R "$entry" "$root/" || warn "could not merge $entry"
            done
            mv "$app_root" "$app_root.pre-$TS"
            ln -s "$root" "$app_root"
            note "linked app Spore catalog to $root (previous catalog moved aside)"
        else
            ln -s "$root" "$app_root"
            note "linked app Spore catalog to $root"
        fi
    fi
    spores_src=$SPORES_SRC
    if [ ! -d "$spores_src" ]; then
        warn "no Spore source tree at $spores_src; the launcher will stay empty until a Spore is installed"
        install_terminal_host
        return 0
    fi
    for package in "$spores_src"/*; do
        name=$(basename "$package")
        if [ ! -d "$package" ] || [ ! -f "$package/spore.toml" ] \
            || [ ! -f "$package/module.wasm" ]; then
            continue
        fi
        # The launcher is the app itself, not an app in its own catalogue.
        if [ "$name" = launcher ]; then
            continue
        fi
        # This app opens a Spore on a canvas surface or through the native
        # host the package names. A Spore that offers neither would only add
        # a row that fails.
        if ! spore_has_app_surface "$package/spore.toml"; then
            note "Spore $name has no canvas or native surface; skipped"
            continue
        fi
        if [ -f "$root/$name/spore.toml" ]; then
            note "Spore $name is already installed"
            continue
        fi
        # Copy the whole package, not a guessed asset list: the manifest names
        # what a Spore needs, and a new kind of asset must not silently vanish.
        mkdir_p "$root/$name"
        if cp -R "$package/." "$root/$name/"; then
            note "installed Spore $name"
            if [ "$name" = terminal ] && ! grep -q '^[[:space:]]*native[[:space:]]*=' "$root/$name/spore.toml"; then
                warn "the $package terminal package predates [launch] native; pass --spores-src to a newer Spores checkout"
            fi
        else
            warn "could not install Spore $name"
        fi
    done
    install_terminal_host "$src"
}

spore_has_app_surface() {
    if awk '
        /^\[launch\]/ { launch = 1; next }
        /^\[/ { launch = 0 }
        launch && /^[[:space:]]*surfaces[[:space:]]*=/ { print; exit }
    ' "$1" | grep -q '"canvas"'; then
        return 0
    fi
    # A native-only Spore is launchable only when it names the host that
    # presents it; otherwise the app has nothing to run.
    grep -q '^[[:space:]]*native[[:space:]]*=' "$1"
}

install_terminal_host() {
    host=$HOME/.local/share/fungos/bin/terminal-host
    # `terminal-host` is a Soil crate: it implements `soil:mycelium` and the
    # terminal surface, so it builds from the Soil checkout, not Spores.
    src=${1:-$SOIL_SOURCE}
    if [ "$DRY_RUN" = 1 ]; then
        note "would install terminal-host for the terminal Spore"
        return 0
    fi
    if [ -n "$TERMINAL_HOST_BIN" ]; then
        if [ -x "$TERMINAL_HOST_BIN" ]; then
            install_atomic "$TERMINAL_HOST_BIN" "$host"
            note "installed terminal-host from $TERMINAL_HOST_BIN"
        else
            warn "--terminal-host $TERMINAL_HOST_BIN is not executable"
        fi
        return 0
    fi
    if [ -x "$BIN_SRC/terminal-host" ]; then
        install_atomic "$BIN_SRC/terminal-host" "$host"
        note "installed terminal-host from $BIN_SRC"
        return 0
    fi
    if [ -z "$src" ] || [ ! -d "$src/crates/terminal-host" ]; then
        warn "terminal-host is not available; the terminal Spore will not open"
        return 0
    fi
    if ! ensure_rust; then
        warn "Rust is unavailable; terminal-host was not built"
        return 0
    fi
    say "Building terminal-host"
    if ! ( cd "$src" && run cargo build --release --locked -p terminal-host ); then
        warn "terminal-host build failed; the terminal Spore will not open"
        return 0
    fi
    install_atomic "$src/target/release/terminal-host" "$host"
    note "installed terminal-host"
}

install_soil() {
    if [ "$DRY_RUN" = 1 ]; then
        install_soil_spores
        if [ "$(uname -s)" = Darwin ]; then
            note "would build Soil, install fungOS.app, and set up the runtime"
        else
            note "would build Soil and set up the runtime (fungOS.app is macOS-only)"
        fi
        return 0
    fi
    # The CLI is useful but not a prerequisite for the app or runtime services.
    # Keep a failed soil-shell build from taking the whole component down.
    if build_soil_cli; then
        :
    else
        warn "soil CLI build failed; continuing with the runtime and fungOS.app"
    fi
    install_soil_spores
    mkdir_p "$HOME/.local/share/fungos/bin" "$HOME/.local/share/fungos/store" \
        "$HOME/.local/share/fungos/spores" "$HOME/.local/share/fungos/packages" \
        "$HOME/.local/share/fungos/media" "$HOME/.local/share/fungos/records" \
        "$HOME/.local/share/fungos/notify" "$HOME/.local/share/fungos/vfs"
    if [ -n "$SOIL_RUNTIME" ] && [ -d "$SOIL_RUNTIME" ]; then
        for name in soil-home soil-store soil-spore files-serve; do
            if [ -f "$SOIL_RUNTIME/$name" ]; then
                install_atomic "$SOIL_RUNTIME/$name" "$HOME/.local/share/fungos/bin/$name"
            fi
        done
    elif [ ! -x "$HOME/.local/share/fungos/bin/soil-store" ]; then
        warn "Soil runtime services are not present; pass --soil-runtime DIR to install them"
    fi

    if [ "$(uname -s)" = Darwin ]; then
        app_src=$APP_BUNDLE
        if [ -z "$app_src" ] && [ -d "$HOME/Applications/fungOS.app" ]; then
            app_src="$HOME/Applications/fungOS.app"
        fi
        if [ -n "$APP_URL" ] && [ ! -d "$app_src" ]; then
            case "$APP_URL" in
                *.zip)
                    tmp=$(mktemp "${TMPDIR:-/tmp}/fungos-app.XXXXXX.zip")
                    curl -fsSL "$APP_URL" -o "$tmp"
                    mkdir -p "$HOME/Applications"
                    unzip -q -o "$tmp" -d "$HOME/Applications"
                    rm -f "$tmp"
                    app_src="$HOME/Applications/fungOS.app"
                    ;;
                *) warn "unsupported app archive (expected .zip): $APP_URL" ;;
            esac
        fi
        if [ -d "$app_src" ] && [ -f "$app_src/Contents/Info.plist" ]; then
            mkdir_p "$HOME/Applications"
            if [ -e "$HOME/Applications/fungOS.app" ] && [ "$app_src" != "$HOME/Applications/fungOS.app" ]; then
                mv "$HOME/Applications/fungOS.app" "$HOME/Applications/fungOS.app.pre-$TS"
            fi
            if [ "$app_src" != "$HOME/Applications/fungOS.app" ]; then
                cp -R "$app_src" "$HOME/Applications/fungOS.app"
            fi
            chmod 755 "$HOME/Applications/fungOS.app/Contents/MacOS/canvas-winit" 2>/dev/null || true
            xattr -dr com.apple.quarantine "$HOME/Applications/fungOS.app" 2>/dev/null || true
            if [ ! -f "$HOME/Applications/fungOS.app/Contents/Resources/launcher.wasm" ]; then
                warn "fungOS.app is missing Contents/Resources/launcher.wasm"
            fi
            note "installed $HOME/Applications/fungOS.app"
        else
            warn "fungOS.app not found; pass --app PATH or --app-url URL"
        fi

        # The menu bar app is a separate bundle: it is per-user, needs no
        # root, and is the status/toggle surface that opens the web UI for
        # everything deeper.
        menubar_src=$MENUBAR_APP
        if [ -z "$menubar_src" ] || [ ! -d "$menubar_src/Contents" ]; then
            for candidate in "$ROOT/dist/macos/Mycelium.app" "$HOME/Applications/Mycelium.app"; do
                if [ -d "$candidate/Contents" ]; then menubar_src=$candidate; break; fi
            done
        fi
        if [ -n "$menubar_src" ] && [ -d "$menubar_src/Contents" ]; then
            menubar_dest=$HOME/Applications/Mycelium.app
            mkdir_p "$HOME/Applications"
            if [ "$menubar_src" != "$menubar_dest" ]; then
                if [ -e "$menubar_dest" ]; then
                    mv "$menubar_dest" "$menubar_dest.pre-$TS"
                fi
                cp -R "$menubar_src" "$menubar_dest"
            fi
            chmod 755 "$menubar_dest/Contents/MacOS/Mycelium" 2>/dev/null || true
            xattr -dr com.apple.quarantine "$menubar_dest" 2>/dev/null || true
            menubar_plist=$HOME/Library/LaunchAgents/dev.fpl.mycelium-menubar.plist
            if [ "$DRY_RUN" = 1 ]; then
                note "would write $menubar_plist"
            else
                mkdir -p "$HOME/Library/LaunchAgents"
                cat >"$menubar_plist" <<MENUBAR
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
    <key>Label</key><string>dev.fpl.mycelium-menubar</string>
    <key>ProgramArguments</key><array><string>$menubar_dest/Contents/MacOS/Mycelium</string></array>
    <key>RunAtLoad</key><true/>
    <key>KeepAlive</key><false/>
    <key>ProcessType</key><string>Interactive</string>
</dict></plist>
MENUBAR
                launchctl bootout "gui/$(id -u)/dev.fpl.mycelium-menubar" >/dev/null 2>&1 || true
                launchctl bootstrap "gui/$(id -u)" "$menubar_plist" >/dev/null 2>&1 || true
            fi
            note "installed $menubar_dest (menu bar)"
        fi
    elif [ -n "$APP_BUNDLE" ] || [ -n "$APP_URL" ]; then
        note "fungOS.app is a macOS bundle; skipping it on $(uname -s)"
    fi

    if [ "$(uname -s)" = Darwin ]; then
        if [ -x "$HOME/.local/share/fungos/bin/soil-home" ]; then
            install_launch_agent dev.fungos.soil-home "$HOME/.local/share/fungos/bin/soil-home" \
                "$HOME/.local/share/fungos/store/home.log" --routes "$MYCELIUM_HOME/frontdoor/routes.json"
        fi
        if [ -x "$HOME/.local/share/fungos/bin/soil-store" ]; then
            install_launch_agent dev.fungos.soil-store "$HOME/.local/share/fungos/bin/soil-store" \
                "$HOME/.local/share/fungos/store/store.log" serve \
                --index "$HOME/.local/share/fungos/store/index.json" \
                --spores-root "$HOME/.local/share/fungos/spores" \
                --packages "$HOME/.local/share/fungos/packages" \
                --media "$HOME/.local/share/fungos/media" \
                --reviews "$HOME/.local/share/fungos/store/reviews.json" \
                --page "$HOME/.local/share/fungos/store/page"
        fi
    else
        if [ -x "$HOME/.local/share/fungos/bin/soil-home" ]; then
            install_user_unit dev.fungos.soil-home "$HOME/.local/share/fungos/bin/soil-home" \
                --routes "$MYCELIUM_HOME/frontdoor/routes.json"
        fi
        if [ -x "$HOME/.local/share/fungos/bin/soil-store" ]; then
            install_user_unit dev.fungos.soil-store "$HOME/.local/share/fungos/bin/soil-store" \
                serve \
                --index "$HOME/.local/share/fungos/store/index.json" \
                --spores-root "$HOME/.local/share/fungos/spores" \
                --packages "$HOME/.local/share/fungos/packages" \
                --media "$HOME/.local/share/fungos/media" \
                --reviews "$HOME/.local/share/fungos/store/reviews.json" \
                --page "$HOME/.local/share/fungos/store/page"
        fi
    fi
}

install_bob() {
    if [ "$DRY_RUN" = 1 ]; then
        note "would build/install fpl-bob and run its setup"
        return 0
    fi
    if [ -x "$BIN_SRC/fpl-bob" ]; then
        install_atomic "$BIN_SRC/fpl-bob" "$LOCAL_BIN/fpl-bob"
    elif [ -n "$BOB_SRC" ] && [ -d "$BOB_SRC" ]; then
        ensure_rust
        say "Building fpl-bob from $BOB_SRC"
        ( cd "$BOB_SRC" && run cargo build --release --locked )
        install_atomic "$BOB_SRC/target/release/fpl-bob" "$LOCAL_BIN/fpl-bob"
    elif [ -d "$HOME/Documents/src/bob" ]; then
        install_bob_from "$HOME/Documents/src/bob"
    else
        ensure_rust
        src="${XDG_CACHE_HOME:-$HOME/.cache}/mycelium/install-src/bob"
        mkdir_p "$(dirname "$src")"
        if [ ! -d "$src/.git" ]; then run git clone --depth 1 "$BOB_REPO" "$src"; fi
        install_bob_from "$src"
    fi
    [ -x "$LOCAL_BIN/fpl-bob" ] || { warn "fpl-bob is unavailable"; return 1; }

    scheduler=$SCHEDULER_URL
    if [ -z "$scheduler" ] && [ "$NO_DIST" != 1 ]; then
        scheduler=$(ask_text "sccache-dist scheduler URL (blank = cache-only)" "")
    fi
    token=$CLIENT_TOKEN
    if [ -n "$scheduler" ] && [ -z "$token" ]; then
        token=$(ask_secret "sccache-dist client token")
    fi
    if [ -z "$REMOTE_MAC_HOST" ]; then
        REMOTE_MAC_HOST=$(ask_text "bob remote Mac host (blank = none)" "")
    fi
    if [ -z "$REMOTE_LINUX_HOST" ]; then
        REMOTE_LINUX_HOST=$(ask_text "bob remote Linux host (blank = none)" "")
    fi
    set -- setup
    if [ -n "$scheduler" ] && [ -n "$token" ] && [ "$NO_DIST" != 1 ]; then
        set -- "$@" --scheduler-url "$scheduler" --client-token "$token"
    else
        set -- "$@" --no-dist
    fi
    if [ -n "$REMOTE_MAC_HOST" ]; then set -- "$@" --remote-mac-host "$REMOTE_MAC_HOST"; fi
    if [ -n "$REMOTE_LINUX_HOST" ]; then set -- "$@" --remote-linux-host "$REMOTE_LINUX_HOST"; fi
    rustc_version=$(rustc --version 2>/dev/null | awk '{print $2}' || true)
    if [ -n "$rustc_version" ] && [ "$rustc_version" != "1.98.1" ]; then
        warn "rustc $rustc_version differs from the pinned 1.98.1; allowing drift"
        set -- "$@" --allow-rust-drift
    fi
    if [ "$DRY_RUN" = 1 ]; then
        note "would run fpl-bob $*"
    else
        printf '+ fpl-bob setup (token redacted)\n'
        "$LOCAL_BIN/fpl-bob" "$@"
        "$LOCAL_BIN/fpl-bob" status 2>/dev/null || true
    fi
}

install_bob_from() {
    src=$1
    ensure_rust
    say "Building fpl-bob"
    ( cd "$src" && run cargo build --release --locked )
    install_atomic "$src/target/release/fpl-bob" "$LOCAL_BIN/fpl-bob"
}

# A unit file that exists but is not running is the failure this installer cares
# about: the component installed and was never started.
unit_running() { # label unit
    if [ "$(uname -s)" = Darwin ]; then
        launchctl print "gui/$(id -u)/$1" >/dev/null 2>&1
        return
    fi
    systemctl --user is-active --quiet "$2.service" 2>/dev/null
}

check_service() { # label unit human
    if [ "$(uname -s)" = Darwin ]; then
        [ -f "$HOME/Library/LaunchAgents/$1.plist" ] || return 0
    else
        [ -f "$HOME/.config/systemd/user/$2.service" ] || return 0
    fi
    if unit_running "$1" "$2"; then
        note "$3 is running"
    else
        detail=""
        if [ "$(uname -s)" = Linux ]; then
            # "activating (auto-restart)" is a crash loop, not a stopped
            # service: the unit starts, exits and is restarted on a timer. That
            # reads as console noise long before anyone checks a status, so say
            # it here with the reason.
            state=$(systemctl --user show -p SubState --value "$2.service" 2>/dev/null)
            if [ "$state" = auto-restart ]; then
                restarts=$(systemctl --user show -p NRestarts --value "$2.service" 2>/dev/null)
                code=$(systemctl --user show -p ExecMainStatus --value "$2.service" 2>/dev/null)
                detail=" (crash-looping: $restarts restarts, last exit $code)"
            fi
        fi
        warn "$3 is installed but not running$detail"
        UNSTARTED="$UNSTARTED $3"
    fi
}

check_ingress() {
    for label in dev.fpl.mycelium-ingress-https dev.fpl.mycelium-ingress-http; do
        check_service "$label" "${label#dev.fpl.}" "$label"
    done
    if [ "$(uname -s)" = Linux ]; then
        # On Linux these are socket units: the service is inactive until a
        # connection arrives, so the socket is what has to be enabled.
        for unit in mycelium-ingress-https mycelium-ingress-http; do
            path="$HOME/.config/systemd/user/$unit.socket"
            [ -f "$path" ] || continue
            if systemctl --user is-active --quiet "$unit.socket" 2>/dev/null; then
                note "$unit.socket is listening"
            else
                warn "$unit.socket is installed but not listening"
                UNSTARTED="$UNSTARTED $unit.socket"
            fi
        done
    fi
}

final_check() {
    MYC="$LOCAL_BIN/mycelium"
    if [ "$DRY_RUN" = 1 ]; then
        note "dry run complete; no files or services were changed"
        return 0
    fi
    UNSTARTED=
    if [ "$START" = 1 ]; then
        say "Checking that installed services are running"
        check_service dev.fpl.mycelium mycelium myceliumd
        if [ "$FRONTDOOR" = yes ]; then
            check_service dev.fpl.mycelium-frontdoor mycelium-frontdoor the-frontdoor
            check_ingress
        fi
        if [ "$VFS" = yes ]; then
            check_service dev.fungos.vfs-fuse fungos-vfs-fuse the-VFS-mount
        fi
        if [ "$SOIL" = yes ]; then
            check_service dev.fungos.soil-store dev.fungos.soil-store the-Spore-store
        fi
        if [ "$VPN" = yes ] && [ -x "$MYC" ]; then
            if "$MYC" iroh-tun status --json 2>/dev/null \
                | grep -Eq '"publishing"[[:space:]]*:[[:space:]]*true'; then
                note "iroh-tun is publishing"
            else
                warn "iroh-tun is installed but not publishing"
                UNSTARTED="$UNSTARTED iroh-tun"
            fi
        fi
    fi
    if [ "$START" = 1 ] && [ -x "$MYC" ]; then
        say "Checking the installation"
        if "$MYC" doctor --json >"$MYCELIUM_HOME/install-doctor.json" 2>&1; then
            note "doctor completed; details in $MYCELIUM_HOME/install-doctor.json"
        else
            warn "doctor reported a problem; see $MYCELIUM_HOME/install-doctor.json"
        fi
    fi
    if [ -n "$UNSTARTED" ]; then
        warn "services that are installed but not running:$UNSTARTED"
        warn "start them with launchctl bootstrap (macOS) or systemctl --user enable --now (Linux)"
    fi
    if [ -n "$FAILED" ]; then
        warn "components that need attention:$FAILED"
        return 1
    fi
    say "Installation complete"
}

# Component flags record that they were passed explicitly. A profile only fills
# the components the command line left alone.
declared() { DECLARED="$DECLARED $1"; }

want() { # VAR VALUE
    case " $DECLARED " in
        *" $1 "*) return 0 ;;
    esac
    eval "$1=\$2"
}

# The default for the components that only exist as source builds, plus the
# reset of a profile's answer for them. Run after apply_profile: a profile's
# `yes` is a wish, and a release install cannot honour it without a compiler.
apply_release_defaults() {
    # A release archive carries Mycelium's binaries and iroh-tun. fungos-vfs,
    # Soil and bob are built from their own repositories, so a release install
    # defaults them off instead of reaching for a compiler. An explicit
    # --vfs/--soil/--bob still builds them, which is the caller's choice.
    LOCAL_COMPONENT_DEFAULT=yes
    RELEASE_ONLY=no
    [ -n "$RELEASE_VERSION" ] || return 0
    RELEASE_ONLY=yes
    LOCAL_COMPONENT_DEFAULT=no
    for name in VFS SOIL BOB; do
        case " $DECLARED " in
            *" $name "*) ;;
            *)
                eval "current=\${$name}"
                if [ "$current" = yes ]; then
                    flag=$(printf '%s' "$name" | tr 'A-Z' 'a-z')
                    note "release install: $flag builds from source, so it is off; pass --$flag to build it"
                fi
                eval "$name=ask"
                ;;
        esac
    done
}

# A profile picks the components this machine should have. Explicit flags always
# win; the profile only fills what the command line did not decide.
#
# The channel decides the state directory, the binary directory and the service
# label before anything else is computed from them. `stable` keeps the historic
# paths so an existing install is untouched.
apply_channel() {
    case "$CHANNEL" in
        stable) CHANNEL_SUFFIX= ;;
        canary)
            CHANNEL_SUFFIX=-canary
            if [ -z "${MYCELIUM_HOME_SET:-}" ]; then
                MYCELIUM_HOME=$HOME/.mycelium-canary
            fi
            if [ -z "${MYCELIUM_PREFIX_SET:-}" ]; then
                PREFIX=$HOME/.local/canary
            fi
            ;;
        *) die "--channel must be stable or canary, not $CHANNEL" ;;
    esac
    BIN_DIR=$MYCELIUM_HOME/bin
    LOCAL_BIN=$PREFIX/bin
    export MYCELIUM_CHANNEL=$CHANNEL
    export MYCELIUM_HOME
}

# The canary channel is a second install, not a second owner of the machine.
# Everything system-wide - the DNS resolver's root daemon and /etc/resolver, the
# privileged frontdoor/ingress ports, the root tunnel - belongs to the stable
# install, so a canary run leaves those components off unless they are named
# explicitly on the command line.
apply_channel_defaults() {
    [ "$CHANNEL" = canary ] || return 0
    for name in VPN VFS SOIL BOB OPERATOR FRONTDOOR GEODE; do
        case " $DECLARED " in
            *" $name "*) ;;
            *) eval "$name=no" ;;
        esac
    done
    [ "$ENROLL" = ask ] && ENROLL=no
    note "canary install: daemon and CLI only; system DNS, frontdoor and tunnel stay with stable"
}

# Write the canary launcher: the same binary with the canary state and channel,
# reachable as `mycelium-canary` from the login PATH.
install_canary_launcher() {
    wrapper="$HOME/.local/bin/mycelium-canary"
    if [ "$DRY_RUN" = 1 ]; then
        note "would write $wrapper"
        return 0
    fi
    mkdir -p "$(dirname "$wrapper")"
    {
        printf '%s\n' '#!/bin/sh'
        printf '%s\n' '# Mycelium canary channel: same binary, separate state and services.'
        printf 'export MYCELIUM_HOME="${MYCELIUM_HOME:-%s}"\n' "$MYCELIUM_HOME"
        printf '%s\n' 'export MYCELIUM_CHANNEL=canary'
        printf 'exec %s "$@"\n' "$BIN_DIR/mycelium"
    } >"$wrapper"
    chmod 755 "$wrapper"
    note "installed $wrapper"
}

# The canary daemon as a user service. The CLI's own `setup --repair` writes the
# stable label and the stable home, so a canary install writes this one itself;
# it is the only place the two channels define a service independently.
install_canary_service() {
    label="dev.fpl.mycelium$CHANNEL_SUFFIX"
    unit="mycelium$CHANNEL_SUFFIX"
    program="$BIN_DIR/mycelium"
    log="$MYCELIUM_HOME/daemon.log"
    if [ "$DRY_RUN" = 1 ]; then
        note "would install the $CHANNEL daemon service ($label)"
        return 0
    fi
    if [ "$(uname -s)" = Darwin ]; then
        plist="$HOME/Library/LaunchAgents/$label.plist"
        mkdir -p "$(dirname "$plist")" "$(dirname "$log")"
        {
            printf '%s\n' '<?xml version="1.0" encoding="UTF-8"?>'
            printf '%s\n' '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">'
            printf '%s\n' '<plist version="1.0"><dict>'
            printf '<key>Label</key><string>%s</string>\n' "$(xml_escape "$label")"
            printf '%s\n' '<key>ProgramArguments</key><array>'
            printf '<string>%s</string><string>_serve</string>\n' "$(xml_escape "$program")"
            printf '%s\n' '</array>'
            printf '%s\n' '<key>EnvironmentVariables</key><dict>'
            printf '<key>MYCELIUM_HOME</key><string>%s</string>\n' "$(xml_escape "$MYCELIUM_HOME")"
            printf '%s\n' '<key>MYCELIUM_CHANNEL</key><string>canary</string>'
            printf '%s\n' '</dict>'
            printf '%s\n' '<key>RunAtLoad</key><true/><key>KeepAlive</key><true/>'
            printf '<key>ThrottleInterval</key><integer>10</integer>\n'
            printf '<key>StandardOutPath</key><string>%s</string>\n' "$(xml_escape "$log")"
            printf '<key>StandardErrorPath</key><string>%s</string>\n' "$(xml_escape "$log")"
            printf '%s\n' '</dict></plist>'
        } >"$plist"
        launchctl bootout "gui/$(id -u)/$label" >/dev/null 2>&1 || true
        if [ "$START" = 1 ]; then
            launchctl bootstrap "gui/$(id -u)" "$plist" || warn "could not start $label"
        fi
        note "installed $plist"
        return 0
    fi
    unit_path="$HOME/.config/systemd/user/$unit.service"
    mkdir -p "$(dirname "$unit_path")"
    {
        printf '[Unit]\nDescription=Mycelium canary daemon\nAfter=default.target\n\n'
        printf '[Service]\nEnvironment=MYCELIUM_HOME=%s\nEnvironment=MYCELIUM_CHANNEL=canary\n' "$MYCELIUM_HOME"
        printf 'ExecStart=%s _serve\nRestart=always\nRestartSec=10\n\n[Install]\nWantedBy=default.target\n' "$program"
    } >"$unit_path"
    if command -v systemctl >/dev/null 2>&1; then
        systemctl --user daemon-reload || true
        if [ "$START" = 1 ]; then
            systemctl --user enable --now "$unit.service" || warn "could not start $unit"
        fi
    fi
    note "installed $unit_path"
}

apply_profile() {
    case "$VFS_ROLE" in
        full|cache) ;;
        *) die "--vfs-role must be full or cache, not $VFS_ROLE" ;;
    esac
    case "$PROFILE" in
        full) ;;
        canvas)
            want SOIL yes
            want VFS yes
            want BOB no
            want GEODE no
            want FRONTDOOR yes
            ;;
        node)
            want SOIL yes
            want BOB no
            want OPERATOR no
            want GEODE no
            want FRONTDOOR yes
            ;;
        flash)
            want SOIL yes
            want VFS yes
            want BOB no
            want OPERATOR no
            want GEODE no
            want FRONTDOOR yes
            want VFS_ROLE cache
            ;;
        minimal)
            want VPN no
            want SOIL no
            want VFS no
            want BOB no
            want OPERATOR no
            want GEODE no
            want FRONTDOOR no
            ;;
        *) die "unknown profile: $PROFILE (full, canvas, node, flash, minimal)" ;;
    esac
    if [ "$PROFILE" != full ]; then
        note "profile $PROFILE: vpn=$VPN vfs=$VFS role=$VFS_ROLE soil=$SOIL bob=$BOB operator=$OPERATOR frontdoor=$FRONTDOOR geode=$GEODE"
    fi
}

parse_args() {
    while [ $# -gt 0 ]; do
        case "$1" in
            -y|--yes) YES=1; shift ;;
            --dry-run) DRY_RUN=1; shift ;;
            --force) FORCE=1; shift ;;
            --no-start) START=0; shift ;;
            --enable-updates) ENABLE_UPDATES=yes; shift ;;
            --no-enable-updates) ENABLE_UPDATES=no; shift ;;
            --no-enroll) ENROLL=no; shift ;;
            --enroll) ENROLL=yes; shift ;;
            --vpn) VPN=yes; declared VPN; shift ;;
            --no-vpn) VPN=no; declared VPN; shift ;;
            --vfs) VFS=yes; declared VFS; shift ;;
            --no-vfs) VFS=no; declared VFS; shift ;;
            --soil) SOIL=yes; declared SOIL; shift ;;
            --no-soil) SOIL=no; declared SOIL; shift ;;
            --bob) BOB=yes; declared BOB; shift ;;
            --no-bob) BOB=no; declared BOB; shift ;;
            --operator) OPERATOR=yes; declared OPERATOR; shift ;;
            --no-operator) OPERATOR=no; declared OPERATOR; shift ;;
            --geode) GEODE=yes; declared GEODE; shift ;;
            --no-geode) GEODE=no; declared GEODE; shift ;;
            --dns) DNS=yes; shift ;;
            --no-dns) DNS=no; shift ;;
            --frontdoor) FRONTDOOR=yes; declared FRONTDOOR; shift ;;
            --no-frontdoor) FRONTDOOR=no; declared FRONTDOOR; shift ;;
            --profile) [ $# -ge 2 ] || die "--profile needs a name"; PROFILE=$2; shift 2 ;;
            --vfs-role) [ $# -ge 2 ] || die "--vfs-role needs full or cache"; VFS_ROLE=$2; declared VFS_ROLE; shift 2 ;;
            --trust-cert) TRUST_CERT=yes; shift ;;
            --no-trust-cert) TRUST_CERT=no; shift ;;
            --local-frontdoor|--local-gateway) LOCAL_FRONTDOOR=1; shift ;;
            --gateway) [ $# -ge 2 ] || die "--gateway needs a URL"; GATEWAY=$2; shift 2 ;;
            --claim|--code) [ $# -ge 2 ] || die "$1 needs a code"; CLAIM=$2; shift 2 ;;
            --claim-file) [ $# -ge 2 ] || die "--claim-file needs a path"; CLAIM_FILE=$2; shift 2 ;;
            --source) [ $# -ge 2 ] || die "--source needs a directory"; SRC=$2; shift 2 ;;
            --bin-dir) [ $# -ge 2 ] || die "--bin-dir needs a directory"; BIN_SRC=$2; shift 2 ;;
            --release) [ $# -ge 2 ] || die "--release needs a tag"; RELEASE_VERSION=$2; shift 2 ;;
            --release-base) [ $# -ge 2 ] || die "--release-base needs a URL"; RELEASE_BASE=$2; shift 2 ;;
            --release-key) [ $# -ge 2 ] || die "--release-key needs a hex key"; RELEASE_KEYS="$RELEASE_KEYS $2"; shift 2 ;;
            --prefix) [ $# -ge 2 ] || die "--prefix needs a directory"; PREFIX=$2; LOCAL_BIN=$PREFIX/bin; shift 2 ;;
            --channel) [ $# -ge 2 ] || die "--channel needs stable or canary"; CHANNEL=$2; shift 2 ;;
            --overlay) [ $# -ge 2 ] || die "--overlay needs a CIDR"; OVERLAY=$2; shift 2 ;;
            --tun) [ $# -ge 2 ] || die "--tun needs an interface"; TUN=$2; shift 2 ;;
            --status) [ $# -ge 2 ] || die "--status needs a path"; STATUS_PATH=$2; shift 2 ;;
            --app) [ $# -ge 2 ] || die "--app needs a bundle path"; APP_BUNDLE=$2; shift 2 ;;
            --app-url) [ $# -ge 2 ] || die "--app-url needs a URL"; APP_URL=$2; shift 2 ;;
            --menubar-app) [ $# -ge 2 ] || die "--menubar-app needs a bundle path"; MENUBAR_APP=$2; shift 2 ;;
            --soil-runtime) [ $# -ge 2 ] || die "--soil-runtime needs a directory"; SOIL_RUNTIME=$2; shift 2 ;;
            --spores-src) [ $# -ge 2 ] || die "--spores-src needs a directory"; SPORES_SOURCE=$2; shift 2 ;;
            --terminal-host) [ $# -ge 2 ] || die "--terminal-host needs a path"; TERMINAL_HOST_BIN=$2; shift 2 ;;
            --vfs-config) [ $# -ge 2 ] || die "--vfs-config needs a path"; VFS_CONFIG=$2; shift 2 ;;
            --vfs-root) [ $# -ge 2 ] || die "--vfs-root needs a path"; VFS_ROOT=$2; shift 2 ;;
            --vfs-mount) [ $# -ge 2 ] || die "--vfs-mount needs a path"; VFS_MOUNT=$2; shift 2 ;;
            --bob-src) [ $# -ge 2 ] || die "--bob-src needs a directory"; BOB_SRC=$2; shift 2 ;;
            --scheduler-url) [ $# -ge 2 ] || die "--scheduler-url needs a URL"; SCHEDULER_URL=$2; shift 2 ;;
            --client-token) [ $# -ge 2 ] || die "--client-token needs a value"; CLIENT_TOKEN=$2; shift 2 ;;
            --remote-mac-host) [ $# -ge 2 ] || die "--remote-mac-host needs a host"; REMOTE_MAC_HOST=$2; shift 2 ;;
            --remote-linux-host) [ $# -ge 2 ] || die "--remote-linux-host needs a host"; REMOTE_LINUX_HOST=$2; shift 2 ;;
            --no-dist) NO_DIST=1; shift ;;
            --reachable) [ $# -ge 2 ] || die "--reachable needs a CIDR"; REACHABLE="$REACHABLE $2"; shift 2 ;;
            -h|--help) usage; exit 0 ;;
            *) die "unknown option: $1 (try --help)" ;;
        esac
    done
}

preflight() {
    case "$(uname -s)" in
        Darwin|Linux) ;;
        *) die "unsupported operating system: $(uname -s)" ;;
    esac
    if [ "$(id -u)" -eq 0 ]; then
        SUDO=
        warn "running as root; user-level services will be installed for root"
    else
        SUDO=sudo
        command -v sudo >/dev/null 2>&1 || die "sudo is required for the root tunnel and DNS service"
    fi
    if [ "$DRY_RUN" = 0 ]; then
        mkdir_p "$MYCELIUM_HOME" "$BIN_DIR" "$LOCAL_BIN"
        LOG="$MYCELIUM_HOME/install.log"
        : >"$LOG"
    else
        LOG=
    fi
    if [ -n "$BIN_SRC" ]; then
        BIN_SRC=$(cd -- "$BIN_SRC" 2>/dev/null && pwd || printf '%s' "$BIN_SRC")
    fi
    export PATH="$LOCAL_BIN:$BIN_DIR:$HOME/.cargo/bin:$PATH"
}

# Automatic updates keep a member's host on the signed release track, so the
# install path decides the default: a release install turns them on for the
# channel that release was published on, while a source or --bin-dir build
# leaves them off (a local build has no release to update to, and the signed
# channel may be older than the binary it replaced). --enable-updates and
# --no-enable-updates override the default either way.
resolve_updates_default() {
    [ "$ENABLE_UPDATES" = ask ] || return 0
    if [ -n "$RELEASE_VERSION" ]; then
        ENABLE_UPDATES=yes
    else
        ENABLE_UPDATES=no
    fi
}

# Where this host gets releases from, and which keys may sign them. Only a
# release install has a source; only a pinned key makes it usable. Without one,
# `update fetch` refuses rather than trusting whatever the channel serves.
write_release_source() {
    [ -n "$RELEASE_VERSION" ] || return 0
    if [ -z "$RELEASE_KEYS" ]; then
        if [ "$ENABLE_UPDATES" = yes ]; then
            warn "no release key pinned; automatic updates cannot verify a download (pass --release-key HEX)"
        fi
        return 0
    fi
    base=$RELEASE_BASE
    if [ -z "$base" ]; then
        case "$RELEASE_VERSION" in
            latest) base="https://github.com/$RELEASE_REPO/releases/latest/download" ;;
            v*) base="https://github.com/$RELEASE_REPO/releases/download/$RELEASE_VERSION" ;;
            *) base="https://github.com/$RELEASE_REPO/releases/download/v$RELEASE_VERSION" ;;
        esac
    fi
    channel=${RELEASE_CHANNEL:-stable}
    path="$MYCELIUM_HOME/release-source.json"
    if [ "$DRY_RUN" = 1 ]; then
        note "would write $path (base $base, channel $channel)"
        return 0
    fi
    keys=
    count=0
    for key in $RELEASE_KEYS; do
        keys="$keys${keys:+, }\"$key\""
        count=$((count + 1))
    done
    mkdir_p "$MYCELIUM_HOME"
    cat >"$path" <<EOF
{
  "base": "$base",
  "channel": "$channel",
  "trusted_keys": [$keys]
}
EOF
    chmod 644 "$path"
    note "wrote $path (channel $channel, $count pinned key(s))"
}

apply_update_policy() {
    MYC="$LOCAL_BIN/mycelium"
    [ -x "$MYC" ] || return 0
    if [ "$DRY_RUN" = 1 ]; then
        if [ "$ENABLE_UPDATES" = yes ]; then
            note "would enable automatic updates on channel ${RELEASE_CHANNEL:-from the release metadata}"
        else
            note "would leave automatic updates disabled"
        fi
        return 0
    fi
    if [ "$ENABLE_UPDATES" = yes ]; then
        set -- update policy enable
        if [ -n "$RELEASE_CHANNEL" ]; then set -- "$@" --channel "$RELEASE_CHANNEL"; fi
        set -- "$@" --write
        if run "$MYC" "$@"; then
            note "automatic updates enabled${RELEASE_CHANNEL:+ on channel $RELEASE_CHANNEL}"
            if [ -n "$RELEASE_VERSION" ] && [ ! -f "$MYCELIUM_HOME/release-source.json" ]; then
                warn "no release source: updates can only use artifacts already on this host"
            fi
        else
            warn "could not enable automatic updates; run: mycelium update policy enable --write"
        fi
        return 0
    fi
    if run "$MYC" update policy disable --write; then
        note "automatic updates left disabled (re-enable with --enable-updates)"
    fi
}

pause_updater() {
    if [ "$DRY_RUN" = 1 ]; then return 0; fi
    if [ "$(uname -s)" = Darwin ]; then
        if launchctl print "gui/$(id -u)/dev.fpl.mycelium-updater" >/dev/null 2>&1; then
            if launchctl bootout "gui/$(id -u)/dev.fpl.mycelium-updater" >/dev/null 2>&1; then
                UPDATER_PAUSED=1
                note "paused the existing automatic-update agent during install"
            fi
        fi
    elif command -v systemctl >/dev/null 2>&1; then
        if systemctl --user stop mycelium-updater >/dev/null 2>&1; then
            UPDATER_PAUSED=1
        fi
    fi
}

resume_updater() {
    [ "$ENABLE_UPDATES" = yes ] || return 0
    [ "$UPDATER_PAUSED" = 1 ] || return 0
    if [ "$(uname -s)" = Darwin ]; then
        plist="$HOME/Library/LaunchAgents/dev.fpl.mycelium-updater.plist"
        [ -f "$plist" ] || return 0
        launchctl bootstrap "gui/$(id -u)" "$plist" >/dev/null 2>&1 \
            || warn "could not restart the automatic-update agent"
    elif command -v systemctl >/dev/null 2>&1; then
        systemctl --user start mycelium-updater >/dev/null 2>&1 \
            || warn "could not restart the automatic-update service"
    fi
}

main() {
    parse_args "$@"
    apply_channel
    # A piped installer has no checkout to build from, and a member should not
    # have to install Rust to join. So the published release is the default when
    # this script is not running out of a source tree; a checkout keeps the
    # source build, and --release/--source/--bin-dir always win.
    if [ -z "$RELEASE_VERSION" ] && [ -z "$SRC" ] && [ -z "$BIN_SRC" ] \
        && ! in_checkout; then
        RELEASE_VERSION=latest
        note "no checkout here; installing the latest published release"
    fi
    resolve_updates_default
    apply_profile
    apply_release_defaults
    apply_channel_defaults
    if [ -n "$RELEASE_VERSION" ]; then
        [ -z "$BIN_SRC" ] || die "--release and --bin-dir are mutually exclusive"
        [ -z "$SRC" ] || die "--release and --source are mutually exclusive"
    fi
    # A join code is meant to be consumed without a conversation. `curl | bash`
    # leaves stdin as a pipe while /dev/tty still exists, so test stdin: piping
    # the installer with a code selects the documented component defaults
    # instead of dying at the prompt guard.
    if [ "$DRY_RUN" = 0 ] && [ "$YES" != 1 ] && [ ! -t 0 ] \
        && { [ -n "$CLAIM" ] || [ -n "$CLAIM_FILE" ]; }; then
        note "join code supplied in a piped install; using the component defaults"
        YES=1
    fi
    if [ "$DRY_RUN" = 0 ] && [ "$YES" != 1 ] && ! have_tty; then
        for name in VPN VFS SOIL BOB OPERATOR GEODE ENROLL; do
            eval "value=\${$name}"
            [ "$value" = ask ] && die "no terminal for prompts; pass --yes or set --vpn/--no-vpn, --vfs/--no-vfs, --soil/--no-soil, --bob/--no-bob, --operator/--no-operator, --geode/--no-geode, and --enroll/--no-enroll"
        done
    fi
    preflight
    say "Mycelium installer"
    note "home: $MYCELIUM_HOME"
    note "binaries: $BIN_DIR and $LOCAL_BIN"
    choose VPN yes "Install the IPv6 iroh-tun VPN? (needs sudo)"
    if [ "$VPN" = yes ]; then
        if [ "$(id -u)" -ne 0 ] && [ "$DRY_RUN" = 0 ] && ! have_tty; then
            warn "VPN selected but no terminal is available for sudo; use --no-vpn or pre-authorize sudo"
        fi
        need_root "install the root-owned iroh-tun service"
    fi
    if [ "$RELEASE_ONLY" = yes ]; then
        for name in VFS SOIL BOB; do
            eval "value=\${$name}"
            [ "$value" = yes ] \
                && note "$name builds from source; this needs a Rust toolchain"
        done
    fi
    choose VFS "$LOCAL_COMPONENT_DEFAULT" "Install the content VFS (fungos-vfs + FUSE/LaunchAgent)?"
    choose SOIL "$LOCAL_COMPONENT_DEFAULT" "Install Soil and fungOS.app?"
    choose BOB "$LOCAL_COMPONENT_DEFAULT" "Install build offload (fpl-bob + sccache)?"
    choose OPERATOR yes "Configure the operator surface (skills, completions, update policy)?"
    choose FRONTDOOR yes "Install the loopback frontdoor and its ingress services?"
    choose GEODE no "Install the optional geode credential broker?"
    if [ "$ENROLL" = ask ]; then
        if ask_yes "Enroll or repair this node's fleet membership?" yes; then ENROLL=yes; else ENROLL=no; fi
    fi
    if [ "$CHANNEL" = canary ]; then
        # The updater is a stable-install service: its agent path carries no
        # MYCELIUM_HOME, so a canary install that enabled it would replace the
        # stable binary. A canary install is updated by running the installer
        # again, which is the point of a dev channel.
        if [ "$ENABLE_UPDATES" = yes ]; then
            note "canary installs are updated by reinstalling; automatic updates stay off"
        fi
        install_mycelium
        install_canary_launcher
        run_component canary install_canary_service
        say "Canary install complete"
        note "run it as: mycelium-canary doctor"
        return 0
    fi
    pause_updater
    if [ "$ENABLE_UPDATES" = yes ]; then trap resume_updater EXIT; fi
    install_mycelium
    write_release_source
    setup_node
    apply_update_policy
    if [ "$VPN" = yes ]; then run_component vpn install_vpn; fi
    if [ "$DNS" = yes ]; then run_component dns install_dns; fi
    if [ "$FRONTDOOR" = yes ]; then run_component frontdoor install_frontdoor; fi
    if [ "$OPERATOR" = yes ]; then run_component operator install_operator; fi
    if [ "$GEODE" = yes ]; then run_component geode install_geode; fi
    if [ "$VFS" = yes ]; then run_component vfs install_vfs; fi
    if [ "$SOIL" = yes ]; then run_component soil install_soil; fi
    if [ "$BOB" = yes ]; then run_component bob install_bob; fi
    final_check
}

# Unit tests source the helpers without running an install.
if [ "${MYCELIUM_INSTALL_LIB:-0}" != 1 ]; then
    main "$@"
fi
