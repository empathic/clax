import { defineConfig, transformWithEsbuild } from "vite";
// The bridge's lazy parts: ES modules with content-hashed names under
// dist/_clax/bridge/. The eager bridge build reads this build's manifest for
// their names. Every module comment mode needs goes into the comment part's
// own file, so comment mode loads in one request; the clip and caps parts
// import what they share with it from that file.
export default defineConfig({
  // Vite keeps the whitespace of an ES library build; the parts are minified
  // whole, as the eager bridge is.
  plugins: [{ name: "minify-parts", renderChunk: { order: "post", handler: (code, chunk) => transformWithEsbuild(code, chunk.fileName, { minify: true, format: "esm", target: "es2022" }) } }],
  build: {
    outDir: "dist/_clax/bridge", emptyOutDir: true, manifest: true, minify: true, sourcemap: false, target: "es2022",
    lib: { entry: { comment: "bridge/src/parts/comment.ts", clip: "bridge/src/parts/clip.ts", caps: "bridge/src/parts/caps.ts" }, formats: ["es"] },
    rollupOptions: {
      // The comment part's file may export more than its entry does (what
      // the other parts import from it), so it needs no separate facade.
      preserveEntrySignatures: "allow-extension",
      output: {
        manualChunks(id, { getModuleInfo }) {
          const seen = new Set<string>();
          const forComment = (m: string): boolean => {
            if (m.endsWith("/bridge/src/parts/comment.ts")) return true;
            if (seen.has(m)) return false;
            seen.add(m);
            return (getModuleInfo(m)?.importers ?? []).some(forComment);
          };
          return forComment(id) ? "comment" : undefined;
        },
        // `just watch` (CLAX_DEV=1) keeps stable names, so the watching bridge
        // build, which reads the names once, never points at a deleted file.
        ...(process.env.CLAX_DEV ? { entryFileNames: "[name].js", chunkFileNames: "shared-[name].js" } : { entryFileNames: "[name]-[hash].js", chunkFileNames: "shared-[hash].js" }),
      },
    },
  },
});
