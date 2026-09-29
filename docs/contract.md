# Artifax contract

This document is the contract between Artifax and the agents that publish to
it: the tools, how a harness session is identified, what a published page must
do, what the daemon guarantees about isolation, and what does not exist yet.
The implementation is the authority where the two disagree:
`crates/artifax-mcp/src/tools.rs` (arguments), `crates/artifax-mcp/src/render.rs`
(result shape), `crates/artifax-server/src/routes/` (daemon errors),
`crates/artifax-core/src/store/sessions.rs` (sessions), and
`plugins/pi/src/artifax.ts` (the Pi tools).

## Tools

Nine tools: `publish`, `read`, `list`, `delete`, `open`, `pin`, `unpin`,
`asset_upload`, `status`. The MCP implementation lives in
`crates/artifax-mcp` and is served two ways:

- the stdio shim `artifax mcp --agent <claude|codex|pi>`, which a harness
  starts once per session and which attributes publishes to that session;
- the daemon's `/mcp` endpoint (MCP streamable HTTP, bearer token required),
  which attributes publishes to no session.

Pi's extension API cannot register an MCP server, so `plugins/pi` implements
the same nine tools in TypeScript against the daemon's REST API, with the same
arguments and the same result and error JSON.

Names as the model sees them:

| Harness | Tool name |
|---|---|
| Claude Code, plugin install | `mcp__plugin_artifax_artifax__<tool>` |
| Claude Code, plain `.mcp.json` entry named `artifax` | `mcp__artifax__<tool>` |
| Codex | `mcp__artifax__<tool>` |
| Pi | `artifax_<tool>` |

The command line covers most of the same operations for scripts and harnesses
without MCP: `artifax publish`, `list`, `open`, `delete`, `pin`, `unpin` and
`status`, each with `--json` for one JSON object on stdout. The CLI's JSON is
its own shape, not the tool result shape below, and there is no CLI
counterpart of `read` or `asset_upload`.

### Results

Every result is one text content block holding a pretty-printed JSON object.
The object always carries `"feedback": []` (comments arrive in phase 3). A
success is the tool's fields plus `feedback`. A failure is marked as an error
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
- `internal`, `corrupt` (500): a storage or database failure; see the daemon log.

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
| `title` | string | no | Artifact title. A new artifact without one is titled `Untitled`; the page's `<title>` is not read. |
| `description` | string | no | One-line description. |
| `icon` | string | no | One generic word, such as `chart` or `map`. |
| `label` | string | no | Short name for this version, at most 60 characters. |
| `capabilities` | object | no | Stored with the artifact; no effect until phase 4. |

On an update, an omitted `title`, `description`, `icon` or `capabilities`
keeps the artifact's current value.

A file entry is an object with exactly one of:

- `path` (string): a local file. Text extensions (`html`, `htm`, `css`, `js`,
  `mjs`, `json`, `svg`, `md`, `txt`, `csv`, `xml`, `map`) that decode as UTF-8
  are sent as text, everything else as base64.
- `content` (string), with `encoding` `"utf8"` (default) or `"base64"`.

plus optional `content_type` (string; otherwise inferred from the published
path's extension). `encoding` with `path` is an error.

Local paths may be absolute or relative. The shim resolves a relative path
against its session's working directory; the daemon's `/mcp` has no session
and rejects relative paths with `invalid_args`; Pi resolves against the Pi
session's working directory and drops a leading `@`.

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
  session working directory.
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
- `unknown_session`: the session the shim registered has ended or no longer
  exists.

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

`opened` is `false` when no browser was started; give the person `url`.

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
  "watches": [],
  "feedback": []
}
```

`version` is the daemon's version. `harness` and `session` are `null` when the
tools have no registered session (the daemon's `/mcp`, or a shim or Pi
extension that has not yet reached a daemon). `session` is the session as it
was when registered, so `last_seen_at` is not live. `daemon_version` (the
daemon's version again) is present only when it differs from the version of
the tools answering, which signals version skew. The Pi extension currently
compares against its own package version, not the Artifax version, so under
Pi it is always present; a fix is scheduled. `watches` is always empty
until phase 3.

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
`parent_pid`, and timestamps. The daemon matches registrations to existing
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
Pi rows get no heartbeat. An ended row is never revived; a shim that reaches a
restarted daemon registers again.

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
The hook also fills an empty `cwd`. The `SessionEnd` hook ends the row by the
Codex session ID.

Hooks run only when the person enables and trusts them. Without them the row
has no harness session ID; the tools work and publishes are attributed to it,
and it ends when the shim exits or through the reaper.

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

## Page contract

Every page follows this contract so it renders well in the gallery, in light and
dark mode, and on a phone:

- A `<title>` element with a short name (two to four words). Pass the same
  name as `title` when you first publish: the daemon does not read the page's
  `<title>`, and an artifact created without `title` is titled "Untitled".
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
- Every route that changes state requires `Authorization: Bearer <token>`
  with the token from `<ARTIFAX_HOME>/daemon.json` (mode 0600): creating and
  publishing artifacts, changing and deleting them, uploading and deleting
  assets, registering, joining and ending sessions, and shutting the daemon
  down. The comparison is constant time. Read routes (`GET /api/...`, the
  gallery, content, blobs) need no token, so LAN viewers can read but not
  write. Content and asset URLs are readable by anyone who can reach the
  daemon and knows the unguessable artifact or asset ID.
- `GET /api/token` hands the token to the gallery in a local browser. It
  answers only when the connection comes from a loopback address and the
  `Host` header is literally `localhost`, `127.0.0.1` or `[::1]` (with an
  optional port), which defeats DNS rebinding; otherwise it returns 403
  `not_loopback`.
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
- Published pages and uploaded files are untrusted content: Artifax never
  executes them outside the browser.
- No telemetry. The daemon makes no calls off the machine; the Claude Code
  and Codex plugins' installer script downloads a release only when no
  `artifax` binary is found.

## What is not yet available

- Comments and feedback (phase 3): the person cannot comment on a page yet,
  `feedback` in every result is always `[]`, `status` reports no `watches`,
  and there are no comment tools.
- Runtime capabilities (phase 4): `window.claude.use(name)` resolves `null`
  for every name, and `capabilities` on `publish` is stored but has no effect.
- Rooms and `sample()` (phase 5): not available.
- Pi: the extension, its session handling and its tools are tested against a
  real daemon, but no model-driven Pi session has been run end to end, because
  no model provider key exists on the build machine.
