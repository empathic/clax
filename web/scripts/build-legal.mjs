// Renders the Terms of Service and the Privacy Policy (docs/legal/*.md) into
// web/dist/terms.html and web/dist/privacy.html: standalone pages, in the
// shell's colors and theme, that a hosted Clax serving the bundled
// documents answers /terms and /privacy with (crates/clax-server/src/hosted.rs,
// legal_page). They load no script but the theme's and no other file.
import { mkdirSync, readFileSync, realpathSync, writeFileSync } from "node:fs";
import { fileURLToPath, pathToFileURL } from "node:url";
import { marked } from "marked";

const docs = fileURLToPath(new URL("../../docs/legal/", import.meta.url));
const dist = fileURLToPath(new URL("../dist/", import.meta.url));

export const PAGES = [
  { slug: "terms", title: "Terms of Service", other: { slug: "privacy", title: "Privacy Policy" } },
  { slug: "privacy", title: "Privacy Policy", other: { slug: "terms", title: "Terms of Service" } },
];

const escape = s => s.replace(/[&<>"]/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);

/** A table whose first row is one label and empty cells is a banner over
 * the whole table: Markdown has no colspan, so the label becomes one
 * header cell spanning every column. */
export function collapseBannerHeaders(html) {
  return html.replace(/<thead>\s*<tr>([\s\S]*?)<\/tr>\s*<\/thead>/g, (match, inner) => {
    const cells = [...inner.matchAll(/<th\b[^>]*>([\s\S]*?)<\/th>/g)];
    const labelled = cells.filter(c => c[1].trim() !== "");
    if (cells.length < 2 || labelled.length !== 1) return match;
    return `<thead><tr><th colspan="${cells.length}">${labelled[0][1]}</th></tr></thead>`;
  });
}

/** Links to the documents' own hosted addresses (https://clax.dev/terms and
 * /privacy) stay on the instance serving them. Every other link to another
 * site opens in a new tab. */
export function localLinks(html) {
  return html
    .replace(/href="https:\/\/clax\.dev\/(terms|privacy)"/g, 'href="/$1"')
    .replace(/<a href="(https?:[^"]*)"/g, '<a href="$1" target="_blank" rel="noopener noreferrer"');
}

/** One page: `markdown` rendered in the page's frame. */
export function page({ title, other }, markdown) {
  const body = localLinks(collapseBannerHeaders(marked.parse(markdown, { async: false })));
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="color-scheme" content="light dark">
<title>${escape(title)} · Clax</title>
<script>try{var t=localStorage.getItem("clax.theme");if(t==="light"||t==="dark")document.documentElement.dataset.theme=t}catch(e){}</script>
<style>
:root { --bg: #f7f7f5; --card: #ffffff; --fg: #1c1b19; --muted: #5f5d58; --border: #e6e5e1; --link: #2f6f2a; --you: #e0532f; --agent: #2f6f2a;
  --font: ui-sans-serif, -apple-system, "Segoe UI", system-ui, sans-serif; color-scheme: light; }
@media (prefers-color-scheme: dark) { :root:not([data-theme="light"]) {
  --bg: #141413; --card: #1c1c1a; --fg: #ecebe7; --muted: #a9a69f; --border: #2c2c29; --link: #7cc36f; --you: #ff7a57; --agent: #7cc36f; color-scheme: dark; } }
:root[data-theme="dark"] { --bg: #141413; --card: #1c1c1a; --fg: #ecebe7; --muted: #a9a69f; --border: #2c2c29; --link: #7cc36f; --you: #ff7a57; --agent: #7cc36f; color-scheme: dark; }
* { box-sizing: border-box; }
html, body { margin: 0; }
body { background: var(--bg); color: var(--fg); font: 15px/1.6 var(--font); -webkit-font-smoothing: antialiased; }
header { display: flex; align-items: center; gap: 10px; padding: 12px 16px; border-bottom: 1px solid var(--border); }
header a.home { display: flex; align-items: center; gap: 8px; color: var(--fg); text-decoration: none; font-weight: 600; }
header svg { width: 28px; height: 17px; }
header svg .l { stroke: var(--you); }
header svg .r { stroke: var(--agent); }
header svg circle { fill: var(--fg); }
header nav { margin-left: auto; font-size: 14px; }
main { max-width: 720px; margin: 0 auto; padding: 24px 16px 64px; }
h1 { font-size: 26px; line-height: 1.25; margin: 8px 0 4px; }
.doc > p:first-child { color: var(--muted); margin-top: 0; }
.doc h2, .doc h3 { font-size: 17px; margin: 28px 0 8px; }
a { color: var(--link); }
.doc ol, .doc ul { padding-left: 24px; }
.doc li { margin: 6px 0; }
.doc table { width: 100%; border-collapse: collapse; margin: 16px 0; font-size: 14px; display: block; overflow-x: auto; }
.doc th, .doc td { border: 1px solid var(--border); padding: 8px 10px; text-align: left; vertical-align: top; }
.doc th { background: var(--card); }
footer { border-top: 1px solid var(--border); padding: 16px; text-align: center; font-size: 13px; color: var(--muted); }
footer a { color: var(--muted); }
</style>
</head>
<body>
<header>
<a class="home" href="/"><svg viewBox="0 0 40 24" aria-hidden="true" focusable="false"><path class="l" d="M2 2.5a9.5 9.5 0 0 1 0 19" fill="none" stroke-width="4.2" stroke-linecap="round"/><path class="r" d="M38 2.5a9.5 9.5 0 0 0 0 19" fill="none" stroke-width="4.2" stroke-linecap="round"/><circle cx="20" cy="12" r="2.6"/></svg>Clax</a>
<nav><a href="/${other.slug}">${escape(other.title)}</a></nav>
</header>
<main>
<h1>${escape(title)}</h1>
<div class="doc">
${body}</div>
</main>
<footer><a href="/terms">Terms of Service</a> · <a href="/privacy">Privacy Policy</a> · <a href="https://github.com/empathic/clax" target="_blank" rel="noopener noreferrer">Source</a></footer>
</body>
</html>
`;
}

if (process.argv[1] && import.meta.url === pathToFileURL(realpathSync(process.argv[1])).href) {
  mkdirSync(dist, { recursive: true });
  for (const p of PAGES) writeFileSync(`${dist}${p.slug}.html`, page(p, readFileSync(`${docs}${p.slug}.md`, "utf8")));
}
