// Prints one build's module graph as JSON, for a person or an agent asking
// why a module is in a bundle: `node scripts/module-graph.mjs [shell|overlay|transport]`
// (`just web-graph`). Each module (by its path under web/, or its package
// path under node_modules) names what it imports statically and lazily, who
// imports it, and its chunk. The build writes nothing: dist/, which the
// release embeds, is never touched.
import { relative } from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "vite";

const web = fileURLToPath(new URL("..", import.meta.url));
const CONFIGS = { shell: "vite.shell.config.ts", overlay: "vite.overlay.config.ts", transport: "vite.transport.config.ts" };
const which = process.argv[2] ?? "shell";
if (!(which in CONFIGS)) throw new Error(`module-graph: no ${which} build; one of ${Object.keys(CONFIGS).join(", ")}`);

const name = id => {
  const at = id.indexOf("/node_modules/");
  return at >= 0 ? id.slice(id.lastIndexOf("/node_modules/") + "/node_modules/".length) : relative(web, id);
};
const modules = {};
const graph = {
  name: "clax-module-graph",
  apply: "build",
  enforce: "post",
  generateBundle(_, bundle) {
    const chunkOf = new Map(Object.values(bundle).flatMap(c => (c.type === "chunk" ? Object.keys(c.modules).map(id => [id, c.fileName]) : [])));
    for (const id of this.getModuleIds()) {
      const m = this.getModuleInfo(id);
      if (!m) continue;
      modules[name(id)] = {
        imports: m.importedIds.map(name),
        lazy: m.dynamicallyImportedIds.map(name),
        importers: [...m.importers, ...m.dynamicImporters].map(name),
        chunk: chunkOf.get(id) ?? null,
      };
    }
  },
};

process.chdir(web);
await build({ configFile: CONFIGS[which], logLevel: "silent", plugins: [graph], build: { write: false } });
process.stdout.write(JSON.stringify({ build: which, modules }, null, 2) + "\n");
