# syntax=docker/dockerfile:1
# A testnet4 moderator node for a host with no terminal or browser of its own
# (Railway). It answers dispute letters on its store onion. Its operator
# rules on them in a browser at the service's public https address, or from a
# paired phone over its admin onion.
#
# Railway: set RAILWAY_DOCKERFILE_PATH=packaging/railway/moderator.Dockerfile,
# OB_VAULT_PASSPHRASE, and PORT=8080 on the service; attach a volume at /data;
# and give the service a public domain for port 8080. Railway ends TLS and
# sees that traffic. Without a domain the node is reached only over Tor.
# Open the public address as soon as the node is up: a new node belongs to the
# first browser that creates a passkey there. To keep strangers from doing so
# first, or to get back in after losing every passkey, set OB_WEB_ACCESS_KEY
# (obkey- and 52 base32 letters), a key a browser can sign in with instead.
# The build context is the repository root. The image needs only Cargo.toml,
# Cargo.lock, rust-toolchain.toml, crates, vendor, scripts, apps/mobile, and
# packaging/railway from it, so `railway up` can run from a copy of just those.
#
# It differs from the home-node image (/Dockerfile) in four ways: Railway
# refuses the VOLUME keyword, its volumes mount owned by root, the chain data
# needs a Tor proxy, which a home machine supplies itself, and the web page is
# also served on the public address.

FROM rust:1-bookworm AS node-build
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
ARG OB_BUILD_COMMIT=unknown
ARG OB_BUILD_DIRTY=false
ARG OB_BUILD_KIND=dev
ENV OB_BUILD_COMMIT=$OB_BUILD_COMMIT OB_BUILD_DIRTY=$OB_BUILD_DIRTY OB_BUILD_KIND=$OB_BUILD_KIND
WORKDIR /src
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates crates
COPY vendor vendor
COPY scripts scripts
RUN cargo build --release --locked -p ob-node && cp target/release/ob-node /ob-node

FROM node:22-bookworm-slim AS web-build
WORKDIR /src/apps/mobile
COPY apps/mobile/package.json apps/mobile/package-lock.json ./
RUN npm ci
COPY apps/mobile ./
RUN npm run build

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates libssl3 tor && rm -rf /var/lib/apt/lists/* \
  && useradd --system --uid 10001 --user-group --home-dir /data --shell /usr/sbin/nologin openbazaar
COPY --from=node-build /ob-node /usr/local/bin/ob-node
COPY --from=web-build /src/apps/mobile/dist /usr/share/openbazaar/web
COPY --chmod=755 packaging/railway/moderator.sh /usr/local/bin/moderator
# Peers and paired devices reach the node over its onions. The web page
# listens on $PORT only when the service has a public domain (moderator.sh).
CMD ["moderator"]
