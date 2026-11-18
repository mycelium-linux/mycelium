#!/bin/sh
# Remove the connector. The tunnel itself stays in Cloudflare: deleting it is a
# console decision with a DNS side effect, so the runbook says so instead.
set -eu
sudo=sudo
[ "$(id -u)" -eq 0 ] && sudo=
state=${MYCELIUM_MANAGED_STATE:-$HOME/.mycelium/managed/${MYCELIUM_MANAGED_ID:-cloudflared-tunnel}}
if [ "$(uname -s)" = Darwin ]; then
    $sudo launchctl bootout system/dev.fpl.cloudflared >/dev/null 2>&1 || true
    $sudo rm -f /Library/LaunchDaemons/dev.fpl.cloudflared.plist
else
    $sudo systemctl disable --now mycelium-cloudflared >/dev/null 2>&1 || true
    $sudo rm -f /etc/systemd/system/mycelium-cloudflared.service
    $sudo systemctl daemon-reload >/dev/null 2>&1 || true
fi
rm -f "$state/token"
echo "uninstall: connector removed; the Cloudflare tunnel and its DNS record were left in place"
