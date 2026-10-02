// Builds the bridge's lazy parts (bridge/src/parts/{comment,clip,caps,room}.ts),
// each on its own with vite.bridge-parts.config.ts, so no part imports
// another: a browser keeps a failed module load for its URL, and a part that
// imported another's file could not recover from that file failing once,
// whatever query its own retry carries. Shared code (anchoring, say) is
// bundled into each part that uses it.
//
// Writes dist/_clax/bridge/.vite/manifest.json ({ <src>: { file, name,
// isEntry } }), which the eager bridge build and the bundle gate read; the
// daemon never serves it. `--watch` rebuilds each part on change; it needs
// CLAX_DEV=1 (stable names), as `just watch` sets. It keeps what the one-shot
// build before it wrote (the same names), so the parts and the manifest the
// bridge watcher reads are never missing, and it replaces the manifest
// atomically.
import { mkdirSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { build } from "vite";

const web = fileURLToPath(new URL("..", import.meta.url));
const out = `${web}dist/_clax/bridge`;
const PARTS = ["comment", "clip", "caps", "room"];
const watch = process.argv.includes("--watch");
if (watch && !process.env.CLAX_DEV) throw new Error("build-parts --watch needs CLAX_DEV=1, so the parts keep stable names");

if (!watch) rmSync(out, { recursive: true, force: true });
const manifest = {};
for (const name of PARTS) {
  const src = `bridge/src/parts/${name}.ts`;
  const result = await build({
    root: web,
    configFile: `${web}vite.bridge-parts.config.ts`,
    build: { lib: { entry: { [name]: src } }, ...(watch ? { watch: {} } : {}) },
  });
  let file = `${name}.js`;
  if (!watch) {
    const chunks = (Array.isArray(result) ? result : [result]).flatMap(r => r.output);
    const own = chunks.filter(c => c.type === "chunk");
    if (own.length !== 1 || !own[0].isEntry) throw new Error(`the ${name} part must build to one file, not ${own.map(c => c.fileName).join(", ")}`);
    file = own[0].fileName;
  }
  manifest[src] = { file, name, isEntry: true };
}
mkdirSync(`${out}/.vite`, { recursive: true });
writeFileSync(`${out}/.vite/manifest.json.tmp`, JSON.stringify(manifest, null, 2) + "\n");
renameSync(`${out}/.vite/manifest.json.tmp`, `${out}/.vite/manifest.json`);
