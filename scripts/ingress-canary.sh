#!/usr/bin/env bash
# Measure public ingress while one Cloudflare connector is stopped.
#
# This tests connector loss only. It cannot test origin loss: every connector
# today points at the same static origin. Stop a connector on a host that is not
# the only one, and do not run this against a single-connector deployment.
#
# usage: scripts/ingress-canary.sh <ssh-host> <systemd-unit> [sample-seconds]
set -euo pipefail

host=${1:?ssh host with the connector}
unit=${2:?systemd unit, e.g. cloudflared-fpl-ingress}
seconds=${3:-20}
out=${OUT:-/tmp/ingress-canary-$(date +%s).log}

probe() {
  local sso m mcp elmers
  sso=$(curl -sS -o /dev/null -w '%{http_code}' --max-time 5 \
    https://sso.fpl.dev/.well-known/openid-configuration || echo ERR)
  m=$(curl -sS -o /dev/null -w '%{http_code}' --max-time 5 \
    https://m.fpl.dev/ || echo ERR)
  mcp=$(curl -sS -o /dev/null -w '%{http_code}' --max-time 5 \
    -X POST https://mcp.fpl.dev/mcp -H 'content-type: application/json' \
    --data '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' || echo ERR)
  elmers=$(curl -sS -o /dev/null -w '%{http_code}' --max-time 5 \
    https://elmers.fpl.dev/ || echo ERR)
  printf '%s sso=%s m=%s mcp=%s elmers=%s\n' \
    "$(date -u +%FT%TZ)" "$sso" "$m" "$mcp" "$elmers" | tee -a "$out"
}

restore() {
  ssh -o BatchMode=yes "$host" "systemctl start $unit" >/dev/null 2>&1 || true
}
trap restore EXIT

echo "before" | tee -a "$out"
probe
ssh -o BatchMode=yes "$host" "systemctl stop $unit"
echo "connector $unit stopped on $host" | tee -a "$out"
for _ in $(seq 1 "$seconds"); do probe; sleep 1; done
restore
trap - EXIT
echo "connector restored" | tee -a "$out"
sleep 5
probe
