# syntax=docker/dockerfile:1
# The clax-fleet image: the server of a Kubernetes deployment of Clax
# (docs/kubernetes.md). Built from the repository root with BuildKit:
#
#   docker build -f deploy/Dockerfile -t <registry>/clax-fleet:<tag> .
#
# Three stages: the web UI (web/dist, which the binary embeds), the release
# binary, and a slim runtime that runs it as an unprivileged user. Base
# images are pinned by digest (the tag beside each says which); the web
# dependencies install in a layer of their own, and cargo's registry and
# target directory, and pnpm's store, are cache mounts, so a source edit
# rebuilds only what it changes.

# The web UI, with the Node and pnpm the repository pins (MODULE.bazel's
# node toolchain, web/package.json's packageManager).
FROM node:22.22.0-bookworm-slim@sha256:dd9d21971ec4395903fa6143c2b9267d048ae01ca6d3ea96f16cb30df6187d94 AS web
WORKDIR /src
RUN corepack enable
COPY web/package.json web/pnpm-lock.yaml web/pnpm-workspace.yaml web/
RUN --mount=type=cache,target=/root/.local/share/pnpm/store \
    cd web && pnpm install --frozen-lockfile
COPY . .
RUN scripts/build-web.sh

# The binary, on the toolchain rust-toolchain.toml pins.
FROM rust:1.94.0-bookworm@sha256:365468470075493dc4583f47387001854321c5a8583ea9604b297e67f01c5a4f AS build
WORKDIR /src
COPY . .
COPY --from=web /src/web/dist web/dist
COPY --from=web /src/web/dist-extension web/dist-extension
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p clax-fleet \
    && install -m 0755 target/release/clax-fleet /clax-fleet

# bookworm-slim
FROM debian:bookworm-slim@sha256:7ff6ba31527b9e336162fa061746c366c4b2032f4d2cff2078bea43f78efe3f8
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates tini \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home-dir /var/lib/clax --create-home clax
COPY --from=build /clax-fleet /usr/local/bin/clax-fleet
USER 10001
WORKDIR /var/lib/clax
# 7480: the instances, to clients and to the other processes; 7481: health,
# status and metrics.
EXPOSE 7480 7481
VOLUME ["/var/lib/clax"]
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/clax-fleet"]
CMD ["serve"]
