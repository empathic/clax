# syntax=docker/dockerfile:1
# The workspace service: ClaxBot's containers for a hosted Clax, which holds
# none itself (README.md here, "The workspace service"). Built from the
# repository root with BuildKit, at the commit the Worker was built from:
#
#   docker build -f deploy/claxbot-workspace/service.Dockerfile \
#     -t clax-workspace-service .
#
# Two stages, the base images pinned as deploy/Dockerfile pins them: the release
# binary, and a slim runtime with `git`, which the service runs to judge and
# commit a change, as an unprivileged user. It holds no toolchain, no repository
# and no secret: every key reaches it through the environment, and
# workspace.toml through its home. It talks to Daytona and GitHub over HTTPS; a
# `kind = "docker"` runtime needs a Docker socket this image does not hold.

FROM rust:1.94.0-bookworm@sha256:365468470075493dc4583f47387001854321c5a8583ea9604b297e67f01c5a4f AS build
WORKDIR /src
ARG TARGETPLATFORM
COPY . .
RUN --mount=type=cache,id=cargo-registry-$TARGETPLATFORM,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=cargo-target-$TARGETPLATFORM,target=/src/target,sharing=locked \
    cargo build --release --locked -p clax-bot-workspace-service \
    && install -m 0755 target/release/clax-bot-workspace-service /clax-bot-workspace-service

# bookworm-slim
FROM debian:bookworm-slim@sha256:7ff6ba31527b9e336162fa061746c366c4b2032f4d2cff2078bea43f78efe3f8
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates git tini \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home-dir /var/lib/clax-workspace --create-home clax
COPY --from=build /clax-bot-workspace-service /usr/local/bin/clax-bot-workspace-service
USER 10001
WORKDIR /var/lib/clax-workspace
# The home: workspace.toml, which the operator puts there, and what must outlive
# the container. Mount something durable on it.
ENV CLAX_WORKSPACE_SERVICE_HOME=/var/lib/clax-workspace
VOLUME ["/var/lib/clax-workspace"]
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/clax-bot-workspace-service"]
