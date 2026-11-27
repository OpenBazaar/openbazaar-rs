#!/bin/sh
# Start a C tor sidecar and the vendor node (see /Dockerfile).
set -eu

if [ "$(id -u)" = 0 ]; then
  install -d -m 700 -o openbazaar -g openbazaar /data /data/tor-c
  chown -R openbazaar:openbazaar /data
  exec setpriv --reuid openbazaar --regid openbazaar --init-groups "$0" "$@"
fi

# ── start C tor ──────────────────────────────────────────────────────
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

# ── start ob-node ────────────────────────────────────────────────────
# Strip --tor (embedded Arti SOCKS) from the CMD args — C tor handles it.
# Add --proxy to route chain queries and Lightning through C tor's SOCKS port.
args=""
skip_next=false
for arg in "$@"; do
  if $skip_next; then skip_next=false; continue; fi
  case "$arg" in
    --tor) continue ;;  # drop embedded Arti
    *) args="$args $arg" ;;
  esac
done
# shellcheck disable=SC2086
exec /usr/local/bin/ob-node $args --proxy socks5h://127.0.0.1:9050 --external-tor "$TOR_DATA"
