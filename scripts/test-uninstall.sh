#!/usr/bin/env bash
# Tests for uninstall.sh. Every machine-level command (launchctl, systemctl,
# security, sudo) is stubbed and the state home is a scratch directory, so a
# failing test can never touch the host's real install.
#
# Run: scripts/test-uninstall.sh
set -Eeuo pipefail

ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
WORK=$(mktemp -d "${TMPDIR:-/tmp}/mycelium-uninstall-test.XXXXXX")
trap 'rm -rf "$WORK"' EXIT

fail() { printf 'FAIL: %s\n' "$1" >&2; exit 1; }
ok() { printf 'ok - %s\n' "$1"; }

export HOME=$WORK/home
export MYCELIUM_HOME=$HOME/.mycelium
export MYCELIUM_PREFIX=$HOME/.local
BIN=$MYCELIUM_PREFIX/bin
STUB=$WORK/stub
LOG=$WORK/commands.log
mkdir -p "$HOME" "$BIN" "$STUB" "$HOME/Library/LaunchAgents" "$HOME/.config/systemd/user"

stub() {
    printf '#!/bin/sh\nprintf "%%s\\n" "%s $*" >>"%s"\nexit 0\n' "$1" "$LOG" >"$STUB/$1"
    chmod 755 "$STUB/$1"
}
for command in launchctl systemctl security sudo; do stub "$command"; done
PATH=$STUB:$PATH
export PATH

seed() {
    # The artefacts install.sh and the CLI write on a stable install.
    : >"$LOG"
    printf '<plist/>\n' >"$HOME/Library/LaunchAgents/dev.fpl.mycelium.plist"
    printf '<plist/>\n' >"$HOME/Library/LaunchAgents/dev.fpl.mycelium-frontdoor.plist"
    printf '<plist/>\n' >"$HOME/Library/LaunchAgents/dev.fpl.mycelium-updater.plist"
    printf '<plist/>\n' >"$HOME/Library/LaunchAgents/dev.fungos.soil-store.plist"
    printf '[Service]\n' >"$HOME/.config/systemd/user/mycelium.service"
    printf '#!/bin/sh\nexit 0\n' >"$BIN/mycelium"
    printf 'binary\n' >"$BIN/iroh-tun"
    printf 'binary\n' >"$BIN/geoded"
    cp "$BIN/mycelium" "$BIN/mycelium.pre-20260101T000000"
    mkdir -p "$MYCELIUM_HOME/pki" "$MYCELIUM_HOME/frontdoor"
    printf 'key\n' >"$MYCELIUM_HOME/pki/node-key.pem"
    printf 'ca\n' >"$MYCELIUM_HOME/frontdoor/ca.pem"
    mkdir -p "$HOME/.agents/skills/mycelium-network-operator"
    printf 'skill\n' >"$HOME/.agents/skills/mycelium-network-operator/SKILL.md"
    mkdir -p "$HOME/.agents/skills/someone-elses-skill"
    printf '# >>> mycelium installer >>>\nexport PATH="$HOME/.local/bin:$PATH"\n# <<< mycelium installer <<<\n' >"$HOME/.zshrc"
    printf 'set -o vi\n' >>"$HOME/.zshrc"
}

run_uninstall() {
    "$ROOT/uninstall.sh" --yes "$@" >"$WORK/out.log" 2>&1 \
        || { cat "$WORK/out.log" >&2; fail "uninstall.sh $* exited non-zero"; }
}

seed
run_uninstall --dry-run
[ -f "$HOME/Library/LaunchAgents/dev.fpl.mycelium.plist" ] \
    || fail "a dry run removed a launch agent"
[ -f "$BIN/mycelium" ] || fail "a dry run removed a binary"
[ -f "$MYCELIUM_HOME/pki/node-key.pem" ] || fail "a dry run removed state"
grep -q 'would\|remove' "$WORK/out.log" || fail "a dry run printed no plan"
ok "a dry run changes nothing and prints the plan"

seed
run_uninstall
for label in dev.fpl.mycelium dev.fpl.mycelium-frontdoor dev.fpl.mycelium-updater dev.fungos.soil-store; do
    [ ! -f "$HOME/Library/LaunchAgents/$label.plist" ] || fail "$label survived"
done
[ ! -f "$HOME/.config/systemd/user/mycelium.service" ] || fail "systemd unit survived"
[ ! -f "$BIN/mycelium" ] || fail "mycelium binary survived"
[ ! -f "$BIN/iroh-tun" ] || fail "iroh-tun binary survived"
[ ! -f "$BIN/mycelium.pre-20260101T000000" ] || fail "backup binary survived"
[ ! -d "$HOME/.agents/skills/mycelium-network-operator" ] || fail "bundled skill survived"
[ -d "$HOME/.agents/skills/someone-elses-skill" ] || fail "an unrelated skill was removed"
[ -f "$MYCELIUM_HOME/pki/node-key.pem" ] || fail "state was removed without --purge"
grep -q 'mycelium installer' "$HOME/.zshrc" && fail "shell block survived"
grep -q 'set -o vi' "$HOME/.zshrc" || fail "the rest of .zshrc was lost"
grep -q 'launchctl bootout gui/' "$LOG" || fail "services were not booted out"
ok "removes services, binaries, skills and the shell block, and keeps state"

seed
run_uninstall --purge
[ ! -d "$MYCELIUM_HOME" ] || fail "--purge kept the state directory"
archive=$(ls "$HOME"/mycelium-uninstall-*.tar.gz 2>/dev/null | head -1)
[ -n "$archive" ] || fail "--purge did not archive the state"
tar -tzf "$archive" | grep -q 'pki/node-key.pem' || fail "the archive does not hold the node key"
# GNU stat reads -c, BSD stat reads -f; the other one may succeed with a
# filesystem report, so try the platform's own spelling first.
mode=$(stat -c '%a' "$archive" 2>/dev/null || stat -f '%Lp' "$archive")
[ "$mode" = 600 ] || [ "$mode" = 0600 ] || fail "the archive is not private (mode $mode)"
ok "--purge archives the keys privately, then removes state"

# A custom home is a sandbox or a future channel: it must not reach for the
# machine's own system wiring.
seed
run_uninstall --home "$WORK/other-home"
grep -q 'system-wide DNS and tunnel wiring left alone' "$WORK/out.log" \
    || fail "a custom home still touched the system wiring"
grep -q '/etc/resolver' "$LOG" && fail "a custom home removed the machine resolver"
ok "a custom home leaves system-wide wiring alone"

# Without the CLI, the sweep is the only path. It still has to work.
seed
rm -f "$BIN/mycelium"
run_uninstall
[ ! -f "$HOME/Library/LaunchAgents/dev.fpl.mycelium.plist" ] \
    || fail "the sweep did not remove a launch agent"
grep -q 'no mycelium binary' "$WORK/out.log" || fail "the sweep did not report the fallback"
ok "sweeps service definitions when the CLI is already gone"

# The home path guard is the difference between an uninstall and an accident.
seed
run_uninstall --purge --no-backup --home "$HOME" >/dev/null 2>&1 || true
grep -q 'refusing to remove' "$WORK/out.log" \
    || fail "an uninstall accepted the user's home as a state directory"
ok "refuses to delete the user's home directory"

# The canary install is a separate channel with its own home, prefix and label.
seed
mkdir -p "$HOME/.mycelium-canary/pki" "$HOME/.local/canary/bin"
printf '#!/bin/sh\nexit 0\n' >"$HOME/.local/canary/bin/mycelium"
printf 'canary binary\n' >"$HOME/.local/canary/bin/iroh-tun"
printf '#!/bin/sh\nexit 0\n' >"$HOME/.local/bin/mycelium-canary"
printf '<plist/>\n' >"$HOME/Library/LaunchAgents/dev.fpl.mycelium-canary.plist"
: >"$LOG"
run_uninstall --channel canary
[ ! -f "$HOME/Library/LaunchAgents/dev.fpl.mycelium-canary.plist" ] || fail "the canary launch agent survived --channel canary"
[ ! -f "$HOME/.local/canary/bin/mycelium" ] || fail "the canary binary survived"
[ ! -f "$HOME/.local/bin/mycelium-canary" ] || fail "the canary launcher survived"
[ -d "$HOME/.mycelium-canary" ] || fail "a canary uninstall removed state without --purge"
[ -f "$HOME/Library/LaunchAgents/dev.fpl.mycelium.plist" ] || fail "removing canary also removed the stable launch agent"
[ -f "$BIN/mycelium" ] || fail "removing canary also removed the stable binary"
ok "--channel canary removes only the canary install"

# And --purge on that channel removes its own state, not the stable one.
seed
mkdir -p "$HOME/.mycelium-canary/pki"
printf 'canary key\n' >"$HOME/.mycelium-canary/pki/node-key.pem"
run_uninstall --channel canary --purge
[ ! -d "$HOME/.mycelium-canary" ] || fail "--channel canary --purge kept canary state"
[ -d "$MYCELIUM_HOME" ] || fail "purging canary removed the stable state"
ok "--channel canary --purge removes only canary state"

printf 'all uninstall tests passed\n'
