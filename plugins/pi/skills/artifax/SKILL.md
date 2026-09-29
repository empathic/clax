---
name: artifax
description: Use when the user wants a web page, app, dashboard, or visual they can open in a browser and comment on; publishes HTML through the artifax tools
---

# Artifax

Artifax runs a local server that hosts HTML pages ("artifacts") published by
agents. Every publish is stored as a numbered version. The person opens the
artifact URL in a browser; you never need to serve or preview the page yourself.
The tools come from the Artifax Pi extension and are named `artifax_<tool>`:
`artifax_publish`, `artifax_read`, `artifax_list`, `artifax_delete`,
`artifax_open`, `artifax_pin`, `artifax_unpin`, `artifax_asset_upload`,
`artifax_status`. The sections below name each tool without the prefix. The
person can also type `/artifax open [id]`, `/artifax list`, or `/artifax status`.

## When to publish

Publish when the result is something the person should look at or interact with
in a browser: a page, a small app, a dashboard, a chart, a report, a prototype,
a visual explanation. Do not publish for answers that read fine as chat text, or
for files that belong in the repository.

Publish a new artifact for a new piece of work. To change something you already
published (or the person asks for an edit), publish a new version of the same
artifact by passing its `id` or `url`. Tell the person the URL from the result;
call `open` only when they want it shown in their browser.

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

## Tools

Results are one JSON object in a text block, with a `feedback` array (empty until
comments exist). Failures are `{"error": {"code", "message", ...}}` with the
tool result marked as an error. Local file arguments may be absolute or
relative to the Pi session's working directory (a leading `@` is dropped).

### publish

Publishes an HTML page as a new artifact or as a new version of an existing one.

Arguments:

- `html` (string) or `file_path` (path to a local HTML file): exactly
  one. Published as `index.html`.
- `files` (object, optional): supporting files keyed by published relative path.
  Each value is `{ "path": "<local file>" }` or
  `{ "content": "...", "encoding": "utf8" | "base64" }` (exactly one of `path`
  and `content`), plus optional `content_type` (inferred from the extension
  otherwise). A `null` value removes a file carried forward from the previous
  version. `index.html` is not allowed here.
- `id` or `url` (string, optional, at most one): the artifact to update. Omit
  both to create a new artifact.
- `if_version` (integer, optional): the version the update is based on.
- `title`, `description`, `icon` (one generic word such as `chart` or `map`),
  `label` (a short name for this version): all optional.
- `capabilities` (object, optional): reserved for phase 4.

Returns `artifact_id`, `url` (for the person), `version` (the new version
number), `title`, and `files` (the published paths).

Update workflow: the `version` from the last `publish` or `read` of that artifact
is the `if_version` for the next update. Pass it whenever you might not be the
only writer, and always after time has passed or the person may have edited.
When omitted it defaults to the current version, which silently builds on
whatever is there.

Conflict: if `if_version` is stale, the call fails with error code `conflict`,
`current` (the current version number), a `current_version` object (`n`,
`label`, `created_at`, `files`, `url`), and a `hint`. Call `read` for the
current version, merge your change into it, and publish again with
`if_version` set to the current number. Never overwrite blindly.

### read

Reads a published file of an artifact as stored (unwrapped, without any
serve-time additions).

Arguments: `url_or_id` (string, required; a URL naming a version such as
`/a/<id>/v/<n>` selects it), `path` (defaults to `index.html`), `version`
(defaults to the URL's version, else current), `max_bytes` (default 200000).

Returns `artifact_id`, `version`, `path`, `content_type`, `size`, `truncated`,
and `content` for text or `content_base64` for binary files under the cap.

### list

Arguments: `limit` (integer, optional), `scope` (`all`, the default, or `mine`
for artifacts this session created).

Returns `artifacts`: objects with `id`, `url`, `title`, `version`, `pinned`,
`updated_at`, `owner_session_id`. Pinned first, then most recently updated.

### delete

Argument: `url_or_id`. Deletes the artifact and all its versions permanently.
Returns `artifact_id` and `deleted: true`. Only delete when the person asks.

### open

Argument: `url_or_id`. Opens the artifact in the person's browser on this
machine. Returns `url` and `opened` (false when no browser could be started;
give the person the URL).

### pin and unpin

Argument: `url_or_id`. Pins to the top of the gallery or removes the pin.
Returns `artifact_id` and `pinned`.

### asset_upload

Uploads local files (images, video, fonts, data) as assets of an artifact.

Arguments: `url_or_id` (required), and `file_path` or `file_paths`
(local paths). Returns `assets`: objects with `id`, `url`, `content_type`,
`size`. Reference an asset from a page by the `url` exactly as returned. Use
`files` in `publish` instead for files that belong to a version (styles,
scripts, small images).

### status

No arguments. Returns `daemon_url`, `version`, `harness`, `session` (what
publishes are attributed to), and `watches`. Use it to confirm the daemon is
reachable.

## What is not yet available

- Comments: reading the person's comments on a page arrives in phase 3. Until
  then `feedback` is always empty; do not promise that you will see comments.
- Capabilities: `window.claude.use(name)` resolves `null` for every name until
  phase 4, and `capabilities` on `publish` is stored with the artifact but has
  no effect until phase 4. Do not build pages that depend on shared state, live
  data, or asking the agent questions.
