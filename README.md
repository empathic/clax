# Artifax

Artifax is a local server for HTML artifacts that agents publish. It stores every version of each artifact and serves a gallery and viewer in your browser.

## Install from source

```
just web
cargo install --path crates/artifax-cli
```

## Use

```
artifax publish index.html --dir site   # publish a directory; prints the artifact ID
artifax open <id>                       # open an artifact in the browser
artifax list                            # list artifacts
artifax serve --bind 0.0.0.0            # serve on the LAN
```

The daemon starts automatically on first use. Data lives in `~/.artifax`.

Design: [docs/superpowers/specs/2026-09-28-artifax-design.md](docs/superpowers/specs/2026-09-28-artifax-design.md)

Comments, agent plugins, and runtime capabilities arrive in later phases.
