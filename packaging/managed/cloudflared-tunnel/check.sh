#!/bin/sh
# Is this host ready to be a public ingress? No writes.
set -eu
fail() { echo "check: $*" >&2; exit 1; }

[ -n "${MYCELIUM_MANAGED_DIR:-}" ] || fail "run me through: mycelium managed plan cloudflared-tunnel"

command -v cloudflared >/dev/null 2>&1 \
    || fail "cloudflared is not installed; the runbook names the one-line install"

# A laptop should not be the public entrance to a fleet: the edge design keeps
# battery-powered and wireless hosts out for a reason.
if command -v pmset >/dev/null 2>&1; then
    pmset -g batt 2>/dev/null | grep -qi 'Battery Power' \
        && fail "on battery: public ingress belongs on a host that stays up"
fi

# The ingress the connector forwards to must already answer on loopback.
command -v curl >/dev/null 2>&1 || fail "curl is required to check the local ingress"
curl -fsS -o /dev/null --max-time 3 http://127.0.0.1:80/healthz 2>/dev/null \
    || echo "check: warning: the loopback ingress did not answer on :80 yet" >&2

[ -n "${CLOUDFLARE_TUNNEL_TOKEN:-}" ] \
    || echo "check: warning: no CLOUDFLARE_TUNNEL_TOKEN; install expects it from geode" >&2
echo "check: ok"
