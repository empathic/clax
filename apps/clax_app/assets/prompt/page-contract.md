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
- `window.claude.use(name)` is the entry point for runtime capabilities (see
  "Runtime capabilities"). It resolves `null` for undeclared names (other than
  `permissions` and `user`) and outside the viewer, so pages must handle
  `null` and work without it.
- Supporting files (stylesheets, scripts, images, data) are referenced with
  relative paths, for example `<link rel="stylesheet" href="style.css">`, and
  published under the same relative path in `files`. Do not use absolute
  `/...` paths or hard-coded hosts.
- A page loads and fetches only its own files, its assets, `data:` and
  `blob:` URLs: no scripts, styles, fonts or images from CDNs or other
  sites, and no `fetch`, `XMLHttpRequest`, `WebSocket` or `EventSource` to
  anywhere else. The daemon's content policy refuses the rest. Data from
  the web comes through the `net` capability.
- `icon.svg` at the top of the files is the app's icon, a square
  full-bleed design. Every publish refuses one that breaks these rules
  (`invalid_icon`), and a version a git push makes leaves it out, naming
  it in the version's note; restoring or forking a version copies its icon
  as it is. The rules: at most 32 KB of well-formed XML without a
  `<!DOCTYPE>`, its elements nested at most 64 deep; an `<svg>` root with
  a square `viewBox` of four numbers, such as `0 0 1024 1024`; only the
  elements `svg`, `g`, `defs`, `title`, `desc`, `path`, `rect`, `circle`,
  `ellipse`, `line`, `polyline`, `polygon`, `linearGradient`,
  `radialGradient`, `stop`, `clipPath`, `mask`, `use`, `symbol`, `text`
  and `tspan`; no `on*` attribute and no `filter` attribute; `href` (or
  `xlink:href`, its prefix declared) only to `#id` within the file, and
  every `url()` a `url(#id)`; no animation, filter or `@` rule in a
  `style`; `<use>`s drawing at most 64 copies, nested at most 3 deep.
  Other `.svg` files are ordinary files.

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

