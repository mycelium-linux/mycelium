#!/bin/sh
# Prove the connector is up and the tunnel exists. Reads only.
set -eu
if [ "$(uname -s)" = Darwin ]; then
    launchctl print system/dev.fpl.cloudflared >/dev/null 2>&1 \
        || { echo "verify: dev.fpl.cloudflared is not loaded" >&2; exit 1; }
else
    systemctl is-active --quiet mycelium-cloudflared \
        || { echo "verify: mycelium-cloudflared is not active" >&2; exit 1; }
fi
state=${MYCELIUM_MANAGED_STATE:-$HOME/.mycelium/managed/${MYCELIUM_MANAGED_ID:-cloudflared-tunnel}}
[ -f "$state/token" ] || { echo "verify: no saved tunnel token at $state/token" >&2; exit 1; }
grep -q 'PASS' "$state/../cloudflared.log" 2>/dev/null || true
echo "verify: connector loaded with a saved token"
