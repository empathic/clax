// Gzip sizes of what must load before each shell entry can render (the HTML
// and its module script with that script's static imports, per the Vite
// manifest), of the eager bridge, of each of the bridge's lazy parts with
// the files it imports (per the parts build's manifest), and of the Chrome
// extension's release build (web/dist-extension: its worker and overlay
// scripts, and each page with the scripts and stylesheets it names),
// against web/perf/bundle-budget.json. The shell sets its type in the
// system's faces, so the build may carry no font file and no entry may load
// one. The extension's release manifest may grant no host and declare no
// content script, and its build may carry no key file and no test hook.
// --record lowers the shell's and the bridge's budgets to the measured
// sizes plus 10%, never raising one, and adds one that is missing; the
// extension's budgets are the spec's ceilings and are never recorded.
//
// Sizes are deflated by pako (a pinned JavaScript port of reference zlib), not
// node:zlib: Node links whichever zlib its build chose (the official builds
// carry Chromium's fork, Homebrew's the system zlib), and those give different
// bytes for the same input at the same level, so a budget measured on one
// machine failed on another by a hundred bytes. pako gives reference zlib's
// output on every platform and Node version.
import { existsSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import pako from "pako";

const dist = new URL("../dist/", import.meta.url);
const budgetFile = new URL("../perf/bundle-budget.json", import.meta.url);
const read = p => readFileSync(new URL(p, dist));
const gz = p => pako.gzip(read(p), { level: 9 }).length;
const manifest = JSON.parse(read(".vite/manifest.json"));

function closure(key, seen = new Set()) {
  if (seen.has(key)) return seen;
  seen.add(key);
  for (const k of manifest[key].imports ?? []) closure(k, seen);
  return seen;
}
const entry = html => {
  const files = [...closure(html)].map(k => manifest[k].file);
  return gz(html) + files.reduce((n, f) => n + gz(f), 0);
};

for (const [html, markers] of [["index.html", []], ["artifact.html", ["<script id=\"clax-early\">", "<!--clax:boot-->", "<!--clax:frame-->", "<h1>Clax</h1>"]]]) {
  const text = read(html).toString();
  if (text.includes(`<link rel="stylesheet"`)) throw new Error(`dist/${html} links a stylesheet; the CSS must be inlined`);
  for (const m of markers) if (!text.includes(m)) throw new Error(`dist/${html} lost ${m}, which the daemon injects at or the shell needs`);
  // The early listener and its guard must run before the stage is parsed,
  // and before the bootstrap block.
  if (html === "artifact.html") {
    const early = text.indexOf(`<script id="clax-early">`);
    if (!(early < text.indexOf("<!--clax:boot-->") && early < text.indexOf("<body"))) throw new Error(`dist/${html}: the clax-early script must come before the bootstrap marker and <body>`);
  }
}

// The browser tests' clock hook (shell/src/clock.ts) is built only with
// CLAX_TEST_CLOCK=1: no file of the release bundle may carry it.
const filesIn = dir => readdirSync(dir, { withFileTypes: true }).flatMap(e => e.isDirectory() ? filesIn(new URL(`${e.name}/`, dir)) : [new URL(e.name, dir)]);
for (const f of filesIn(dist)) {
  const text = readFileSync(f, "latin1");
  if (text.includes("claxTestClock") || text.includes("clax.test-clock")) throw new Error(`${f.pathname} carries the test clock hook; web/dist must be built without CLAX_TEST_CLOCK`);
}

// Fonts: none. No font file anywhere in the build, and no entry preloads
// one or declares an @font-face that fetches one.
const FONT_FILE = /\.(woff2?|ttf|otf|eot)$/i;
for (const f of filesIn(dist)) {
  if (FONT_FILE.test(f.pathname)) throw new Error(`${f.pathname} is a font file; the shell uses the system's faces and ships none`);
}
for (const html of ["index.html", "artifact.html"]) {
  const text = read(html).toString();
  // Each <link> tag on its own, so attribute order and quoting do not matter.
  const preloadsFont = [...text.matchAll(/<link\b[^>]*>/gi)].some(([tag]) => /\brel\s*=\s*["']?[^"'>]*\bpreload\b/i.test(tag) && (/\bas\s*=\s*["']?font\b/i.test(tag) || /\.(woff2?|ttf|otf)\b/i.test(tag)));
  if (preloadsFont) throw new Error(`dist/${html} preloads a font; the shell uses the system's faces`);
  for (const face of text.matchAll(/@font-face\s*\{([^}]*)\}/g)) {
    if (/url\(/.test(face[1])) throw new Error(`dist/${html}: an @font-face fetches a font; the shell uses the system's faces`);
  }
}

const partsManifest = JSON.parse(read("_clax/bridge/.vite/manifest.json"));
const partKeys = { comment: "partComment", clip: "partClip", caps: "partCaps", room: "partRoom", sample: "partSample" };
function part(name) {
  const key = Object.keys(partsManifest).find(k => partsManifest[k].isEntry && partsManifest[k].name === name);
  if (!key) throw new Error(`dist/_clax/bridge has no ${name} part`);
  const seen = new Set();
  const walk = k => { if (seen.has(k)) return; seen.add(k); for (const i of partsManifest[k].imports ?? []) walk(i); };
  walk(key);
  return [...seen].reduce((n, k) => n + gz(`_clax/bridge/${partsManifest[k].file}`), 0);
}

const sizes = { gallery: entry("index.html"), artifact: entry("artifact.html"), bridge: gz("_clax/bridge.js") };
for (const [name, key] of Object.entries(partKeys)) sizes[key] = part(name);

// The extension's release build.
const ext = new URL("../dist-extension/", import.meta.url);
const extGz = f => pako.gzip(readFileSync(new URL(f, ext)), { level: 9 }).length;
const extManifest = JSON.parse(readFileSync(new URL("manifest.json", ext), "utf8"));
for (const k of ["host_permissions", "content_scripts"]) {
  if (k in extManifest) throw new Error(`dist-extension/manifest.json declares ${k}; the release build grants hosts only at runtime`);
}
// The worker's test hook (`globalThis.claxTest`, built only when
// __CLAX_EXT_TEST__ is true) belongs to dist-extension-test alone.
for (const f of filesIn(ext)) {
  const text = readFileSync(f, "latin1");
  if (text.includes("claxTest") || text.includes("__CLAX_EXT_TEST__")) throw new Error(`${f.pathname} carries the extension's test hook; dist-extension must be built with __CLAX_EXT_TEST__ false`);
}
for (const f of filesIn(ext)) {
  if (/(^|\/)key(\/|\.pub)/.test(f.pathname.slice(ext.pathname.length))) throw new Error(`${f.pathname} is a key file; the key reaches the build only as the manifest's key`);
}
sizes.extOverlay = extGz("overlay.js");
sizes.extWorker = extGz("sw.js");
// The pages: the HTML and every script and stylesheet in assets/ it names.
for (const [key, html] of [["extComposer", "composer.html"], ["extPanel", "sidepanel.html"]]) {
  const text = readFileSync(new URL(html, ext), "utf8");
  const files = [...text.matchAll(/(?:src|href)="\.\/(assets\/[^"]+)"/g)].map(m => m[1]);
  sizes[key] = extGz(html) + files.reduce((n, f) => n + extGz(f), 0);
}
console.log(`gzip bytes: gallery ${sizes.gallery}, artifact ${sizes.artifact}, eager bridge ${sizes.bridge}, parts: comment ${sizes.partComment}, clip ${sizes.partClip}, caps ${sizes.partCaps}, room ${sizes.partRoom}, sample ${sizes.partSample}, extension: overlay ${sizes.extOverlay}, worker ${sizes.extWorker}, composer ${sizes.extComposer}, panel ${sizes.extPanel}`);

const RECORDED = ["gallery", "artifact", "bridge", ...Object.values(partKeys)];
const EXTENSION = ["extOverlay", "extWorker", "extComposer", "extPanel"];
const MEASURED = [...RECORDED, ...EXTENSION];
const KEYS = [...MEASURED, "bridgeBaseline"];
const budget = existsSync(budgetFile) ? JSON.parse(readFileSync(budgetFile, "utf8")) : null;
// A missing or non-numeric budget would turn its check off; refuse it instead.
// Recording may add a missing recorded budget, never the baseline or an
// extension ceiling.
const bad = (budget ? KEYS.filter(k => !Number.isFinite(budget[k])) : []).filter(k => !process.argv.includes("--record") || !RECORDED.includes(k));
if (bad.length) {
  console.error(`web/perf/bundle-budget.json lacks a numeric budget for: ${bad.join(", ")}`);
  process.exit(1);
}
if (process.argv.includes("--record")) {
  const up = n => Math.floor(n * 1.1);
  const next = { ...(budget ?? { bridgeBaseline: sizes.bridge }) };
  for (const k of RECORDED) next[k] = Math.min(up(sizes[k]), Number.isFinite(budget?.[k]) ? budget[k] : Infinity);
  writeFileSync(budgetFile, JSON.stringify(next, null, 2) + "\n");
  process.exit(0);
}
if (!budget) throw new Error("web/perf/bundle-budget.json is missing; run node scripts/bundle-size.mjs --record");
let failed = false;
for (const k of MEASURED) {
  if (sizes[k] > budget[k]) { console.error(`${k}: ${sizes[k]} gzip bytes, over its budget of ${budget[k]}`); failed = true; }
}
if (failed) process.exit(1);
