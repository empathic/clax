# Artifax contract

This document is the contract between Artifax and the agents that publish to
it: the tools, how a harness session is identified, what a published page must
do, what the daemon guarantees about isolation, and what does not exist yet.
The implementation is the authority where the two disagree:
`crates/artifax-mcp/src/tools.rs` (arguments), `crates/artifax-mcp/src/render.rs`
(result shape), `crates/artifax-server/src/routes/` (daemon errors),
`crates/artifax-core/src/store/sessions.rs` (sessions),
`crates/artifax-core/src/feedback.rs` and
`crates/artifax-core/src/store/feedback.rs` (comment delivery),
`crates/artifax-hooks/src/events.rs` (hooks), and `plugins/pi/src/artifax.ts`
(the Pi tools).

## Tools

Twenty-two tools: `publish`, `read`, `list`, `delete`, `open`, `pin`, `unpin`,
`asset_upload`, `status`, the comment tools `comments_read`,
`comments_reply`, `comments_resolve`, `watch`, `wait_for_feedback` (see
"Comments and feedback"), and the data tools `db_get`, `db_list`,
`db_query`, `db_set`, `db_update`, `db_delete`, `db_str_replace`,
`db_batch` (see "Runtime capabilities"). The MCP implementation lives in
`crates/artifax-mcp` and is served two ways:

- the stdio shim `artifax mcp --agent <claude|codex>`, which a harness
  starts once per session and which attributes publishes to that session
  (Pi does not use it; `--agent pi` is a usage error);
- the daemon's `/mcp` endpoint (MCP streamable HTTP, bearer token required),
  which attributes publishes to no session.

Pi's extension API cannot register an MCP server, so `plugins/pi` implements
the same twenty-two tools in TypeScript against the daemon's REST API, with the same
arguments and the same result and error JSON.

Names as the model sees them:

| Harness | Tool name |
|---|---|
| Claude Code, plugin install | `mcp__plugin_artifax_artifax__<tool>` |
| Claude Code, plain `.mcp.json` entry named `artifax` | `mcp__artifax__<tool>` |
| Codex | `mcp__artifax__<tool>` |
| Pi | `artifax_<tool>` |

The command line covers the same operations for scripts and harnesses
without MCP: `artifax publish`, `read`, `list`, `open`, `delete`, `pin`,
`unpin`, `asset upload` and `status`, each with `--json` for one JSON object
on stdout. `artifax read <ID|URL> [--version N] [--path P] [--max-bytes N]`
and `artifax asset upload <ID|URL> <file>...` run the `read` and
`asset_upload` tools, and with `--json` print exactly the tool's result object
(including `feedback`) on one line; a tool error exits 1 with
`error: <code>: <message>` on stderr. Without `--json`, `read` writes the
file's content (no trailing newline added) and `asset upload` prints one asset
URL per line. The other commands' JSON is their own shape, not the tool
result shape below. `artifax publish` takes a new artifact's title from
`--title`, else the page's `<title>`, as the `publish` tool does. `artifax
open` exits 1 with `could not open a browser; open <url> yourself` when the
opener fails (see `open`); `artifax open --json` only prints the URL.

### Results

Every result's first text content block holds a pretty-printed JSON object.
The object always carries a `feedback` array: the comments sent to this
session that the call handed over (tier 1; always `[]` through the daemon's
`/mcp`, which has no session; for `wait_for_feedback`, the comments it waited
for). When it is not empty, a second text block follows: `---`, a newline, and
the payload text described under "Comments and feedback". A success is the
tool's fields plus `feedback`. Error results carry `"feedback": []` and no
second block. A failure is marked as an error
result (`isError: true` over MCP; Pi marks a thrown tool error the same way)
and its text is:

```json
{
  "error": {
    "code": "not_found",
    "message": "not found"
  },
  "feedback": []
}
```

`error` may carry extra fields, named under each code below. A result field
the daemon has no value for is `null`.

The one exception: arguments that fail the tool's schema (a wrong type, a
missing required argument, or an unknown argument, since every schema rejects
unknown fields) are refused before the tool runs. Over MCP that is a
protocol-level invalid-params error, not a tool result, so it has no
`error.code` and no `feedback`. Pi reports it as an error result whose text is
Pi's own validation message (`Validation failed for tool ...`), not JSON.

### Error codes every tool can return

These come from the tool layer itself, not the daemon:

- `daemon_unreachable`: the daemon could not be found, started, or connected
  to. Extra field `log`, the daemon log path (`<ARTIFAX_HOME>/logs/daemon.log`).
- `timeout`: a request to the daemon passed its client-side deadline. Extra
  fields `detail` and `log`. For a publish, the outcome is unknown; read the
  artifact before retrying.
- `bad_response`: the daemon answered with a success status but a body that
  could not be read.
- `internal` (Pi only): an unexpected exception inside the extension.

Any error the daemon returns is passed through unchanged as `error`. The
daemon codes a tool can surface in normal operation:

- `not_found` (404): no such artifact, version, file, or session.
- `timeout` (408): the daemon's own request deadline passed (message
  `request exceeded ...`, no `log` field). On a write the outcome is unknown.
- `unauthorized` (401): the bearer token was refused. The shim and the Pi
  extension re-discover the daemon and retry once, so this surfaces only when
  that retry is refused too.
- `internal`, `corrupt` (500): a storage or database failure; see the daemon
  log. A failed file operation is `internal` with the message
  `storage error: <kind>`, naming the I/O error kind (for example
  `storage error: StorageFull`) and never a path.

### publish

Publishes an HTML page as a new artifact, or as a new version of an existing
one.

Arguments:

| Name | Type | Required | Meaning |
|---|---|---|---|
| `html` | string | exactly one of `html`, `file_path` | The page, published as `index.html`. |
| `file_path` | string | exactly one of `html`, `file_path` | Local HTML file, published as `index.html`. |
| `files` | object: path to file entry or `null` | no | Supporting files by published relative path. `null` removes a file carried forward from the previous version. `index.html` is not allowed as a key. |
| `id` | string | no; at most one of `id`, `url` | Artifact to update. |
| `url` | string | no; at most one of `id`, `url` | Artifact to update, as any artifact URL. |
| `if_version` | integer ≥ 0 | no | The version the update is based on. Defaults to the artifact's current version. Ignored when creating. |
| `title` | string | on create, unless the page has a `<title>` | Artifact title. Creating without it takes the page's `<title>` (below). Updates never need it. |
| `description` | string | no | One-line description. |
| `icon` | string | no | One generic word, such as `chart` or `map`. |
| `label` | string | no | Short name for this version, at most 60 characters. |
| `capabilities` | object | no | Stored with the artifact; no effect until phase 4. |

On an update, an omitted `title`, `description`, `icon` or `capabilities`
keeps the artifact's current value.

Title on create: a new artifact needs a title. When `title` is omitted, the
tool uses the text of the page's first `<title>` element: the tag name in any
case, attributes allowed; `&amp;`, `&lt;`, `&gt;`, `&quot;` and `&apos;`
decoded once (other entities kept as written); runs of whitespace collapsed to
one space; trimmed; cut to 200 characters. A page sent as base64 (a
`file_path` that is not UTF-8 or lacks a text extension), or one with no
closed `<title>` holding non-blank text, gives none, and the call fails with
`invalid_args`. An explicit `title` always wins; an explicit blank one on
create reaches the daemon, which refuses it (`POST /api/artifacts` without a
non-blank `title` is 400 `invalid_args` `title is required when creating an
artifact`).

A file entry is an object with exactly one of:

- `path` (string): a local file. Text extensions (`html`, `htm`, `css`, `js`,
  `mjs`, `json`, `svg`, `md`, `txt`, `csv`, `xml`, `map`) that decode as UTF-8
  are sent as text, everything else as base64.
- `content` (string), with `encoding` `"utf8"` (default) or `"base64"`.

plus optional `content_type` (string; otherwise inferred from the published
path's extension). `encoding` with `path` is an error.

Local paths may be absolute or relative. The shim resolves a relative path
against its session's working directory, registering the session first if it
has not yet; a session whose working directory is not known yet (under Codex,
when reading it failed and until the `SessionStart` hook fills it in and the
next heartbeat brings it to the shim) refuses it with `invalid_args` `file
paths must be absolute: session <ID> has no working directory yet to resolve
'<path>' against`. The daemon's `/mcp` has no session and refuses relative
paths with `invalid_args` `file paths must be absolute: there is no session
working directory to resolve '<path>' against`. Pi resolves against the Pi
session's working directory and drops a leading `@`; if Pi reports no working
directory it refuses relative paths with that second message.

Result:

```json
{
  "artifact_id": "7q3k9mzx2b4t",
  "url": "http://localhost:7480/a/7q3k9mzx2b4t",
  "version": 2,
  "title": "Sales Dashboard",
  "files": ["app.js", "index.html", "style.css"],
  "feedback": []
}
```

`url` is for the person. `files` lists every published path of the new
version, including files carried forward from the previous one.

Errors:

- `invalid_args`: neither or both of `html` and `file_path`; both `id` and
  `url`; `index.html` in `files`; a file entry with neither or both of `path`
  and `content`, or with `encoding` alongside `path`; a relative path with no
  session working directory; a new artifact with neither `title` nor a usable
  `<title>` (message ``a new artifact needs a title: pass `title`, or give the
  page a non-empty <title>``), or with a blank `title` (the daemon's
  `title is required when creating an artifact`).
- `invalid_id`: `id` or `url` names no artifact ID.
- `file_unreadable`: a local file could not be read. Extra field `path`.
- `conflict` (409): `if_version` is not the current version. Extra fields
  `current` (the current version number), and when the current version could
  be fetched, `current_version` (`n`, `label`, `created_at`, `files`, `url`)
  and `hint`. Read the current version, merge, and publish again with
  `if_version` set to `current`.
- `not_found`: the artifact to update does not exist.
- `invalid_path`: a published path is empty, absolute, ends in `/`, contains
  `\`, a `.` or `..` segment, or a control character; or two paths differ only
  by letter case; or one path is both a file and a directory.
- `invalid_encoding`: base64 `content` does not decode.
- `file_too_large`: one file exceeds 16 MiB.
- `body_too_large`: the files together exceed 64 MiB decoded, or the request
  body exceeds 96 MiB.
- `label_too_long`: `label` exceeds 60 characters.
- `unknown_session`: the session the tools registered has ended (for example
  through a `SessionEnd` hook) or no longer exists. The shim and the Pi
  extension register a new session with the same harness session ID and retry
  once, so this surfaces only when that retry fails the same way.

```json
{
  "error": {
    "code": "conflict",
    "message": "artifact is at version 3",
    "current": 3,
    "current_version": {
      "n": 3,
      "label": "tighter layout",
      "created_at": "2026-09-29T10:15:02.114Z",
      "files": ["index.html", "style.css"],
      "url": "http://localhost:7480/a/7q3k9mzx2b4t/v/3"
    },
    "hint": "read the current version, merge your change, and retry with if_version = 3"
  },
  "feedback": []
}
```

### read

Reads one published file of an artifact version as stored, before serve-time
wrapping.

| Name | Type | Required | Meaning |
|---|---|---|---|
| `url_or_id` | string | yes | Artifact ID or URL. A URL naming a version (`/a/<id>/v/<n>`, `/c/<id>/v/<n>/...`, `http://<id>.localhost:<port>/v/<n>/...`) selects it. |
| `path` | string | no | Published path; default `index.html`. |
| `version` | integer ≥ 0 | no | Version to read; default the URL's version, else the current version. |
| `max_bytes` | integer ≥ 0 | no | Cap on returned bytes; default 200000. |

Result:

```json
{
  "artifact_id": "7q3k9mzx2b4t",
  "version": 2,
  "path": "index.html",
  "content_type": "text/html",
  "truncated": false,
  "size": 1843,
  "content": "<!doctype html>...",
  "feedback": []
}
```

Text content types (`text/*`, `application/json`, `application/javascript`,
`image/svg+xml`) come back as `content`, cut at `max_bytes` without splitting
a character. Other types come back as `content_base64` when `size` is at most
`max_bytes`, and with neither field otherwise. `size` is the full file size;
`truncated` is `size > max_bytes`.

Errors: `invalid_id`; `not_found` (no such artifact, no such version, or no
such file in that version).

### list

| Name | Type | Required | Meaning |
|---|---|---|---|
| `limit` | integer ≥ 0 | no | Most artifacts to return. |
| `scope` | `"all"` or `"mine"` | no | `all` (default) or only artifacts this session created. Without a session (the daemon's `/mcp`), `mine` is empty. |

Result, pinned first, then most recently updated:

```json
{
  "artifacts": [
    {
      "id": "7q3k9mzx2b4t",
      "url": "http://localhost:7480/a/7q3k9mzx2b4t",
      "title": "Sales Dashboard",
      "version": 2,
      "pinned": false,
      "updated_at": "2026-09-29T10:15:02.114Z",
      "owner_session_id": "01K6AB3Q9X7N2M4P5R6S8T0V1W"
    }
  ],
  "feedback": []
}
```

`owner_session_id` is `null` for artifacts published without a session.

Errors: only those every tool can return.

### delete

| Name | Type | Required |
|---|---|---|
| `url_or_id` | string | yes |

Deletes the artifact and all its versions.

```json
{ "artifact_id": "7q3k9mzx2b4t", "deleted": true, "feedback": [] }
```

Errors: `invalid_id`, `not_found`.

### open

| Name | Type | Required |
|---|---|---|
| `url_or_id` | string | yes |

Checks that the artifact exists, then opens its URL in a browser on the
machine running the tool (`open` on macOS, `xdg-open` elsewhere). With
`ARTIFAX_NO_OPEN` set, no browser is started.

```json
{ "url": "http://localhost:7480/a/7q3k9mzx2b4t", "opened": true, "feedback": [] }
```

The tool waits up to 1.5 s for the opener. `opened` is `true` when the opener
exits successfully within that time, or is still running then (best effort:
some openers hand off and linger, so one that fails later is still reported as
opened). It is `false` when the opener cannot be started or exits
unsuccessfully (for example `xdg-open` with no display or handler), or when
`ARTIFAX_NO_OPEN` is set; then give the person `url`.

Errors: `invalid_id`, `not_found`.

### pin and unpin

| Name | Type | Required |
|---|---|---|
| `url_or_id` | string | yes |

Pins the artifact to the top of the gallery, or removes the pin.

```json
{ "artifact_id": "7q3k9mzx2b4t", "pinned": true, "feedback": [] }
```

Errors: `invalid_id`, `not_found`.

### asset_upload

Uploads local files as assets of an artifact. Assets are not part of any
version; a page references one by the returned `url`.

| Name | Type | Required | Meaning |
|---|---|---|---|
| `url_or_id` | string | yes | The artifact the assets belong to. |
| `file_path` | string | at least one of `file_path`, `file_paths` | One local file. |
| `file_paths` | array of string | at least one of `file_path`, `file_paths` | Several local files. |

Paths resolve as for `publish`. Every file is read before any is uploaded;
uploads then happen one at a time, so a failure part way leaves the earlier
files uploaded. The content type is inferred from the file name.

```json
{
  "assets": [
    {
      "id": "01K6AB7D2E3F4G5H6J7K8M9N0P",
      "url": "http://localhost:7480/_blob/01K6AB7D2E3F4G5H6J7K8M9N0P",
      "content_type": "image/png",
      "size": 48213
    }
  ],
  "feedback": []
}
```

Errors:

- `invalid_args`: neither `file_path` nor `file_paths`; a relative path with
  no session working directory.
- `invalid_id`, `file_unreadable` (extra field `path`), `not_found`.
- `unsupported_type`: accepted types are `image/*`, `video/*`, `font/*`,
  `application/pdf`, `text/css`, `text/javascript`, `text/csv`,
  `text/markdown`, `application/json`, `text/plain`.
- `asset_too_large`: a file exceeds 20 MiB.
- `body_too_large`: the upload request exceeds 21 MiB.

### status

No arguments.

```json
{
  "daemon_url": "http://localhost:7480",
  "version": "0.2.0",
  "harness": "claude",
  "session": {
    "id": "01K6AB3Q9X7N2M4P5R6S8T0V1W",
    "harness": "claude",
    "harness_session_id": "3f0c9f7e-5a44-4b8e-9d0e-2a6f1c7b9e21",
    "cwd": "/Users/alex/proj",
    "pid": 48213,
    "parent_pid": 48200,
    "started_at": "2026-09-29T10:02:40.551Z",
    "last_seen_at": "2026-09-29T10:14:40.560Z",
    "ended_at": null
  },
  "watches": [
    {
      "session_id": "01K6AB3Q9X7N2M4P5R6S8T0V1W",
      "artifact_id": "7q3k9mzx2b4t",
      "replies_armed": true,
      "created_at": "2026-09-29T10:05:12.304Z"
    }
  ],
  "push": {
    "tier": null,
    "available": false,
    "reason": "Claude Code has no native push; comments arrive at the end of a turn (Stop hook), with the next prompt, on the next artifax tool call, or during wait_for_feedback"
  },
  "feedback": []
}
```

`version` is the daemon's version. `harness` and `session` are `null` when the
tools have no registered session (the daemon's `/mcp`, or a shim or Pi
extension that has not yet reached a daemon). `session` is the row as of the
last registration or heartbeat: the shim heartbeats every 60 s, so its
`last_seen_at` lags by up to a minute and a `cwd` the `SessionStart` hook
filled in appears after the next heartbeat; Pi sends no heartbeat, so under Pi
it is the row as registered. `daemon_version` (the daemon's version again) is
present only when it differs from the Artifax version of the tools answering
(the shim's binary, or the Pi package, which carries the same version), which
signals version skew.

`plugin_version` is the version in the manifest of the plugin that started the
shim, and `skew` is `true` when it differs from the shim's binary version
(`false` when they match). Both are absent when the shim does not know its
plugin root: it reads `CLAUDE_PLUGIN_ROOT`, then `PLUGIN_ROOT`, then, under
Codex, its working directory when that holds `.codex-plugin/plugin.json`. The
daemon's `/mcp` and the Pi extension never report them. The shim also logs the
comparison to stderr when it starts (a warning on skew).

`watches` lists this session's watches (`[{session_id, artifact_id,
replies_armed, created_at}]`; `[]` without a session). `push` says whether
comments can be pushed into this session (tier 5) and why not:
`{tier, available, reason}`, plus, for Codex, `codex_home` (the recorded
`CODEX_HOME`, or `null`), `last_error` (why the last `codex queue` run failed,
or `null`) and `last_error_at` (when, as an RFC 3339 timestamp, or `null`); a
successful run clears both, and a failure leaves `available` true. Under Codex `tier` is `"queue"` when available, else
`null` with `reason` "Codex session ID unknown, native push disabled" or the
daemon's reason for having no `codex` (see "Delivery tiers per harness");
under Pi it is `{"tier": "inject", "available": true, "reason": null}`; under
Claude Code it is as shown above. `push` is `null` without a session, or when
the daemon could not be asked.

Version skew: a shim that finds a daemon older than itself stops it and starts
its own on the old daemon's bind address (the port is the shim's `--port`,
7480 by default), logging the replacement. A newer daemon, or one whose
version does not parse, is kept.

Errors: only those every tool can return.

## Sessions

A session is one harness conversation. Publishes made through a session's
tools carry it (the `X-Artifax-Session` header on the daemon's publish routes)
and the artifact records it as `owner_session_id`; the gallery shows which
session published each artifact and whether that session is live.

The shim takes `harness_session_id` from `CLAUDE_CODE_SESSION_ID` under Claude
Code, else from `ARTIFAX_SESSION_ID` for any harness, else sends none.

A session row has `harness` (`claude`, `codex`, `pi`), `harness_session_id`
(the harness's own ID, when known), `cwd`, `pid` (the shim or Pi process),
`parent_pid`, and timestamps. Registration and join accept only those three
harness names (anything else is 400 `invalid_args` `harness must be one of
claude, codex, pi`); an empty `harness_session_id` on registration counts as
none. The daemon matches registrations to existing
live rows so that a shim and a hook for the same conversation share one row:

- A registration with a `harness_session_id` reuses the live row with the same
  `(harness, harness_session_id)`; failing that, it adopts the live row with
  the same `(harness, parent_pid)` that has no harness session ID yet.
- A registration without one reuses the live row with the same
  `(harness, parent_pid)` seen within the last 300 seconds (a restarted shim).
- A hook's join (`POST /api/sessions/join`) reuses the live row with the same
  `(harness, harness_session_id)`; failing that, gives the ID to the live row
  without one whose `parent_pid` is the hook's parent process, then each of the
  hook's ancestors, nearest first (up to six); failing that, it inserts a
  hook-only row for a later shim registration to adopt.
- A given `cwd` fills an empty one on the matched row.

Rows end when the shim's stdin closes or it receives SIGTERM (3 s deadline),
when a `SessionEnd` hook or Pi's `session_shutdown` ends them, or when the
daemon's reaper finds a row unseen for 300 seconds whose `pid` is unknown or no
longer alive. The shim marks its row seen every 60 seconds; hook-only rows and
Pi rows get no heartbeat. An ended row is never revived. A shim or Pi
extension whose row was ended under it registers a new row with the same
harness session ID on its next tool call (see `unknown_session`), and a shim
heartbeat that finds its row ended or gone registers again; a shim that
reaches a restarted daemon registers again.

When a row ends, its watches are dropped, and each of its comments not yet
handed over is deleted when another live session is already a target of the
same comment, and otherwise waits untargeted for the next session that
publishes or watches the artifact (see "Comments and feedback").

The hooks give up rather than hold up the harness: `session-start` after 4 s
(3 s per daemon request), `session-end` after 2.5 s (2 s per request, inside
Codex's 3 s `SessionEnd` cap), `stop` after 8 s and `prompt` after 4 s (3 s
per request; the plugins give the Stop hook 10 s and the prompt hook 5 s). A
hook that gives up, or finds no daemon, exits 0 with no output. The `stop` and
`prompt` hooks find the live row by the harness session ID in their input and
do nothing when there is none.

### Claude Code

Claude Code passes `CLAUDE_CODE_SESSION_ID`, `CLAUDE_PID` and
`CLAUDE_PROJECT_DIR` to MCP servers. The shim registers with
`harness_session_id` set to `CLAUDE_CODE_SESSION_ID` and `cwd` set to
`CLAUDE_PROJECT_DIR` (else its own working directory), so the row is keyed by
the Claude session ID from the start. Hooks never start a daemon: when one is
running, the `SessionStart` hook (`artifax hook --agent claude session-start`)
joins by that same ID and adds the daemon URL to the session context, and the
`SessionEnd` hook ends every live row with that ID; when none is running, they
do nothing. If `CLAUDE_CODE_SESSION_ID` is absent, the shim registers as under
Codex and the hook's parent-PID join applies.

The `UserPromptSubmit` hook (`artifax hook --agent claude prompt`) and the
`Stop` hook (`artifax hook --agent claude stop`) hand comments over (tiers 3
and 2 under "Comments and feedback"); the `SessionStart` hook also appends
comments already waiting for the session to its context.

Without hooks the row is still keyed by the session ID; it ends when the shim
exits or through the reaper.

### Codex

Codex passes only `PATH`, `PWD` and the variables the plugin's `env_vars`
lists to MCP servers, and starts the shim in the plugin's own directory. The
shim therefore registers with no `harness_session_id` (unless
`ARTIFAX_SESSION_ID` is set in its environment, which the plugin does not
forward), its parent process
(Codex) as `parent_pid`, and Codex's working directory as `cwd`, read with
`lsof` on macOS or from `/proc/<pid>/cwd` on Linux (empty when that fails). The
`SessionStart` hook runs under a shell, so its parent is that shell; it sends
the Codex session ID from its input, its parent PID, and its ancestors, and
the daemon gives the ID to the shim's row by matching Codex's PID among them.
The hook also fills an empty `cwd`, records its `CODEX_HOME` (when set) for
`codex queue`, and adds the daemon URL and any comments already waiting for
the session (those tier 3 would carry) to the session context. The `Stop`
hook (`artifax hook --agent codex stop`) hands comments over at the end of a
turn. The `SessionEnd` hook ends the row by the
Codex session ID.

Hooks run only when the person enables and trusts them. Without them the row
has no harness session ID; the tools work and publishes are attributed to it,
it ends when the shim exits or through the reaper, and comments reach it only
through tiers 1 and 4.

Because a registration without a harness session ID adopts the live row with
the same `(harness, parent_pid)`, several conversations hosted by one Codex
process (one shim each) share the first conversation's session: all of them
publish as that session, and a later conversation's `SessionStart` hook joins
a hook-only row that no shim adopts.

### Pi

Pi has no hooks and no MCP. The extension registers the session itself on
Pi's `session_start` event: `POST /api/sessions` with
`{"harness": "pi", "harness_session_id": <Pi session ID>, "cwd": <Pi's cwd>,
"pid": <Pi's PID>, "parent_pid": <its parent>}`, starting a daemon first when
none is running. Pi waits for this handler, so it is cut off after 3 seconds;
when it does not finish, the first tool call registers instead. On
`session_shutdown` the extension ends the row (3 s deadline). It sends no
heartbeat: a row whose Pi process has exited without a clean shutdown is ended
by the reaper.

### No session

The daemon's `/mcp` endpoint has no session: publishes have no owner,
`list` with `scope: "mine"` is empty, `status` reports `null` for `harness`
and `session`, and relative file paths are rejected.

The `artifax` CLI has no session either: `artifax publish` never attributes a
publish to one. A new artifact it publishes has no owner, so comments sent to
the agent on it wait, undelivered, until an agent session watches it (the
`watch` tool) or publishes a version of it. After the URL the command prints
`note: published without an agent session; comments on this page will wait
until an agent session watches it` when neither the new version nor the
artifact has a session; `--json` reports it as `"session": null`, or the
session's ID (the version's, else the artifact owner's) when there is one.

## Comments and feedback

People comment on a page in the browser: comment mode outlines the element
under the pointer; a click anchors a thread to that element, a text selection
to that range. Over an element taller or wider than the viewport, or covering
more than 60% of it (a whole file in one `<pre>`), comment mode targets the
text under the pointer instead: its line in preformatted text, else the block
or sentence around it, anchored as a range; the outline always stays inside
the viewport. The bridge records the anchor (spec §9) and a PNG clip
whenever the region it renders fits 1600 × 2400 CSS px: the element, or a
range's nearest block, when that fits; for a range inside a larger block, the
lines from 120 px above to 120 px below it at the block's width, with the
picked text marked in the outline's tint (from 120 px above the start of a
range taller than that); for an element larger than that, the whole element
scaled down when it is wholly in view, else its part in the viewport, grown
within it to that size. Clips are stored at `<ARTIFAX_HOME>/artifacts/<aid>/clips/<thread ID>.png`. A
thread is plain until the person presses **Send to agent** or writes `@agent`
(as a word, not inside an address) in a comment; from then on, every later
viewer comment on it is sent too. A viewer comment on a resolved thread
reopens it.

Every HTML page of a version is commentable: `index.html` and every supporting
file stored as `text/html` are served with the bridge (a fragment inside the
document skeleton, a full document as written plus the bridge tag; a page that
already carries a bridge tag keeps exactly one). The raw bytes stay available
at `/api/artifacts/<id>/versions/<n>/files/<path>`. An anchor names its page
in `file`: the published path (`index.html` for the index; an anchor without
`file` is on the index). A thread's `file` must be a published path of its
version, or the thread is refused with `invalid_anchor`. Following a link
inside the page to another HTML page of the artifact loads that page with the
bridge, so comment mode works there the same way. The shell shows the pins of
the page in the frame only, lists threads on other pages in the sidebar
labelled "on <file>", and opening one takes the frame to that page and scrolls
to the thread. A thread detaches only when its anchor is not found on its own
page.

A sent comment goes to every live target session: the session that created
the artifact, and every session that watches it. Publishing (a new artifact or
a new version) makes the publishing session watch the artifact with replies
on (an existing watch keeps its setting); the `watch` tool adds or removes a watch. Agent replies and resolves need
a live session and work only on sent threads.

### Shell URLs

`/a/<id>` shows the latest version and `/a/<id>/v/<n>` version `n`; either may
be followed by `/<file>`, a published path whose segments are percent-encoded,
to open the frame on that page (`/a/<id>/about.html`,
`/a/<id>/v/2/docs/source.html`; `index.html` is never written). After
`/a/<id>`, a `v` segment followed by an all-digit segment is a version and
anything else starts the file path, so a file published under `v/<digits>/` is
reachable only through the versioned form. The address bar follows the frame,
and each move to another page is one history entry. A plain click on a link
to another page of the version (no query, no modifier key, not cancelled by
the page, not in comment mode) is handed to the shell, which pushes that
page's URL and moves the frame without an entry of its own, so back and
forward move between pages, also after the shell reloaded. The link's
fragment is kept, in the frame and in the address bar. A link to the page
already shown stays with the browser; under another spelling of its path
(`index.html` for the version's root) it is followed in place, without a
history entry. Opening a thread
on another page from the sidebar works the same way. Any other navigation
inside the page (a script, a form) keeps the frame's own history entry, and
the shell replaces its URL when the new page greets. A page the version does
not hold shows a message instead of the frame; a thread anchored on one is
listed under Detached. The `url` in tool results and payloads stays
the artifact URL (`/a/<id>`); every `url_or_id` argument accepts the page form
too (the page is ignored; a version in it is used where the tool takes one).

The address bar's fragment and the frame's stay in step: a shell URL with a
fragment (`/a/<id>/source.html#docs%2Fcontract.md`) opens the frame at it,
back and forward carry it to the frame, and a fragment change inside the page
(a link or a script) replaces the address bar's fragment without a history
entry of the shell's own; copy link includes it.

### Tools

| Tool | Arguments | Result |
|---|---|---|
| `comments_read` | `url_or_id`; optional `thread_id`, `cursor`, `include_resolved` | `{artifact_id, url, threads: [{thread_id, status, sent_to_agent, version, anchor: {kind, selector, quote, custom_name, file}, clip_path, comments: [{id, author_kind, author_name, body, created_at}], feedback_state}], next_cursor, note}` |
| `comments_reply` | `url_or_id`, `thread_id`, `text` | `{thread_id, replied: true, comment_id}` or `{thread_id, replied: false, guidance}` |
| `comments_resolve` | `url_or_id`, `thread_id` | `{thread_id, resolved: true, status}` or `{thread_id, resolved: false, guidance}` |
| `watch` | `url_or_id`; optional `on` (default true), `replies` (default true) | `{artifact_id, url, watching, replies_armed}` |
| `wait_for_feedback` | optional `url_or_id`; optional `timeout_s` (default 50) | `{feedback: [...], waited_s, call_again}` |

`comments_read` returns the open threads (and resolved ones with
`include_resolved`) oldest first, 50 per page, with `next_cursor` naming the
next page (`null` on the last); `thread_id` returns that one thread whatever
its status. `quote` has its whitespace collapsed and is cut to 200
characters followed by `…`. `clip_path` is the absolute path of the clip, or
`null` when none was captured. `feedback_state` is the
thread's delivery state (`{thread_id, state, tier, since, resends,
exhausted}`, see "What the person sees"), or `null` when nothing on it was
sent. `note` says that comment text comes from people viewing the page.
Through a session, reading acknowledges the returned comments of sent threads
for that session (a comment added after the read stays pending).

`comments_reply` posts the reply as the agent; the person sees
`Agent · via <harness>`. Replying to or resolving a thread acknowledges its
comments for the session. Resolving a thread withdraws its comments that no
session has been handed yet. When a later viewer comment reopens a sent
thread, the comments a viewer resolve withdrew are sent again with it (no
session saw them); comments already handed over are not resent.

`watch` with `on: false` removes the session's watch (the result has
`watching: false, replies_armed: false`); the session that created the
artifact still receives its comments through tiers 1, 3 and 4. Watching an
artifact hands the session any of its comments that were waiting untargeted.

`wait_for_feedback` returns as soon as comments sent to this session arrive
(only those on `url_or_id` when given), or after `timeout_s` seconds with
`feedback: []` and `call_again: true`. `timeout_s` is raised to 1 and capped
at 600. `waited_s` is whole seconds.

Error codes besides the common ones: `invalid_id` (`url_or_id` names no
artifact), `invalid_args` (a `thread_id` that is not a thread ID, empty
`text`), `invalid_comment` (`text` over 10,000 characters), `invalid_cursor`
(a `cursor` that is not a thread of the artifact), `not_found` (the artifact
or thread is gone), `no_session` (`watch` and `wait_for_feedback` through
`/mcp`), `unknown_session` (`comments_reply` and `comments_resolve` through
`/mcp`, which sends no session; this is checked before whether the thread was
sent). Through a session, replies and resolves on a thread that was not sent
to the agent are not errors: the result carries `guidance` and nothing
changes.

### Payload

Each forwarded comment is rendered as:

```
[artifax] Comment sent to you on "Quarterly Review" (http://localhost:7480/a/7q3k9mzx2b4t), thread 01J9...
Anchored on: main > section:nth-of-type(2) > h2  «Quarterly goals»  (v3)
Clip: /Users/alex/.artifax/artifacts/7q3k9mzx2b4t/clips/01J9....png
Alex: "Make this a two-column layout and drop the third bullet."
Reply with comments_reply, then comments_resolve when done.
```

The title and the comment text are JSON strings (U+0085, U+2028 and U+2029
escaped too), so a comment is always one line. The anchor line holds the
page's file followed by ` › ` when it is not `index.html`
(`Anchored on: source.html › main > h2  «Sources»  (v3)`), the
selector (`custom:<name>` for a custom anchor) and, when there is one, the
quote with whitespace collapsed, `«` and `»` replaced by `"`, and cut to 120
characters followed by `…`. Author names lose control characters, `"` and `:`, and are cut to
40 characters (`Viewer` when empty). A resend says `Comment sent to you
(resent)`. A thread without a clip says `Clip: none (no screenshot was captured
for this comment)`. The payload starts with `[artifax] N comments sent to
you:` (`1 comment` for one) on its own line, followed by the comments
separated by blank lines. Tool results carry this text after `---` in a
second text block; the Stop hook's `reason`, the prompt hook's
`additionalContext`, `codex queue --message`, and Pi's follow-up message carry
it without `---`. The structured form is each result's `feedback` array:
`{feedback_id, thread_id, comment_id, artifact_id, artifact_title, url,
version, anchor, clip_path, author, body, resent, created_at}`.

### Delivery tiers per harness

Measured on 2026-09-29 with Codex CLI 0.158.0 and Claude Code 2.1.284; Pi
0.73.1 from its source.

| Tier | Claude Code | Codex | Pi |
|---|---|---|---|
| 1, tool result | next artifax tool call (shim) | next artifax tool call (shim) | next `artifax_*` tool call (`tool_result` handler) |
| 2, Stop hook | end of the turn: `{"decision":"block","reason":...}` continues the turn with the payload | same shape and behaviour, measured with `codex exec` | none |
| 3, prompt hook | the person's next message (`UserPromptSubmit` `additionalContext`); also at session start (`SessionStart` `additionalContext`) | only at session start: the `SessionStart` hook adds waiting comments to its `additionalContext`; no `UserPromptSubmit` hook is wired | none |
| 4, `wait_for_feedback` | immediate while waiting | immediate while waiting; one call stays under Codex's 60 s tool limit | immediate while waiting |
| 5, native push | none: an idle Claude Code session is not woken | `codex queue`: an idle attached TUI starts a turn in about 0.2 s; a busy one runs it as its next turn; with no client attached (an exited TUI, a `codex exec` thread) it is held until `codex resume`, and `codex queue` still exits 0 | the extension long-polls and calls `sendUserMessage(..., {deliverAs: "followUp"})`: a turn starts at once when idle, after the current work when busy (from source; not run live) |

Tier 1 applies to every successful tool result of a session-bound shim or Pi
extension, except `wait_for_feedback`, whose result is tier 4. Tiers 2 and 5
apply only to watches with `replies_armed`. While `stop_hook_active` is set,
the Stop hook blocks only for comments never handed over before, so each
comment blocks a stop at most once.

Tier 5 for Codex needs the Codex session ID (from the `SessionStart` hook, so
hooks must be enabled and trusted), `codex` from `ARTIFAX_CODEX_BIN` when it
is set, else from the daemon's `PATH`, and the session's `CODEX_HOME` (passed
by the hook when set; otherwise `codex` uses its default). A set
`ARTIFAX_CODEX_BIN` is never followed by a `PATH` search: the empty string
turns Codex push off on purpose, and a value that is not an executable file
turns it off with a reason naming that value. `artifax doctor --agent codex`
checks the first two, naming where `codex` came from. `GET /api/push` reports
only the daemon's `codex`: without the token `{"codex": {available, source,
reason}}` (where `codex` came from, and why push is off when it is); with the
token it adds `bin`, the path (or `null`). `status` reports `push` for the
session with the reason when it is off. Tier 5 (Codex and Pi) is skipped while
the session is inside `wait_for_feedback`, which delivers instead. The
comments read "delivered via codex queue" from the moment they are claimed for
`codex queue` (the state is published then, before it runs) until it finishes
(at most 10 s); a run that exits 0 changes nothing further. Any failure (a
non-zero exit, a kill by a signal, a timeout of 10 s, or a failure to start
it) releases the comments to tiers 1 to 4 (they read "sent" again), and
`codex queue` is not tried again for them. A failure never ends the session:
it stays live with its watches, and `status` shows the reason in
`push.last_error` (`codex queue exited with code <n>`, `codex queue was
killed by a signal`, `codex queue timed out`, or `codex queue could not run:
<error>`).

Pi's tier 5 long-poll runs from the session's registration to
`session_shutdown`, 50 s per poll, pausing 5 s after a failed poll or one that
came back empty in under a second. While the session is inside
`wait_for_feedback`, the daemon answers each of these polls at once with
`{"feedback": [], "text": null, "waited_s": 0}` and hands the comments to the
wait, so the loop pauses. It finds a running daemon but never starts one.

### Acknowledgement and resends

A feedback row (one per forwarded comment and target session) is delivered
once, by the first tier that hands it over. Tiers 1 and 4 count as seen and
acknowledge at once; so does the agent calling `comments_read`,
`comments_reply`, or `comments_resolve` on the thread. A row delivered by
tiers 2, 3, or 5 and not acknowledged within 2 minutes is resent, marked
`(resent)`, by the next tool result or the next Stop hook (not while
`stop_hook_active` is set), at most three times. Then the thread shows
"delivered, not acknowledged".

When a session ends, each of its rows not yet handed over is deleted when
another live session is a target of the same comment, and otherwise waits
untargeted. Untargeted rows (also those of a comment sent while no target
session was live) go to the next session that publishes a version of the
artifact or watches it.

### What the person sees

The thread's waiting indicator follows the `feedback_state` event:

| State | Indicator |
|---|---|
| `sent` | "sent, waiting for the agent · <elapsed> · waiting on <the tier: its next artifax tool call, the end of its turn, Codex to pick up the queued message, Pi to take the message>" |
| `delivered` | "delivered via <tier> · <elapsed> ago · not yet acknowledged" (then "· resent once" or "· resent N times"); "delivered, not acknowledged" after three resends |
| `acknowledged` | "seen by the agent" |
| `agent_ended` | "agent session ended; waiting for a new one" |

A resolved thread's `resolved_by` is `viewer:<public ID>`, `viewer:anonymous`
(a resolve without a viewer cookie), or `agent:<harness>`; it never carries a
viewer cookie or a session ID. The card reads "Resolved by" and the viewer's
own name when it resolved the thread and has one, "Viewer" for any other
viewer, or "Agent · via <harness>". An agent comment carries `via_harness`
(the replying session's harness, e.g. `claude`; `null` on viewer comments).

## Page contract

Every page follows this contract so it renders well in the gallery, in light and
dark mode, and on a phone:

- A `<title>` element with a short name (two to four words). A new artifact
  needs a title: pass `title` on the first publish or give the page a
  non-empty `<title>`, which the tools then use.
- Colors and other design values are CSS custom properties (tokens) on `:root`.
- Dark mode is provided twice, so both the system setting and the person's
  explicit choice work:
  - under `@media (prefers-color-scheme: dark)`, guarded by
    `:root:not([data-theme="light"])`;
  - again under `:root[data-theme="dark"]`.
- `body` has an explicit background (and text color) taken from the tokens.
- The layout works at phone width: a 16 px side gutter, no horizontal page
  scroll.
- Browser storage (`localStorage`, `sessionStorage`, IndexedDB) is optional
  convenience only. Wrap every read and write in `try/catch` and make the page
  render correctly without it.
- `window.claude.use(name)` is the entry point for runtime capabilities. Until
  capabilities ship (phase 4) it resolves `null` for every name, so pages must
  handle `null` and work without it.
- Supporting files (stylesheets, scripts, images, data) are referenced with
  relative paths, for example `<link rel="stylesheet" href="style.css">`, and
  published under the same relative path in `files`. Do not use absolute
  `/...` paths or hard-coded hosts.

Minimal skeleton:

```html
<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Example Page</title>
<style>
  :root { --bg: #ffffff; --fg: #1a1a1a; --accent: #2b5fd9; }
  @media (prefers-color-scheme: dark) {
    :root:not([data-theme="light"]) { --bg: #14161a; --fg: #ececec; --accent: #7aa2ff; }
  }
  :root[data-theme="dark"] { --bg: #14161a; --fg: #ececec; --accent: #7aa2ff; }
  body { margin: 0; padding: 16px; background: var(--bg); color: var(--fg); font: 16px/1.5 system-ui, sans-serif; }
  a { color: var(--accent); }
</style>
</head>
<body>
<h1>Example Page</h1>
</body>
</html>
```

## Security model

- The daemon binds `127.0.0.1` by default. `artifax serve --bind 0.0.0.0` (or
  another address) serves on the LAN.
- Every `/api/**` route answers only when the `Host` header is literally
  `localhost`, `127.0.0.1` or `[::1]` (port optional), or exactly the IP and
  port the connection arrived on (the port may be left out only when it is
  80); anything else, including every other DNS name, gets 403
  `forbidden_host`, which defeats DNS rebinding. Under a LAN bind (including
  `0.0.0.0` or `::`) a viewer reaches the API on `http://<the interface's
  IP>:<port>`; another IP or port, or a name for it, is refused. The shell,
  content, blob, and `/healthz` paths are not subject to this check.
- Every route that changes state, except the viewer routes below, requires
  `Authorization: Bearer <token>` with the token from
  `<ARTIFAX_HOME>/daemon.json` (mode 0600): creating and publishing artifacts,
  changing and deleting them, uploading and deleting assets, registering,
  joining and ending sessions, watches, taking and acknowledging a session's
  feedback, and shutting the daemon down. Reading sessions and their watches
  (`GET /api/sessions...`) needs it too, since a session row carries a working
  directory, process IDs and the harness's session ID; without it they are 401
  `unauthorized`. The comparison is constant time. The other read routes
  (`GET /api/artifacts...` including threads and clips, `GET /api/push`, the
  gallery, content, blobs) need no token, so LAN viewers can read artifacts
  and comment on them but not publish or change them. `GET /api/push` names
  the daemon's `codex` path (`bin`) only to a request with the token. `GET /api/artifacts/<id>` returns
  `{artifact, versions}`; like each entry of the artifact list, `artifact`
  carries `owner_session_id`, `owner_live` and `owner_harness`, never the
  owner's session row. Content and asset URLs are readable by anyone who can
  reach the daemon and knows the unguessable artifact or asset ID.
- `GET /api/token` hands the token to the gallery in a local browser. On top
  of the `Host` rule above, it answers only when the connection comes from a
  loopback address and the `Host` header is literally `localhost`,
  `127.0.0.1` or `[::1]` (with an optional port); otherwise (for example a LAN
  peer, or the LAN IP as `Host`) it returns 403 `not_loopback`.
- `/mcp` requires the bearer token on every request and accepts only a `Host`
  of `localhost`, `127.0.0.1`, `::1`, or the daemon's own address and port.
- Each artifact has its own origin, `http://<id>.localhost:<port>`. On that
  host the daemon serves only `/v/<n>/...` (that artifact's content),
  `/healthz`, `/_artifax/...` and `/_blob/...`; every other path, the API
  included, is a 404. The host must be exactly a valid 12-character lowercase
  artifact ID followed by `.localhost` and an optional numeric port.
- The viewer probes `http://<id>.localhost:<port>/healthz` (1 s timeout,
  cached per browser session) when the gallery is opened on `localhost` or
  `127.0.0.1`. When it answers, the content iframe loads from the artifact's
  origin with no `sandbox` attribute; the distinct origin is the isolation.
  Otherwise, and always when the gallery is opened on any other host (the
  LAN), the iframe loads `/c/<id>/v/<n>/` from the main origin with
  `sandbox="allow-scripts allow-forms allow-modals allow-popups
  allow-downloads"` and no `allow-same-origin`, so the content runs in an
  opaque origin.
- Content served on the main origin (`/c/<id>/v/<n>/...`) carries
  `Content-Security-Policy: sandbox allow-scripts allow-forms allow-modals
  allow-popups allow-downloads`, `/_blob/...` responses and the raw file route
  (`/api/artifacts/<id>/versions/<n>/files/...`) carry
  `Content-Security-Policy: sandbox`, so opening any of them as a top-level
  page cannot reach the API same-origin. Content on an artifact's own origin
  carries no CSP; the origin is the boundary. Supporting files and blobs are
  sent with `X-Content-Type-Options: nosniff`.
- The viewer routes (creating a thread, commenting, sending to the agent,
  resolving, and `GET`/`PUT /api/viewers/me`) need no token, so LAN viewers
  can comment. They refuse a request whose `Origin` is not the daemon's own
  (`http://` plus the request's `Host`, never an artifact origin) with 403
  `forbidden_origin`, so a published page cannot comment, send, or resolve on
  the person's behalf; requests without an `Origin` header (scripts) are
  allowed. The daemon serves plain HTTP only. A viewer is identified by the
  `artifax_viewer` cookie (`HttpOnly`, host-only, `SameSite=Lax`), whose value
  the daemon accepts only when it is a ULID. The cookie never leaves the
  daemon: no response body, event, thread view, comment, or log carries it.
  Outside the cookie a viewer is named by its public ID (`u_` and 22
  lowercase hex digits, assigned once and never changed): `GET`/`PUT
  /api/viewers/me` answer `{"viewer": {"public_id", "display_name",
  "created_at"}}`, and a viewer's resolve records `resolved_by`
  `viewer:<public ID>`. Comment threads never carry a session ID: an
  agent's resolve records `agent:<harness>` and its comments `via_harness`
  (the artifact list and artifact view still name the owning session's ID
  in `owner_session_id` and each version's `session_id`, which are not
  credentials).
  Agent replies and resolves need
  the token and `X-Artifax-Session` naming a live session (400
  `unknown_session` otherwise). Thread views carry `clip_path` only for
  requests with the token and never in `/api/events`; the clip itself
  (`GET /api/artifacts/<aid>/threads/<tid>/clip`) is served with
  `Content-Security-Policy: sandbox` and `X-Content-Type-Options: nosniff`.
  Comment text is untrusted input: tool results and the skill say so, and the
  payload quotes it as a JSON string.
- Published pages and uploaded files are untrusted content: Artifax never
  executes them outside the browser.
- No telemetry. The daemon makes no calls off the machine; the Claude Code
  and Codex plugins' launcher script tries to download a release only when
  no `artifax` binary is found (installed, or built in a source checkout),
  only for the MCP server (never for a hook), and no release has been
  published yet.

## Known limitations

- Content inside a nested `<iframe>` within a page is a dead zone in comment
  mode (pointer events never reach the page's own document, so it cannot be
  picked), and its area renders blank in comment clips.
- CSS counters and list numbering inside a region clip restart, because the
  clip renders a copy of the region.

## What is not yet available

- Runtime capabilities (phase 4): `window.claude.use(name)` resolves `null`
  for every name, and `capabilities` on `publish` is stored but has no effect.
- Rooms and `sample()` (phase 5): not available.
- Pi: the extension, its session handling and its tools are tested against a
  real daemon, but no model-driven Pi session has been run end to end, because
  no model provider key exists on the build machine.
