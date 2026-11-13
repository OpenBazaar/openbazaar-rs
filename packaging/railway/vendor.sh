#!/bin/sh
# Start the home node. Railway volumes mount owned by root; hand /data to the
# node user, then drop privileges. A compose file that already runs as
# openbazaar skips the chown.
set -eu

if [ "$(id -u)" = 0 ]; then
  install -d -m 700 -o openbazaar -g openbazaar /data
  chown -R openbazaar:openbazaar /data
  exec setpriv --reuid openbazaar --regid openbazaar --init-groups /usr/local/bin/ob-node "$@"
fi

exec /usr/local/bin/ob-node "$@"
