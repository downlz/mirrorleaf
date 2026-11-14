# syntax=docker/dockerfile:1
# The relay's image (M13, E10). Build from the repository root:
# `docker compose up -d --build` in the ledger folder does this.

FROM rust:1.99-slim-trixie AS build
WORKDIR /src
COPY . .
# The relay's size doesn't matter, so skip the slow link-time optimisation
# the app builds use.
ENV CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16
# The caches are locked: the ledger's and the relay's images build one at a
# time when Compose builds both.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/src/target,sharing=locked \
    cargo build --release --locked -p mirrorleaf-relay \
    && cp target/release/mirrorleaf-relay /usr/local/bin/

FROM debian:13-slim
# CA certificates: the HTTP client that asks the ledger won't start without
# them, though it speaks plain HTTP over the server's own network.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /usr/local/bin/mirrorleaf-relay /usr/local/bin/
# Where Compose mounts the certificates (Caddy's data, or certbot's folder):
# made here, as the container's own files are read-only.
RUN mkdir /certs
# Compose runs it as root, without capabilities, to read Caddy's certificate
# key; on its own it runs as nobody.
USER nobody
EXPOSE 3340 7842/udp 9090
ENTRYPOINT ["mirrorleaf-relay"]
