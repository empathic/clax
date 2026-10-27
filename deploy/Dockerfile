# The clax-fleet image: the server of a Kubernetes deployment of Clax
# (docs/kubernetes.md). Built from the repository root:
#
#   docker build -f deploy/Dockerfile -t clax-fleet:dev .
#
# Three stages: the web UI (web/dist, which the binary embeds), the
# release binary, and a slim runtime that runs it as an unprivileged user.

# The web UI, with the Node and pnpm the repository pins (MODULE.bazel's
# node toolchain, web/package.json's packageManager).
FROM node:22.22.0-bookworm-slim AS web
WORKDIR /src
RUN corepack enable
COPY . .
RUN cd web && pnpm install --frozen-lockfile && cd .. && scripts/build-web.sh

# The binary, on the toolchain rust-toolchain.toml pins.
FROM rust:1.94.0-bookworm AS build
WORKDIR /src
COPY . .
COPY --from=web /src/web/dist web/dist
COPY --from=web /src/web/dist-extension web/dist-extension
RUN cargo build --release --locked -p clax-fleet \
    && install -m 0755 target/release/clax-fleet /clax-fleet

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates tini \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home-dir /var/lib/clax --create-home clax
COPY --from=build /clax-fleet /usr/local/bin/clax-fleet
USER 10001
WORKDIR /var/lib/clax
# 7480: the instances, to clients and to the other processes; 7481: health.
EXPOSE 7480 7481
VOLUME ["/var/lib/clax"]
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/clax-fleet"]
CMD ["serve"]
