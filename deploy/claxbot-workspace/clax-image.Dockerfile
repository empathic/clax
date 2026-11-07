# The container clax.toml's `image` names: what ClaxBot needs to build and test
# Clax. Versions come from the repository: .bazelversion, rust-toolchain.toml,
# web/package.json and preview.yml (Node 26). Build it from this directory:
#
#   docker build -t clax-workspace -f clax-image.Dockerfile .
#
# Daytona runs it as a snapshot, x86-64 and sized there: arm64 fails on the
# Android NDK. What a cold build takes is in README.md, "A snapshot on Daytona".
# `image` then names the snapshot (README.md, "A snapshot on Daytona"):
#
#   docker build --platform linux/amd64 -t clax-workspace:1 -f clax-image.Dockerfile .
#   daytona snapshot push clax-workspace:1 --name clax-workspace-1 --cpu 4 --memory 8 --disk 10
#
# It holds toolchains and no repository, no Bazel output and no secret. A run
# clones into /tmp/clax-repo as the unprivileged user below: the runtime drops
# every capability, so a container installs nothing and owns only its home
# and /tmp.
# Debian 13 and not Ubuntu 24.04: clax-core's gitctx tests need git 2.44 or later
# (crates/clax-core/src/gitctx_capture_tests.rs), and 24.04 ships 2.43.
FROM debian:trixie

ARG TARGETARCH
ARG BAZELISK_VERSION=1.30.0
ARG BAZEL_VERSION=9.3.0
ARG RUST_VERSION=1.94.0
ARG NODE_VERSION=26.13.0
ARG PNPM_VERSION=12.3.4
ARG JUST_VERSION=1.59.0

# build-essential is the C toolchain Bazel finds for cc_ rules and for the C in
# Rust crates; clang, lld and llvm build the wasm32 Worker. The rest are what a
# build and an agent's shell call: `unzip` and `zip` for Bazel's archives,
# `ripgrep` for reading a repository of this size.
RUN apt-get update \
 && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      build-essential ca-certificates clang curl git jq less lld llvm \
      pkg-config procps python3 python3-venv ripgrep unzip xz-utils zip \
 && rm -rf /var/lib/apt/lists/*

# A repository a run clones from a mounted path belongs to another user.
RUN git config --system --add safe.directory '*' \
 && useradd --create-home --uid 1000 --shell /bin/bash clax

ENV BAZELISK_HOME=/opt/bazelisk \
    RUSTUP_HOME=/opt/rustup \
    CARGO_HOME=/opt/cargo \
    PATH=/opt/cargo/bin:/opt/node/bin:/usr/local/bin:/usr/bin:/bin

# Bazelisk, with Bazel itself, so that a run does not fetch 60 MB before its
# first command. A bazel on PATH is what scripts/run.sh looks for next.
RUN case "$TARGETARCH" in amd64) a=amd64 ;; arm64) a=arm64 ;; *) exit 1 ;; esac \
 && curl -fsSL -o /usr/local/bin/bazelisk \
      "https://github.com/bazelbuild/bazelisk/releases/download/v${BAZELISK_VERSION}/bazelisk-linux-${a}" \
 && chmod 0755 /usr/local/bin/bazelisk \
 && ln -s bazelisk /usr/local/bin/bazel \
 && USE_BAZEL_VERSION="$BAZEL_VERSION" bazelisk version \
 && chown -R clax "$BAZELISK_HOME"

# Rust, for `cargo fmt` and what else a person runs by hand: Bazel fetches its
# own rustc by checksum (MODULE.bazel). All of rust-toolchain.toml's targets, or
# rustup would fetch the rest at the first `cargo` in the repository.
RUN curl -fsSL https://sh.rustup.rs | sh -s -- -y --no-modify-path --profile minimal \
      --default-toolchain "$RUST_VERSION" \
      --component rustfmt --component clippy \
      --target wasm32-unknown-unknown --target aarch64-apple-ios --target aarch64-apple-ios-sim \
 && chmod -R a+rX "$RUSTUP_HOME" "$CARGO_HOME"

RUN case "$TARGETARCH" in amd64) a=x64 ;; arm64) a=arm64 ;; *) exit 1 ;; esac \
 && mkdir /opt/node \
 && curl -fsSL "https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-${a}.tar.xz" \
    | tar -xJ --strip-components=1 -C /opt/node \
 && npm install --global "pnpm@${PNPM_VERSION}"

RUN case "$TARGETARCH" in amd64) a=x86_64 ;; arm64) a=aarch64 ;; *) exit 1 ;; esac \
 && curl -fsSL "https://github.com/casey/just/releases/download/${JUST_VERSION}/just-${JUST_VERSION}-${a}-unknown-linux-musl.tar.gz" \
    | tar -xz -C /usr/local/bin just

# `docker exec`, and so every command of a run, takes this user and its home
# from here.
USER clax
WORKDIR /home/clax
