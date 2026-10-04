// Gzip sizes of what must load before each shell entry can render (the HTML
// and its module script with that script's static imports, per the Vite
// manifest), of the eager bridge, of each of the bridge's lazy parts with
// the files it imports (per the parts build's manifest), and the raw bytes of
// the self-hosted WOFF2 fonts, against
// web/perf/bundle-budget.json. --record lowers the budgets to the measured
// sizes plus 10%, never raising one, and adds a budget that is missing.
import { existsSync, readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { gzipSync } from "node:zlib";

const dist = new URL("../dist/", import.meta.url);
const budgetFile = new URL("../perf/bundle-budget.json", import.meta.url);
const read = p => readFileSync(new URL(p, dist));
const gz = p => gzipSync(read(p), { level: 9 }).length;
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

// Fonts: at most three WOFF2 files, every @font-face swap, none preloaded;
// their bytes (already compressed) are budgeted as `fonts`.
const fontDir = new URL("_clax/fonts/", dist);
const woffs = readdirSync(fontDir).filter(f => f.endsWith(".woff2"));
if (woffs.length > 3) throw new Error(`dist/_clax/fonts holds ${woffs.length} WOFF2 files; at most 3`);
for (const html of ["index.html", "artifact.html"]) {
  const text = read(html).toString();
  // Each <link> tag on its own, so attribute order and quoting do not matter.
  const preloadsFont = [...text.matchAll(/<link\b[^>]*>/gi)].some(([tag]) => /\brel\s*=\s*["']?[^"'>]*\bpreload\b/i.test(tag) && /\.woff2\b/i.test(tag));
  if (preloadsFont) throw new Error(`dist/${html} preloads a font; fonts must never block or jump the queue`);
  for (const face of text.matchAll(/@font-face\s*\{([^}]*)\}/g)) {
    if (/url\(/.test(face[1]) && !/font-display:\s*swap/.test(face[1])) throw new Error(`dist/${html}: an @font-face without font-display: swap`);
  }
}
const fontBytes = woffs.reduce((n, f) => n + statSync(new URL(f, fontDir)).size, 0);

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
sizes.fonts = fontBytes;
console.log(`gzip bytes: gallery ${sizes.gallery}, artifact ${sizes.artifact}, eager bridge ${sizes.bridge}, parts: comment ${sizes.partComment}, clip ${sizes.partClip}, caps ${sizes.partCaps}, room ${sizes.partRoom}, sample ${sizes.partSample}; raw bytes: fonts ${sizes.fonts}`);

const MEASURED = ["gallery", "artifact", "bridge", ...Object.values(partKeys), "fonts"];
const KEYS = [...MEASURED, "bridgeBaseline"];
const budget = existsSync(budgetFile) ? JSON.parse(readFileSync(budgetFile, "utf8")) : null;
// A missing or non-numeric budget would turn its check off; refuse it instead.
// Recording may add a missing measured budget, never the baseline.
const bad = (budget ? KEYS.filter(k => !Number.isFinite(budget[k])) : []).filter(k => !process.argv.includes("--record") || !MEASURED.includes(k));
if (bad.length) {
  console.error(`web/perf/bundle-budget.json lacks a numeric budget for: ${bad.join(", ")}`);
  process.exit(1);
}
if (process.argv.includes("--record")) {
  const up = n => Math.floor(n * 1.1);
  const next = { ...(budget ?? { bridgeBaseline: sizes.bridge }) };
  for (const k of MEASURED) next[k] = Math.min(up(sizes[k]), Number.isFinite(budget?.[k]) ? budget[k] : Infinity);
  writeFileSync(budgetFile, JSON.stringify(next, null, 2) + "\n");
  process.exit(0);
}
if (!budget) throw new Error("web/perf/bundle-budget.json is missing; run node scripts/bundle-size.mjs --record");
let failed = false;
for (const k of MEASURED) {
  if (sizes[k] > budget[k]) { console.error(`${k}: ${sizes[k]} gzip bytes, over its budget of ${budget[k]}`); failed = true; }
}
if (failed) process.exit(1);
