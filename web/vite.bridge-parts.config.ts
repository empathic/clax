import { defineConfig, transformWithEsbuild } from "vite";
// One of the bridge's lazy parts: an ES module with a content-hashed name
// under dist/_clax/bridge/. scripts/build-parts.mjs builds each part with
// this config and its own entry, so every part is self-contained: no part
// imports another, and a part's retry after a failed load (`?retry=<n>`)
// fetches everything it needs afresh. The eager bridge build reads the
// manifest that script writes for the parts' names.
export default defineConfig({
  // Vite keeps the whitespace of an ES library build; the parts are minified
  // whole, as the eager bridge is.
  plugins: [{ name: "minify-parts", renderChunk: { order: "post", handler: (code, chunk) => transformWithEsbuild(code, chunk.fileName, { minify: true, format: "esm", target: "es2022" }) } }],
  build: {
    outDir: "dist/_clax/bridge", emptyOutDir: false, manifest: false, minify: true, sourcemap: false, target: "es2022",
    // `lib.entry` is the part's, given by scripts/build-parts.mjs.
    lib: { formats: ["es"] } as never,
    rollupOptions: {
      // `just watch` (CLAX_DEV=1) keeps stable names, so the watching bridge
      // build, which reads the names once, never points at a deleted file.
      output: process.env.CLAX_DEV ? { entryFileNames: "[name].js" } : { entryFileNames: "[name]-[hash].js" },
    },
  },
});
