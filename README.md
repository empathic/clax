# Artifax

Artifax is a local server for HTML artifacts that agents publish. It stores every version of each artifact and serves a gallery and viewer in your browser, where people comment on a page and send comments back to the agent that published it.

## Prerequisites

Rust 1.94 (pinned by `rust-toolchain.toml`), Node 22, and `just`.

## Install from source

No release has been published yet, so a clone of this repository is the only
way to install Artifax:

```
just web                                  # build the web UI, which the binary embeds
cargo build -p artifax-cli                # builds target/debug/artifax
cargo install --path crates/artifax-cli   # optional: puts `artifax` on PATH (~/.cargo/bin)
```

The Codex plugin finds the checkout's `target/debug/artifax` (or
`target/release/artifax`) itself. The Claude Code plugin does when Claude Code
runs it from the checkout or `ARTIFAX_SOURCE_DIR` names the checkout; otherwise
use `cargo install` (see [plugins/claude-code/README.md](plugins/claude-code/README.md)).
To run `artifax` from a shell without installing it, use `target/debug/artifax`.

## Use

```
artifax publish index.html --dir site   # publish a directory; prints the artifact URL (--json gives the ID)
artifax open <ID>                       # open an artifact in the browser
artifax list                            # list artifacts
artifax read <ID> --path style.css      # print a published file (--json gives the read tool's result)
artifax asset upload <ID> photo.png     # upload assets; prints each asset URL
artifax status                          # show whether the daemon is running
artifax doctor                          # check the home directory, daemon, database, and stored files
artifax doctor --fix                    # also remove stray temp files and stale rows (never live artifacts' rows)
artifax doctor --agent codex            # also check each layer of a harness's plugin: claude, codex or pi
artifax stop                            # stop the daemon
artifax serve --bind 0.0.0.0            # serve on the LAN (stop a running daemon first)
```

The daemon starts automatically on first use. Data lives in `~/.artifax`; set `ARTIFAX_HOME` to use a different directory.

A new artifact needs a title: `--title`, or else the page's `<title>`. Updates keep the current title unless `--title` is given.

To serve on the LAN, stop a running daemon first, then run `artifax serve --bind 0.0.0.0`.

## Use from an agent

Each harness gets the same twenty-two tools (`publish`, `read`, `list`, `delete`, `open`, `pin`, `unpin`, `asset_upload`, `status`, `comments_read`, `comments_reply`, `comments_resolve`, `watch`, `wait_for_feedback`, `db_get`, `db_list`, `db_query`, `db_set`, `db_update`, `db_delete`, `db_str_replace`, `db_batch`) and the `artifax` skill:

- Claude Code: `/plugin marketplace add /path/to/artifax`, then `/plugin install artifax@artifax`. See [plugins/claude-code/README.md](plugins/claude-code/README.md).
- Codex: `codex plugin marketplace add /path/to/artifax`, then `codex plugin add artifax@artifax`. See [plugins/artifax/README.md](plugins/artifax/README.md).
- Pi: `pi install /absolute/path/to/artifax/plugins/pi`. See [plugins/pi/README.md](plugins/pi/README.md).

Build the binary first (`cargo build -p artifax-cli`, above). The Claude Code and Codex plugins run it through `scripts/ensure-artifax.sh`, which uses `ARTIFAX_BIN`, else `artifax` on `PATH`, else `~/.local/bin/artifax` or `~/.artifax/bin/artifax`, else the source checkout's `target/release/artifax` or `target/debug/artifax`, whichever is newer. Downloading a release is its last resort, and is not available until the first release is published. The Pi extension needs `artifax` on `PATH` (`cargo install --path crates/artifax-cli`) or `ARTIFAX_BIN`. When a plugin half works, `artifax doctor --agent <claude|codex|pi>` says which layer failed, and `~/.artifax/logs/hooks.log` has a line for every hook run.

## Documentation

[docs/contract.md](docs/contract.md) is the contract for agents and integrators: every tool's arguments, results and error codes, how sessions are identified per harness, the page contract, the security model, and what is not yet available.

## Development

- `just help` (or bare `just`) lists every recipe with a description.
- `just dev` runs an auto-reloading server on port 7480 and can be left running. A Rust change rebuilds and restarts the daemon; a web change rebuilds `web/dist` (reload the browser to see it). Extra arguments go to `serve`, for example `just dev --bind 0.0.0.0`. Ctrl-C stops everything. A daemon already on port 7480 must be stopped first (`just stop`). It needs `cargo-watch` (`cargo install cargo-watch`).
  - It serves the home in `ARTIFAX_HOME` (default `~/.artifax`, the one your agents' plugins use) and prints which at start. Run `ARTIFAX_HOME=<scratch dir> just dev` to keep development off the real home.
  - While a Rust change rebuilds, no daemon is running. A plugin's MCP server or the Pi extension that calls the same home in that gap starts its own daemon from its own binary; when the dev daemon comes back the two contend for `daemon.json` and one of them exits, often the dev daemon. A scratch `ARTIFAX_HOME` avoids this.
- `just check` formats the Rust code, then runs every quality gate.
- `just ci` runs the same gates CI runs, without formatting.

`scripts/quality_gates.sh` runs every check CI runs: the justfile, installer, and plugin structure tests (`scripts/test-plugins.sh`, which also checks that the three skill copies and `docs/contract.md` share the page contract word for word, that the skill copies share the comment loop word for word, that the plugins wire their Stop and prompt hooks, that the workspace, both plugin manifests, the Pi package and the installer's `MIN_VERSION` carry one version, that the Rust and Pi tool descriptions match `plugins/pi/test/fixtures/contract.json`, and, through `scripts/sync-skill-tools.py --check`, that each skill's generated tool block and the tool lists in `docs/contract.md` and the READMEs name exactly that fixture's tools), `cargo fmt`, clippy with `-D warnings`, `cargo check` without test features, `cargo test`, the comment-loop smoke (`scripts/smoke-comment-loop.sh`: a scripted Claude Code session, no model, through tiers 1, 2, 4 and 5 with a fake `codex`), the web lint (`oxlint`, configured in `web/.oxlintrc.json`), web typecheck and unit tests, the web build, the Pi extension's typecheck and tests, and the Playwright end-to-end tests. `just web-test` runs the web lint, typecheck, and unit tests.

## Security model

Writes, and reads of the session list (working directories, process IDs, harness session IDs), need the token in `~/.artifax/daemon.json` (mode 0600, served only to localhost browsers), so LAN viewers can read artifacts and comment on them but not publish or change them. Every `/api` route answers only to a `Host` of `localhost`, `127.0.0.1`, `[::1]`, or the exact IP and port the connection arrived on (403 `forbidden_host` otherwise), which defeats DNS rebinding. The comment routes need no token but refuse requests from another origin, including published pages. The viewer cookie never leaves the daemon; viewers are named by a public ID, comment threads carry no session IDs, and the daemon's `codex` path is served only with the token. Published content is isolated on `<id>.localhost` origins or sandboxed. Content and asset URLs are fetchable by anyone who knows the unguessable ID.

Design: [docs/superpowers/specs/2026-09-28-artifax-design.md](docs/superpowers/specs/2026-09-28-artifax-design.md)

Runtime capabilities arrive in a later phase.
