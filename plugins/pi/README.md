# Clax for Pi

Publish HTML pages from [Pi](https://pi.dev) to a local Clax server, view
them in a browser, and get the comments people leave on them back into the
session.

Pi's extension API has no MCP support, so this extension calls the Clax
daemon's REST API directly and registers the tools in process. The tools take
the same arguments and return the same JSON as the Clax MCP tools.

## Install

From a clone of this repository:

```
pi install /absolute/path/to/clax/plugins/pi
```

or, for one run, `pi -e /absolute/path/to/clax/plugins/pi`. The extension
has no runtime dependencies beyond the modules Pi provides to extensions
(`@mariozechner/pi-coding-agent`, `typebox`), so it needs no `npm install`.

The extension needs the `clax` CLI: it uses `$CLAX_BIN` when set, else
`clax` on `PATH`. Install the CLI with `cargo install --path crates/clax-cli`
from the clone (after `just web`, which builds the web UI the binary embeds),
or set `CLAX_BIN` to the clone's `target/debug/clax` after
`cargo build -p clax-cli`. No release has been published yet.
`clax doctor --agent pi` checks the installed package, its skill, and the
daemon's Pi sessions.

## What it adds

- Twenty-two tools: `clax_publish`, `clax_read`, `clax_list`,
  `clax_delete`, `clax_open`, `clax_pin`, `clax_unpin`,
  `clax_asset_upload`, `clax_status`, `clax_comments_read`,
  `clax_comments_reply`, `clax_comments_resolve`, `clax_watch`,
  `clax_wait_for_feedback`, and the data tools `clax_db_get`,
  `clax_db_list`, `clax_db_query`, `clax_db_set`,
  `clax_db_update`, `clax_db_delete`, `clax_db_str_replace`,
  `clax_db_batch`. Pi keeps every tool in one namespace, hence the
  prefix. Relative file paths resolve against the Pi session's working
  directory.
- On session start, the extension finds the daemon for `$CLAX_HOME` (default
  `~/.clax`), starting it with `clax serve` when none is running, and
  registers the Pi session (harness `pi`, Pi's session ID, the working
  directory). Publishes are attributed to that session. When no daemon can be
  started then, the first tool call tries again. If the session is ended under
  the extension, the next tool call registers a new one. On session shutdown
  the session is ended.
- The `clax` skill (`skills/clax/SKILL.md`): when to publish, the page
  contract, and the comment loop.
- The `/clax` command: `/clax open [ID]` opens the gallery or an
  artifact, `/clax list` lists artifacts, `/clax status` shows the daemon
  and session.

## The comment loop

People comment on a page in the browser and send a thread to the agent with
**Send to agent** or `@agent`. Pi has no hooks, so those comments reach the
session in three ways:

- Tool results: every successful `clax_*` call except
  `clax_wait_for_feedback` gets the comments sent since the last call, in
  its JSON `feedback` array and as a trailing text block starting with `---`
  and `[clax] N comments sent to you:`. A failed fetch leaves the result as
  it was.
- `clax_wait_for_feedback`: returns as soon as a comment arrives.
- Follow-up messages: once the session is registered, the extension
  long-polls the daemon (50 s per poll, pausing 5 s after a failed poll or one
  that came back empty in under a second) for comments on artifacts the session watches
  with replies on, and hands each batch to Pi with
  `sendUserMessage(text, {deliverAs: "followUp"})`. When Pi is idle this
  starts a turn at once; while it is working the message waits until the
  current work is done. While the agent is inside
  `clax_wait_for_feedback`, the daemon hands comments to the wait and
  answers these polls empty at once, so the loop pauses. The poll only finds a
  running daemon; it never starts one. It stops on session shutdown.

A comment handed over as a follow-up message counts as seen only once the
agent reads, replies to, or resolves its thread; until then it is resent on a
later tool result after two minutes, at most three times.

Tool failures are reported as error results holding
`{"error": {"code", "message", ...}, "feedback": []}`; when the daemon cannot
be reached the code is `daemon_unreachable` and `log` names
`<CLAX_HOME>/logs/daemon.log`.

## Environment

- `CLAX_HOME`: the Clax home (default `~/.clax`).
- `CLAX_BIN`: the `clax` binary to run; when set it must be executable.
- `CLAX_NO_OPEN`: when set, `open` returns the URL without starting a browser.

## Developing

`just pi-test` installs the dev dependencies and runs the typecheck and the
tests, which start a real daemon with `cargo run`. `scripts/smoke-pi.sh`
(manual) runs `pi -p` with only this extension loaded against scratch Pi and
Clax homes, and checks that the session was registered.
