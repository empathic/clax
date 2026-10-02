// Gzip sizes of what must load before each shell entry can render (the HTML
// and its module script with that script's static imports, per the Vite
// manifest), of the eager bridge, and of each of the bridge's lazy parts with
// the files it imports (per the parts build's manifest), against
// web/perf/bundle-budget.json. --record lowers the budgets to the measured
// sizes plus 10%, never raising one, and adds a budget that is missing.
import { existsSync, readFileSync, writeFileSync } from "node:fs";
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

const partsManifest = JSON.parse(read("_clax/bridge/.vite/manifest.json"));
const partKeys = { comment: "partComment", clip: "partClip", caps: "partCaps", room: "partRoom" };
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
console.log(`gzip bytes: gallery ${sizes.gallery}, artifact ${sizes.artifact}, eager bridge ${sizes.bridge}, parts: comment ${sizes.partComment}, clip ${sizes.partClip}, caps ${sizes.partCaps}, room ${sizes.partRoom}`);

const MEASURED = ["gallery", "artifact", "bridge", ...Object.values(partKeys)];
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
