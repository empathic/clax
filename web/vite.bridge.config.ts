import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";

// The eager bridge, one classic script. It names its lazy parts by the
// content-hashed files the parts build (scripts/build-parts.mjs) wrote.
type Entry = { file: string; name?: string; isEntry?: boolean };
const manifest = JSON.parse(readFileSync("dist/_clax/bridge/.vite/manifest.json", "utf8")) as Record<string, Entry>;
const parts = Object.fromEntries(Object.values(manifest).filter(e => e.isEntry && e.name).map(e => [e.name!, e.file]));
for (const name of ["comment", "clip", "caps", "room", "sample"]) if (!parts[name]) throw new Error(`the parts build has no ${name} entry; run node scripts/build-parts.mjs first`);

export default defineConfig({
  // CLAX_TEST_CLOCK=1 builds the browser tests' bridge (see shell/src/clock.ts).
  define: { __CLAX_PARTS__: JSON.stringify(parts), __CLAX_TEST_CLOCK__: JSON.stringify(process.env.CLAX_TEST_CLOCK === "1") },
  resolve: { alias: { "clax-bridge-parts": fileURLToPath(new URL("./bridge/src/parts-url.ts", import.meta.url)) } },
  build: {
    outDir: "dist/_clax", emptyOutDir: false,
    lib: { entry: "bridge/src/bridge.ts", name: "claxBridge", formats: ["iife"], fileName: () => "bridge.js" },
    minify: true, sourcemap: false,
  },
});
