# Artifax

Artifax is a local server for HTML artifacts that agents publish. It stores every version of each artifact and serves a gallery and viewer in your browser.

## Prerequisites

Rust 1.94 (pinned by `rust-toolchain.toml`), Node 22, and `just`.

## Install from source

```
just web
cargo install --path crates/artifax-cli
```

## Use

```
artifax publish index.html --dir site   # publish a directory; prints the artifact URL (--json gives the ID)
artifax open <id>                       # open an artifact in the browser
artifax list                            # list artifacts
artifax status                          # show whether the daemon is running
artifax doctor                          # check the home directory, daemon, and database
artifax stop                            # stop the daemon
artifax serve --bind 0.0.0.0            # serve on the LAN (stop a running daemon first)
```

The daemon starts automatically on first use. Data lives in `~/.artifax`; set `ARTIFAX_HOME` to use a different directory.

To serve on the LAN, stop a running daemon first, then run `artifax serve --bind 0.0.0.0`.

## Security model

Writes need the token in `~/.artifax/daemon.json` (mode 0600, served only to localhost browsers), so LAN viewers can only read. Published content is isolated on `<id>.localhost` origins or sandboxed. Content and asset URLs are fetchable by anyone who knows the unguessable ID.

Design: [docs/superpowers/specs/2026-09-28-artifax-design.md](docs/superpowers/specs/2026-09-28-artifax-design.md)

Comments, agent plugins, and runtime capabilities arrive in later phases.
