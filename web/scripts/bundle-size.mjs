// Gzip sizes of what must load before each shell entry can render (the HTML
// and its module script with that script's static imports, per the Vite
// manifest) and of the eager bridge, against web/perf/bundle-budget.json.
// --record lowers the budgets to the measured sizes plus 10%, never raising one.
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

for (const [html, markers] of [["index.html", []], ["artifact.html", ["<!--clax:boot-->", "<!--clax:frame-->"]]]) {
  const text = read(html).toString();
  if (text.includes(`<link rel="stylesheet"`)) throw new Error(`dist/${html} links a stylesheet; the CSS must be inlined`);
  for (const m of markers) if (!text.includes(m)) throw new Error(`dist/${html} lost the ${m} marker the daemon injects at`);
}

const sizes = { gallery: entry("index.html"), artifact: entry("artifact.html"), bridge: gz("_clax/bridge.js") };
console.log(`gzip bytes: gallery ${sizes.gallery}, artifact ${sizes.artifact}, eager bridge ${sizes.bridge}`);

const budget = existsSync(budgetFile) ? JSON.parse(readFileSync(budgetFile, "utf8")) : null;
if (process.argv.includes("--record")) {
  const up = n => Math.floor(n * 1.1);
  const next = { ...(budget ?? { bridgeBaseline: sizes.bridge }) };
  for (const k of ["gallery", "artifact", "bridge"]) next[k] = Math.min(up(sizes[k]), budget?.[k] ?? Infinity);
  writeFileSync(budgetFile, JSON.stringify(next, null, 2) + "\n");
  process.exit(0);
}
if (!budget) throw new Error("web/perf/bundle-budget.json is missing; run node scripts/bundle-size.mjs --record");
let failed = false;
for (const k of ["gallery", "artifact", "bridge"]) {
  if (sizes[k] > budget[k]) { console.error(`${k}: ${sizes[k]} gzip bytes, over its budget of ${budget[k]}`); failed = true; }
}
if (failed) process.exit(1);
