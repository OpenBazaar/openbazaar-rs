#!/bin/sh
# Start a Tor proxy and the moderator node (see moderator.Dockerfile).
set -eu

if [ "$(id -u)" = 0 ]; then
  # The volume mounts owned by root. Hand it to the node's user and start again as that user.
  install -d -m 700 -o openbazaar -g openbazaar /data /data/tor-proxy
  chown -R openbazaar:openbazaar /data
  exec setpriv --reuid openbazaar --regid openbazaar --init-groups "$0" "$@"
fi

# Filter peers and public Esplora servers are reached through this proxy, so
# they see a Tor exit and not this host. The node's onions use its own Tor client.
tor --quiet --SocksPort 127.0.0.1:9050 --DataDirectory /data/tor-proxy --Log "err stderr" &

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
  --web-dir /usr/share/openbazaar/web $web
