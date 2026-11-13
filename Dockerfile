# syntax=docker/dockerfile:1
# OpenBazaar home node: ob-node with remote access and the web shop.
#
#   docker build -t ob-node .                      # from source (the default)
#   docker build --target release -t ob-node .     # from release binaries in dist/ (CI)
#
# packaging/docker/compose.yml runs it; docs/home-node.md explains pairing.

FROM rust:1-bookworm AS node-build
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
ARG OB_BUILD_COMMIT=unknown
ARG OB_BUILD_DIRTY=false
ARG OB_BUILD_KIND=dev
ENV OB_BUILD_COMMIT=$OB_BUILD_COMMIT OB_BUILD_DIRTY=$OB_BUILD_DIRTY OB_BUILD_KIND=$OB_BUILD_KIND
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p ob-node && cp target/release/ob-node /ob-node

FROM node:22-bookworm-slim AS web-build
WORKDIR /src/apps/mobile
COPY apps/mobile/package.json apps/mobile/package-lock.json ./
RUN npm ci
COPY apps/mobile ./
RUN npm run build

FROM debian:bookworm-slim AS base
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates libssl3 util-linux && rm -rf /var/lib/apt/lists/* \
  && useradd --system --uid 10001 --user-group --home-dir /data --shell /usr/sbin/nologin openbazaar \
  && install -d -m 700 -o openbazaar -g openbazaar /data
# Do not declare VOLUME: Railway rejects the keyword and asks for a Railway
# Volume mounted at /data instead. packaging/docker/compose.yml still mounts
# the named volume there; an image without VOLUME keeps that data.
# Railway mounts that volume as root, so the image starts as root, vendor.sh
# chowns /data, and setpriv drops to openbazaar. Compose sets user 10001 and
# skips the chown.
COPY --chmod=755 packaging/railway/vendor.sh /usr/local/bin/vendor
ENTRYPOINT ["vendor"]
# The node and its gateway listen on loopback inside the container; remote
# devices reach it only through its onion service, so no port is published.
CMD ["serve", "8787", "--remote", "--web-dir", "/usr/share/openbazaar/web", "--data-dir", "/data", "--vault", "passphrase"]

FROM base AS release
ARG TARGETARCH
COPY --chmod=755 dist/linux-${TARGETARCH}/ob-node /usr/local/bin/ob-node
COPY dist/web /usr/share/openbazaar/web

FROM base AS source
COPY --from=node-build /ob-node /usr/local/bin/ob-node
COPY --from=web-build /src/apps/mobile/dist /usr/share/openbazaar/web
