# Clax

Clax is a local server for HTML artifacts that agents publish. It stores every version of each artifact and serves a gallery and viewer in your browser, where people comment on a page and send comments back to the agent that published it.

## Prerequisites

Rust 1.94 (pinned by `rust-toolchain.toml`), Node 22, and `just`.

## Install from source

No release has been published yet, so a clone of this repository is the only
way to install Clax:

```
just web                                  # build the web UI, which the binary embeds
cargo build -p clax-cli                   # builds target/debug/clax
cargo install --path crates/clax-cli      # optional: puts `clax` on PATH (~/.cargo/bin)
```

The Codex plugin finds the checkout's `target/debug/clax` (or
`target/release/clax`) itself. The Claude Code plugin does when Claude Code
runs it from the checkout or `CLAX_SOURCE_DIR` names the checkout; otherwise
use `cargo install` (see [plugins/claude-code/README.md](plugins/claude-code/README.md)).
To run `clax` from a shell without installing it, use `target/debug/clax`.

## Use

```
clax publish index.html --dir site   # publish a directory; prints the artifact URL (--json gives the ID)
clax open <ID>                       # open an artifact in the browser
clax list                            # list artifacts
clax read <ID> --path style.css      # print a published file (--json gives the read tool's result)
clax asset upload <ID> photo.png     # upload assets; prints each asset URL
clax status                          # show whether the daemon is running
clax doctor                          # check the home directory, daemon, database, and stored files
clax doctor --fix                    # also remove stray temp files and stale rows (never live artifacts' rows)
clax doctor --agent codex            # also check each layer of a harness's plugin: claude, codex or pi
clax stop                            # stop the daemon
clax serve --bind 0.0.0.0            # serve on the LAN (stop a running daemon first)
```

The daemon starts automatically on first use. Data lives in `~/.clax`; set `CLAX_HOME` to use a different directory.

A new artifact needs a title: `--title`, or else the page's `<title>`. Updates keep the current title unless `--title` is given.

To serve on the LAN, stop a running daemon first, then run `clax serve --bind 0.0.0.0`.

## Use from an agent

Each harness gets the same twenty-two tools (`publish`, `read`, `list`, `delete`, `open`, `pin`, `unpin`, `asset_upload`, `status`, `comments_read`, `comments_reply`, `comments_resolve`, `watch`, `wait_for_feedback`, `db_get`, `db_list`, `db_query`, `db_set`, `db_update`, `db_delete`, `db_str_replace`, `db_batch`) and the `clax` skill:

- Claude Code: `/plugin marketplace add /path/to/clax`, then `/plugin install clax@clax`. See [plugins/claude-code/README.md](plugins/claude-code/README.md).
- Codex: `codex plugin marketplace add /path/to/clax`, then `codex plugin add clax@clax`. See [plugins/clax/README.md](plugins/clax/README.md).
- Pi: `pi install /absolute/path/to/clax/plugins/pi`. See [plugins/pi/README.md](plugins/pi/README.md).

Build the binary first (`cargo build -p clax-cli`, above). The Claude Code and Codex plugins run it through `scripts/ensure-clax.sh`, which uses `CLAX_BIN`, else `clax` on `PATH`, else `~/.local/bin/clax` or `~/.clax/bin/clax`, else the source checkout's `target/release/clax` or `target/debug/clax`, whichever is newer. Downloading a release is its last resort, and is not available until the first release is published. The Pi extension needs `clax` on `PATH` (`cargo install --path crates/clax-cli`) or `CLAX_BIN`. When a plugin half works, `clax doctor --agent <claude|codex|pi>` says which layer failed, and `~/.clax/logs/hooks.log` has a line for every hook run.

## Documentation

[docs/contract.md](docs/contract.md) is the contract for agents and integrators: every tool's arguments, results and error codes, how sessions are identified per harness, the page contract, the security model, and what is not yet available.

## Upgrading

Pages viewed before this version may still be cached by the browser (older daemons marked supporting HTML pages immutable, and Chrome can keep serving such a copy inside the artifact frame even after "Clear site data"). After upgrading from such a daemon, clear "Cached images and files" for "All time" once, at chrome://settings/clearBrowserData. From this version on, every HTML page is revalidated on each load and the bridge URL names its version, so upgrades never need this.

## Development

- `just help` (or bare `just`) lists every recipe with a description.
- `just dev` runs an auto-reloading server on port 7480 and can be left running. A Rust change rebuilds and restarts the daemon; a web change rebuilds `web/dist` (reload the browser to see it). Extra arguments go to `serve`, for example `just dev --bind 0.0.0.0`. Ctrl-C stops everything. A daemon already on port 7480 must be stopped first (`just stop`). It needs `cargo-watch` (`cargo install cargo-watch`).
  - It serves the home in `CLAX_HOME` (default `~/.clax`, the one your agents' plugins use) and prints which at start. Run `CLAX_HOME=<scratch dir> just dev` to keep development off the real home.
  - While a Rust change rebuilds, no daemon is running. A plugin's MCP server or the Pi extension that calls the same home in that gap starts its own daemon from its own binary; when the dev daemon comes back the two contend for `daemon.json` and one of them exits, often the dev daemon. A scratch `CLAX_HOME` avoids this.
- `just check` formats the Rust code, then runs every quality gate.
- `just ci` runs the same gates CI runs, without formatting.

`scripts/quality_gates.sh` runs every check CI runs: the justfile, installer, and plugin structure tests (`scripts/test-plugins.sh`, which also checks that the three skill copies and `docs/contract.md` share the page contract word for word, that the skill copies share the comment loop word for word, that the plugins wire their Stop and prompt hooks, that the workspace, both plugin manifests, the Pi package and the installer's `MIN_VERSION` carry one version, that the Rust and Pi tool descriptions match `plugins/pi/test/fixtures/contract.json`, and, through `scripts/sync-skill-tools.py --check`, that each skill's generated tool block and the tool lists in `docs/contract.md` and the READMEs name exactly that fixture's tools), the web lint (`oxlint`, configured in `web/.oxlintrc.json`), web typecheck and unit tests, the web build (before the Rust gates, which serve or embed it), `cargo fmt`, clippy with `-D warnings`, `cargo check` without test features, `cargo test`, the comment-loop smoke (`scripts/smoke-comment-loop.sh`: a scripted Claude Code session, no model, through tiers 1, 2, 4 and 5 with a fake `codex`), the Pi extension's typecheck and tests, and the Playwright end-to-end tests. `just web-test` runs the web lint, typecheck, and unit tests.

## Security model

Writes, and reads of the session list (working directories, process IDs, harness session IDs), need the token in `~/.clax/daemon.json` (mode 0600, served only to localhost browsers), so LAN viewers can read artifacts and comment on them but not publish or change them. Every `/api` route answers only to a `Host` of `localhost`, `127.0.0.1`, `[::1]`, or the exact IP and port the connection arrived on (403 `forbidden_host` otherwise), which defeats DNS rebinding. The comment routes need no token but refuse requests from another origin, including published pages. The viewer cookie never leaves the daemon; viewers are named by a public ID, comment threads carry no session IDs, and the daemon's `codex` path is served only with the token. Published content is isolated on `<id>.localhost` origins or sandboxed. Content and asset URLs are fetchable by anyone who knows the unguessable ID.

Design: [docs/superpowers/specs/2026-09-28-clax-design.md](docs/superpowers/specs/2026-09-28-clax-design.md)

Runtime capabilities arrive in a later phase.
