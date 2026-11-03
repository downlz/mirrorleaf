# syntax=docker/dockerfile:1
# The ledger's image (implementation plan M3). Build from the repository root:
# `docker compose up -d --build` in the ledger folder does this.

FROM rust:1.99-slim-trixie AS build
WORKDIR /src
COPY . .
# The ledger's size doesn't matter, so skip the slow link-time optimisation
# the app builds use.
ENV CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p mirrorleaf-ledger \
    && cp target/release/mirrorleaf-ledger /usr/local/bin/

FROM debian:13-slim
# CA certificates for HTTPS to FCM.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /usr/local/bin/mirrorleaf-ledger /usr/local/bin/
USER nobody
EXPOSE 8080
ENTRYPOINT ["mirrorleaf-ledger"]
