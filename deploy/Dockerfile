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
# rebuilds only what it changes. The cargo mounts are keyed by the target
# platform, so a multi-platform build never mixes the architectures'
# artifacts. `--build-arg CLAX_BUILD_COMMIT=<sha>` records the commit the
# fleet's /status reports (the context leaves out .git, so without it the
# commit is `unknown`).

# The web UI, with the Node and pnpm the repository pins (MODULE.bazel's
# node toolchain, web/package.json's packageManager).
FROM node:22.22.0-bookworm-slim@sha256:dd9d21971ec4395903fa6143c2b9267d048ae01ca6d3ea96f16cb30df6187d94 AS web
WORKDIR /src
COPY web/package.json web/pnpm-lock.yaml web/pnpm-workspace.yaml web/
# pnpm at packageManager's version, from npm: the Corepack this Node bundles
# cannot start pnpm 12 (it looks for the bin/pnpm.cjs pnpm 12 no longer has).
RUN npm install --global --no-fund --no-audit \
    "$(node -p 'require("./web/package.json").packageManager')"
RUN --mount=type=cache,target=/root/.local/share/pnpm/store \
    cd web && pnpm install --frozen-lockfile
COPY . .
# `pnpm run build` rather than scripts/build-web.sh: the script keeps
# unchanged files' modification times for incremental cargo builds, which a
# fresh layer has no use for.
RUN cd web && pnpm run build

# The binary, on the toolchain rust-toolchain.toml pins.
FROM rust:1.94.0-bookworm@sha256:365468470075493dc4583f47387001854321c5a8583ea9604b297e67f01c5a4f AS build
WORKDIR /src
ARG TARGETPLATFORM
ARG CLAX_BUILD_COMMIT
COPY . .
COPY --from=web /src/web/dist web/dist
COPY --from=web /src/web/dist-extension web/dist-extension
RUN --mount=type=cache,id=cargo-registry-$TARGETPLATFORM,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=cargo-target-$TARGETPLATFORM,target=/src/target,sharing=locked \
    CLAX_BUILD_COMMIT="$CLAX_BUILD_COMMIT" cargo build --release --locked -p clax-fleet \
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
