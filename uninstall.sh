#!/usr/bin/env bash
# Mycelium uninstall: stop and remove what install.sh and the CLI installed.
#
#   ./uninstall.sh --dry-run      # print the plan, change nothing
#   ./uninstall.sh                # remove services and binaries, keep state
#   ./uninstall.sh --purge        # also remove state (archived first)
#
# The order matters and follows ownership, not directories: each component is
# asked to remove itself through its own CLI while that CLI still exists, and
# only then does the script sweep what is left. A component that is already
# broken or missing falls through to the sweep, because an uninstaller that
# needs the thing it is uninstalling is an uninstaller that cannot clean up.
#
# State is kept unless --purge is given: keys, the enrolled identity, the CA
# and the DNS policy are not reproducible from a release. --purge archives
# them to ~/mycelium-uninstall-<timestamp>.tar.gz first (0700) unless
# --no-backup is passed, and never deletes a directory it cannot recognise.

set -Eeuo pipefail

DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]:-$0}")" 2>/dev/null && pwd || pwd)
# Shared constants, prompts and the dry-run/verbose helpers live in install.sh;
# sourcing it with the library switch defines them without running an install.
MYCELIUM_INSTALL_LIB=1
# shellcheck disable=SC1091
source "$DIR/install.sh"

TS=${TS:-$(date +%Y%m%dT%H%M%S)}
PURGE=0
NO_BACKUP=0
KEEP_BINARIES=0
ROOT_OK=0
# System-wide wiring (the root tunnel daemon, the DNS daemon, /etc/resolver,
# the root-owned copy of the binary) belongs to the default home only: it is
# the install that claimed the machine. A custom --home is a sandbox or a
# channel, and must not remove the machine's ingress.
SYSTEM_WIRING=1
CHANNEL=stable
CHANNEL_SUFFIX=
HOME_DECLARED=0
PREFIX_DECLARED=0
SUDO=sudo

usage() {
    cat <<'USAGE'
usage: uninstall.sh [options]

Stops and removes the Mycelium services, binaries and shell integration. State
(keys, identity, CA, DNS policy) is kept unless --purge is passed.

Options:
  --dry-run        print the plan; change nothing
  --yes            do not prompt
  --purge          also remove state and application data
  --no-backup      with --purge, do not archive state first
  --keep-binaries  remove services and shell integration, keep the binaries
  --home DIR       state directory to remove (default ~/.mycelium)
  --prefix DIR     user prefix whose bin/ holds the binaries (default ~/.local)
  --channel NAME   stable (default) or canary; picks that channel's home,
                   prefix and service labels
  -h, --help       this text
USAGE
}

parse_args() {
    while [ $# -gt 0 ]; do
        case "$1" in
            --dry-run) DRY_RUN=1; shift ;;
            --yes|-y) YES=1; shift ;;
            --purge) PURGE=1; shift ;;
            --no-backup) NO_BACKUP=1; shift ;;
            --keep-binaries) KEEP_BINARIES=1; shift ;;
            --home) MYCELIUM_HOME=$2; BIN_DIR=$MYCELIUM_HOME/bin; SYSTEM_WIRING=0; HOME_DECLARED=1; shift 2 ;;
            --prefix) PREFIX=$2; LOCAL_BIN=$PREFIX/bin; PREFIX_DECLARED=1; shift 2 ;;
            --channel)
                CHANNEL=$2
                case "$CHANNEL" in
                    stable) CHANNEL_SUFFIX= ;;
                    canary)
                        CHANNEL_SUFFIX=-canary
                        # A canary install owns its state and binaries and never
                        # the machine's system wiring. The channel flag is
                        # stronger than an inherited MYCELIUM_HOME/MYCELIUM_PREFIX:
                        # an explicit --home/--prefix is stronger still.
                        if [ "$HOME_DECLARED" = 0 ]; then
                            MYCELIUM_HOME=$HOME/.mycelium-canary
                            BIN_DIR=$MYCELIUM_HOME/bin
                        fi
                        if [ "$PREFIX_DECLARED" = 0 ]; then
                            PREFIX=$HOME/.local/canary
                            LOCAL_BIN=$PREFIX/bin
                        fi
                        SYSTEM_WIRING=0
                        ;;
                    *) die "--channel must be stable or canary, not $CHANNEL" ;;
                esac
                shift 2
                ;;
            -h|--help) usage; exit 0 ;;
            *) die "unknown option: $1 (try --help)" ;;
        esac
    done
}

# Every step is printed before it runs, so the log is the plan. A dry run ends
# at the print.
act() { # description command...
    note "$1"
    shift
    [ "$DRY_RUN" = 1 ] && return 0
    "$@"
}

# Root steps are optional: a host where the operator cannot or will not
# authenticate still gets its user services removed, and the leftovers are
# reported rather than leaving the run half-dead at the first sudo prompt.
get_root() {
    [ "$ROOT_OK" = 1 ] && return 0
    [ "$(id -u)" -eq 0 ] && { ROOT_OK=1; SUDO=; return 0; }
    if [ "$DRY_RUN" = 1 ]; then
        ROOT_OK=1
        return 0
    fi
    if "$SUDO" -n true 2>/dev/null || "$SUDO" -v 2>/dev/null; then
        ROOT_OK=1
        return 0
    fi
    warn "sudo was not granted; root-owned parts (tunnel daemon, DNS daemon, $ROOT_LIBEXEC) are left in place"
    return 1
}

cli() {
    if [ -x "$LOCAL_BIN/mycelium" ]; then printf '%s' "$LOCAL_BIN/mycelium"; fi
}

# Ask a component to remove itself, while it still can.
component_uninstall() {
    myc=$(cli)
    if [ -n "$myc" ]; then
        act "stop the daemon" "$myc" daemon stop || true
        act "disable automatic updates" "$myc" update policy disable --write || true
        act "remove the frontdoor and ingress services" "$myc" frontdoor uninstall --write || true
        cfg=$MYCELIUM_HOME/dns/active-settings.json
        if [ -f "$cfg" ]; then
            act "remove the system DNS resolver" "$myc" dns resolver system-remove --config "$cfg" --write || true
        fi
        act "disable the iroh-tun service" "$myc" iroh-tun config --enabled false --write || true
    else
        note "no mycelium binary at $LOCAL_BIN; sweeping service definitions instead"
    fi
}

remove_launch_agent() {
    label=$1
    plist="$HOME/Library/LaunchAgents/$label.plist"
    [ "$DRY_RUN" = 1 ] && [ -f "$plist" ] && note "remove $plist"
    if [ "$DRY_RUN" != 1 ]; then
        launchctl bootout "gui/$(id -u)/$label" >/dev/null 2>&1 || true
        [ -f "$plist" ] && note "remove $plist"
        rm -f "$plist"
    fi
}

remove_user_unit() {
    unit=$1
    path="$HOME/.config/systemd/user/$unit.service"
    if [ "$DRY_RUN" != 1 ]; then
        if command -v systemctl >/dev/null 2>&1; then
            systemctl --user disable --now "$unit.service" >/dev/null 2>&1 || true
        fi
        [ -f "$path" ] && note "remove $path"
        rm -f "$path"
    elif [ -f "$path" ]; then
        note "remove $path"
    fi
}

sweep_user_services() {
    # Every label the installer and the CLI can leave behind on a stable
    # install. Keeping the whole list here is the point: a label that is added
    # elsewhere and not here becomes an orphan that survives uninstall.
    for label in \
        dev.fpl.mycelium$CHANNEL_SUFFIX \
        dev.fpl.mycelium-frontdoor$CHANNEL_SUFFIX \
        dev.fpl.mycelium-ingress-https$CHANNEL_SUFFIX \
        dev.fpl.mycelium-ingress-http$CHANNEL_SUFFIX \
        dev.fpl.mycelium-updater$CHANNEL_SUFFIX \
        dev.fpl.mycelium-dns$CHANNEL_SUFFIX \
        dev.fpl.mycelium-dns-client$CHANNEL_SUFFIX \
        dev.fpl.mycelium-doh$CHANNEL_SUFFIX \
        dev.fpl.mycelium-menubar$CHANNEL_SUFFIX \
        dev.fpl.geoded$CHANNEL_SUFFIX \
        dev.fungos.vfs-fuse$CHANNEL_SUFFIX \
        dev.fungos.vfs-content$CHANNEL_SUFFIX \
        dev.fungos.soil-home$CHANNEL_SUFFIX \
        dev.fungos.soil-store$CHANNEL_SUFFIX \
        dev.fpl.mycelium-gateway$CHANNEL_SUFFIX \
        dev.fpl.mycelium-https$CHANNEL_SUFFIX; do
        remove_launch_agent "$label"
    done
    for unit in \
        mycelium$CHANNEL_SUFFIX \
        mycelium-frontdoor$CHANNEL_SUFFIX \
        mycelium-ingress-https$CHANNEL_SUFFIX \
        mycelium-ingress-http$CHANNEL_SUFFIX \
        mycelium-update$CHANNEL_SUFFIX \
        mycelium-dns$CHANNEL_SUFFIX \
        mycelium-updater$CHANNEL_SUFFIX \
        geoded$CHANNEL_SUFFIX \
        fungos-vfs-fuse$CHANNEL_SUFFIX \
        fungos-vfs-content$CHANNEL_SUFFIX \
        fungos-soil-home$CHANNEL_SUFFIX \
        fungos-soil-store$CHANNEL_SUFFIX; do
        remove_user_unit "$unit"
    done
    if [ "$DRY_RUN" != 1 ] && command -v systemctl >/dev/null 2>&1; then
        systemctl --user daemon-reload >/dev/null 2>&1 || true
    fi
}

sweep_system_services() {
    if [ "$(uname -s)" = Darwin ]; then
        for label in dev.fpl.iroh-tun dev.fpl.mycelium-dnsd; do
            plist="/Library/LaunchDaemons/$label.plist"
            [ -f "$plist" ] || continue
            act "remove root daemon $label" "$SUDO" launchctl bootout "system/$label" || true
            act "remove $plist" "$SUDO" rm -f "$plist" || true
        done
        [ -d "$ROOT_LIBEXEC" ] && act "remove $ROOT_LIBEXEC" "$SUDO" rm -rf "$ROOT_LIBEXEC" || true
        [ -d /var/run/mycelium ] && act "remove /var/run/mycelium" "$SUDO" rm -rf /var/run/mycelium || true
        resolver=/etc/resolver/myc
        [ -f "$resolver" ] && act "remove $resolver" "$SUDO" rm -f "$resolver" || true
    else
        for unit in mycelium-iroh-tun mycelium-dnsd; do
            path="/etc/systemd/system/$unit.service"
            [ -f "$path" ] || continue
            act "remove system unit $unit" "$SUDO" systemctl disable --now "$unit.service" || true
            act "remove $path" "$SUDO" rm -f "$path" || true
        done
        act "reload systemd" "$SUDO" systemctl daemon-reload || true
        [ -d "$ROOT_LIBEXEC" ] && act "remove $ROOT_LIBEXEC" "$SUDO" rm -rf "$ROOT_LIBEXEC" || true
        [ -d /run/mycelium ] && act "remove /run/mycelium" "$SUDO" rm -rf /run/mycelium || true
    fi
}

untrust_ca() {
    [ "$(uname -s)" = Darwin ] || return 0
    for ca in "$MYCELIUM_HOME/frontdoor/ca.pem" \
        "$MYCELIUM_HOME/gateway/ca.pem" \
        "$MYCELIUM_HOME/host-web/ca.pem"; do
        [ -f "$ca" ] || continue
        act "remove the trusted frontdoor CA from the login keychain" \
            security remove-trusted-cert "$ca" || warn "could not untrust $ca; remove it in Keychain Access"
    done
}

remove_shell_block() {
    start="# >>> mycelium installer >>>"
    finish="# <<< mycelium installer <<<"
    for rc in "$HOME/.zshrc" "$HOME/.bashrc" "$HOME/.bash_profile" "$HOME/.profile"; do
        [ -f "$rc" ] || continue
        grep -qF "$start" "$rc" 2>/dev/null || continue
        act "remove the Mycelium block from $rc" awk \
            -v start="$start" -v finish="$finish" \
            '$0 == start { skip = 1; next } $0 == finish { skip = 0; next } !skip' \
            "$rc" >"$rc.mycelium-tmp"
        if [ "$DRY_RUN" != 1 ]; then
            cp -p "$rc" "$rc.pre-uninstall-$TS"
            mv "$rc.mycelium-tmp" "$rc"
        else
            rm -f "$rc.mycelium-tmp"
        fi
    done
}

remove_skills() {
    for root in "$HOME/.agents/skills" "$HOME/.claude/skills" "$HOME/.codex/skills"; do
        [ -d "$root" ] || continue
        for name in mycelium-network-operator mycelium-fleet-security mycelium-driver-development mycelium; do
            [ -d "$root/$name" ] || continue
            act "remove skill $root/$name" rm -rf "$root/$name"
        done
    done
}

remove_binaries() {
    [ "$KEEP_BINARIES" = 1 ] && { note "keeping binaries (--keep-binaries)"; return 0; }
    for dir in "$LOCAL_BIN" "$BIN_DIR"; do
        [ -d "$dir" ] || continue
        for name in mycelium myceliumd mycelium-frontdoor mycelium-ingress mycelium-dns-resolver \
            mycelium-canary geoded iroh-tun soil fpl-bob fungos-vfs fungos-vfs-provider \
            fungos-vfs-fetch fungos-vfs-fuse terminal-host; do
            [ -e "$dir/$name" ] || continue
            act "remove $dir/$name" rm -f "$dir/$name"
        done
        # Backups made by install.sh are this installer's artefacts too, but
        # only ones that clearly belong to a replaced binary.
        for backup in "$dir"/*.pre-*; do
            [ -e "$backup" ] || continue
            act "remove $backup" rm -f "$backup"
        done
    done
    # The canary launcher is written into the login PATH, outside the canary
    # prefix, so it is named explicitly rather than swept.
    launcher="$HOME/.local/bin/mycelium-canary"
    [ -e "$launcher" ] && act "remove $launcher" rm -f "$launcher" || true
}

# A path is only removable when it is one of the directories this product
# created. A typo in --home must not become `rm -rf $HOME`.
safe_path() {
    case "$1" in
        ""|/|/Users|/home|"$HOME") return 1 ;;
    esac
    case "$1" in
        "$HOME"/*) return 0 ;;
        /var/*|/usr/local/*|/etc/*) return 0 ;;
    esac
    return 1
}

purge_state() {
    [ "$PURGE" = 1 ] || {
        note "state kept: $MYCELIUM_HOME (re-run with --purge to remove it)"
        return 0
    }
    archive=
    if [ "$NO_BACKUP" != 1 ] && [ -d "$MYCELIUM_HOME" ] && [ "$DRY_RUN" != 1 ]; then
        archive="$HOME/mycelium-uninstall-$TS.tar.gz"
        (umask 077; tar -czf "$archive" -C "$(dirname "$MYCELIUM_HOME")" "$(basename "$MYCELIUM_HOME")") \
            || { archive=; warn "could not archive state; not removing it (pass --no-backup to remove anyway)"; return 1; }
        note "archived state to $archive (contains keys - keep it private)"
    elif [ "$DRY_RUN" = 1 ] && [ -d "$MYCELIUM_HOME" ]; then
        note "would archive $MYCELIUM_HOME, then remove it"
    fi
    for path in "$MYCELIUM_HOME" "$HOME/.geode" "$HOME/.local/share/fungos" \
        "$HOME/Library/Application Support/fungOS" "$HOME/Library/Logs/fungOS" \
        "$HOME/.cache/mycelium"; do
        [ -d "$path" ] || continue
        if ! safe_path "$path"; then
            warn "refusing to remove $path; it is not a directory this installer wrote"
            continue
        fi
        act "remove $path" rm -rf "$path"
    done
    if [ "$(uname -s)" = Darwin ] && [ -d "$HOME/Applications/fungOS.app" ]; then
        act "remove $HOME/Applications/fungOS.app" rm -rf "$HOME/Applications/fungOS.app"
    fi
    if [ "$(uname -s)" = Darwin ] && [ -d "$HOME/Applications/Mycelium.app" ]; then
        act "remove $HOME/Applications/Mycelium.app" rm -rf "$HOME/Applications/Mycelium.app"
    fi
}

confirm() {
    [ "$YES" = 1 ] && return 0
    [ "$DRY_RUN" = 1 ] && return 0
    if ! have_tty; then
        die "no terminal for prompts; pass --yes to uninstall"
    fi
    what="services, binaries and shell integration"
    [ "$PURGE" = 1 ] && what="$what, plus state and application data"
    ask_yes "Remove the Mycelium $what?" no \
        || die "aborted"
}

main() {
    parse_args "$@"
    say "Mycelium uninstall"
    note "home: $MYCELIUM_HOME"
    note "binaries: $LOCAL_BIN and $BIN_DIR"
    confirm
    component_uninstall
    sweep_user_services
    if [ "$SYSTEM_WIRING" = 1 ]; then
        if get_root; then
            sweep_system_services
        fi
    else
        note "system-wide DNS and tunnel wiring left alone (custom home)"
    fi
    untrust_ca
    remove_shell_block
    remove_skills
    remove_binaries
    purge_state
    say "Uninstall complete"
    if [ "$PURGE" != 1 ]; then
        note "the tunnel key, node identity and CA in $MYCELIUM_HOME were kept"
    fi
}

main "$@"
