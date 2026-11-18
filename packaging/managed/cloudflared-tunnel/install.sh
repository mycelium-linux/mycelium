#!/bin/sh
# Install cloudflared as a root service that dials out to Cloudflare and
# forwards to the loopback ingress. The token is the whole credential: it names
# the tunnel, so this script never writes a domain or a certificate.
set -eu
id=${MYCELIUM_MANAGED_ID:-cloudflared-tunnel}
state=${MYCELIUM_MANAGED_STATE:-$HOME/.mycelium/managed/$id}
sudo=sudo
[ "$(id -u)" -eq 0 ] && sudo=

command -v cloudflared >/dev/null 2>&1 || {
    if command -v brew >/dev/null 2>&1; then
        brew install cloudflared
    else
        echo "install: cloudflared is missing and Homebrew is not available" >&2
        exit 1
    fi
}

[ -n "${CLOUDFLARE_TUNNEL_TOKEN:-}" ] || {
    echo "install: set CLOUDFLARE_TUNNEL_TOKEN (geode: cloudflare.tunnel-token)" >&2
    exit 1
}

mkdir -p "$state"
umask 077
printf '%s\n' "$CLOUDFLARE_TUNNEL_TOKEN" >"$state/token"
chmod 600 "$state/token"

if [ "$(uname -s)" = Darwin ]; then
    plist=/Library/LaunchDaemons/dev.fpl.cloudflared.plist
    $sudo tee "$plist" >/dev/null <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>dev.fpl.cloudflared</string>
<key>ProgramArguments</key><array>
<string>/usr/local/bin/cloudflared</string><string>tunnel</string>
<string>--no-autoupdate</string><string>run</string>
<string>--token-file</string><string>$state/token</string>
</array>
<key>RunAtLoad</key><true/><key>KeepAlive</key><true/>
<key>ThrottleInterval</key><integer>10</integer>
<key>StandardOutPath</key><string>/var/log/mycelium-cloudflared.log</string>
<key>StandardErrorPath</key><string>/var/log/mycelium-cloudflared.log</string>
</dict></plist>
PLIST
    $sudo launchctl bootout system/dev.fpl.cloudflared >/dev/null 2>&1 || true
    $sudo launchctl bootstrap system "$plist"
else
    unit=/etc/systemd/system/mycelium-cloudflared.service
    $sudo tee "$unit" >/dev/null <<UNIT
[Unit]
Description=Cloudflare tunnel to the Mycelium loopback ingress
After=network-online.target

[Service]
ExecStart=/usr/local/bin/cloudflared tunnel --no-autoupdate run --token-file $state/token
Restart=always
RestartSec=10
User=root

[Install]
WantedBy=multi-user.target
UNIT
    $sudo systemctl daemon-reload
    $sudo systemctl enable --now mycelium-cloudflared
fi

echo "install: cloudflared is running against the loopback ingress"
echo "install: point the tunnel's public hostname at http://127.0.0.1:80 (see runbook.md)"
