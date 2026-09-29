# Artifax for Pi

Publish HTML pages from [Pi](https://pi.dev) to a local Artifax server, view
them in a browser, and (from phase 3) get comments back.

Pi's extension API has no MCP support, so this extension calls the Artifax
daemon's REST API directly and registers the tools in process. The tools take
the same arguments and return the same JSON as the Artifax MCP tools.

## Install

From a clone of this repository:

```
pi install /absolute/path/to/artifax/plugins/pi
```

or, for one run, `pi -e /absolute/path/to/artifax/plugins/pi`. The extension
has no runtime dependencies beyond the modules Pi provides to extensions
(`@mariozechner/pi-coding-agent`, `typebox`), so it needs no `npm install`.

The extension needs the `artifax` CLI: it uses `$ARTIFAX_BIN` when set, else
`artifax` on `PATH`. Install the CLI with `cargo install --path crates/artifax-cli`
from the clone, or from a release.

## What it adds

- Tools `artifax_publish`, `artifax_read`, `artifax_list`, `artifax_delete`,
  `artifax_open`, `artifax_pin`, `artifax_unpin`, `artifax_asset_upload`, and
  `artifax_status`. Pi keeps every tool in one namespace, hence the prefix.
  Relative file paths resolve against the Pi session's working directory.
- On session start, the extension finds the daemon for `$ARTIFAX_HOME` (default
  `~/.artifax`), starting it with `artifax serve` when none is running, and
  registers the Pi session (harness `pi`, Pi's session ID, the working
  directory). Publishes are attributed to that session. When no daemon can be
  started then, the first tool call tries again. On session shutdown the session
  is ended.
- The `artifax` skill (`skills/artifax/SKILL.md`): when to publish and the
  page contract.
- The `/artifax` command: `/artifax open [id]` opens the gallery or an
  artifact, `/artifax list` lists artifacts, `/artifax status` shows the daemon
  and session.

Tool failures are reported as error results holding
`{"error": {"code", "message", ...}, "feedback": []}`; when the daemon cannot
be reached the code is `daemon_unreachable` and `log` names
`<ARTIFAX_HOME>/logs/daemon.log`.

## Environment

- `ARTIFAX_HOME`: the Artifax home (default `~/.artifax`).
- `ARTIFAX_BIN`: the `artifax` binary to run; when set it must be executable.
- `ARTIFAX_NO_OPEN`: when set, `open` returns the URL without starting a browser.

## Developing

`just pi-test` installs the dev dependencies and runs the typecheck and the
tests, which start a real daemon with `cargo run`. `scripts/smoke-pi.sh`
(manual) runs `pi -p` with only this extension loaded against scratch Pi and
Artifax homes, and checks that the session was registered.
