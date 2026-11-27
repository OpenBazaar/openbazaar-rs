#!/bin/sh
# Start a C tor sidecar and the moderator node (see moderator.Dockerfile).
set -eu

if [ "$(id -u)" = 0 ]; then
  # The volume mounts owned by root. Hand it to the node's user and start again as that user.
  install -d -m 700 -o openbazaar -g openbazaar /data /data/tor-c
  chown -R openbazaar:openbazaar /data
  exec setpriv --reuid openbazaar --regid openbazaar --init-groups "$0" "$@"
fi

# ── start C tor ──────────────────────────────────────────────────────
# One C tor process handles both SOCKS egress (chain queries, rates) and
# hidden-service publishing (admin, store onions).
TOR_DATA=/data/tor-c
install -d -m 700 "$TOR_DATA"
cat > "$TOR_DATA/torrc" <<EOF
SocksPort 127.0.0.1:9050 IsolateSOCKSAuth
ControlPort 127.0.0.1:9051
CookieAuthentication 1
DataDirectory $TOR_DATA
SafeLogging 1
Log err stderr
ClientOnly 1
EOF

tor -f "$TOR_DATA/torrc" --quiet &

# Wait for the control port to become ready (up to 60 s).
waited=0
while [ "$waited" -lt 60 ]; do
  if [ -f "$TOR_DATA/control_auth_cookie" ]; then
    break
  fi
  sleep 1
  waited=$((waited + 1))
done

# With a public domain, the operator's browser signs in at that address
# (OB_WEB_ACCESS_KEY). OB_WEB_ORIGIN names a custom domain instead.
origin="${OB_WEB_ORIGIN:-${RAILWAY_PUBLIC_DOMAIN:+https://$RAILWAY_PUBLIC_DOMAIN}}"
web=""
if [ -n "$origin" ]; then
  web="--web-listen 0.0.0.0:${PORT:-8080} --web-origin $origin"
fi

# $web is two flags and their values, or nothing.
# shellcheck disable=SC2086
exec ob-node serve 8787 --remote --moderate --network testnet4 --data-dir /data \
  --vault passphrase --no-rates --proxy socks5h://127.0.0.1:9050 \
  --web-dir /usr/share/openbazaar/web \
  --external-tor "$TOR_DATA" $web
