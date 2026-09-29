# Artifax for Pi

Publish HTML pages from [Pi](https://pi.dev) to a local Artifax server, view
them in a browser, and get the comments people leave on them back into the
session.

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
from the clone, or from a release. A release's `.sha256` file comes from the
same place as its tarball: the checksum protects integrity, not authenticity.

## What it adds

- Tools `artifax_publish`, `artifax_read`, `artifax_list`, `artifax_delete`,
  `artifax_open`, `artifax_pin`, `artifax_unpin`, `artifax_asset_upload`,
  `artifax_status`, `artifax_comments_read`, `artifax_comments_reply`,
  `artifax_comments_resolve`, `artifax_watch`, and
  `artifax_wait_for_feedback`. Pi keeps every tool in one namespace, hence the
  prefix. Relative file paths resolve against the Pi session's working
  directory.
- On session start, the extension finds the daemon for `$ARTIFAX_HOME` (default
  `~/.artifax`), starting it with `artifax serve` when none is running, and
  registers the Pi session (harness `pi`, Pi's session ID, the working
  directory). Publishes are attributed to that session. When no daemon can be
  started then, the first tool call tries again. If the session is ended under
  the extension, the next tool call registers a new one. On session shutdown
  the session is ended.
- The `artifax` skill (`skills/artifax/SKILL.md`): when to publish, the page
  contract, and the comment loop.
- The `/artifax` command: `/artifax open [ID]` opens the gallery or an
  artifact, `/artifax list` lists artifacts, `/artifax status` shows the daemon
  and session.

## The comment loop

People comment on a page in the browser and send a thread to the agent with
**Send to agent** or `@agent`. Pi has no hooks, so those comments reach the
session in three ways:

- Tool results: every successful `artifax_*` call except
  `artifax_wait_for_feedback` gets the comments sent since the last call, in
  its JSON `feedback` array and as a trailing text block starting with `---`
  and `[artifax] N comments sent to you:`. A failed fetch leaves the result as
  it was.
- `artifax_wait_for_feedback`: returns as soon as a comment arrives.
- Follow-up messages: once the session is registered, the extension
  long-polls the daemon (50 s per poll, pausing 5 s after a failed poll or one
  that came back empty in under a second) for comments on artifacts the session watches
  with replies on, and hands each batch to Pi with
  `sendUserMessage(text, {deliverAs: "followUp"})`. When Pi is idle this
  starts a turn at once; while it is working the message waits until the
  current work is done. The poll only finds a running daemon; it never starts
  one. It stops on session shutdown.

A comment handed over as a follow-up message counts as seen only once the
agent reads, replies to, or resolves its thread; until then it is resent on a
later tool result after two minutes, at most three times.

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
