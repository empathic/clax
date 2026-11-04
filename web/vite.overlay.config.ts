import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { defineConfig, type Plugin } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { componentCssIntoScript } from "./scripts/overlay-component-css.mjs";

/** The two builds of the one entry: the classic script and its ES module twin. */
const OUTPUTS = { iife: "overlay.js", es: "overlay.mjs" } as const;

/** Fails the build unless each output (overlay.js, overlay.mjs) is one
 * file, as an IIFE cannot split and the module is built the same; and
 * serves the host API's types beside the script (/_clax/overlay.d.ts),
 * once. */
function oneFileWithTypes(): Plugin {
  return {
    name: "clax-overlay-one-file",
    apply: "build",
    enforce: "post",
    generateBundle(out, bundle) {
      const chunks = Object.values(bundle).filter(f => f.type === "chunk");
      if (chunks.length !== 1) return this.error(`the overlay built ${chunks.length} chunks; it must be one file`);
      if (out.format === "iife") this.emitFile({ type: "asset", fileName: "overlay.d.ts", source: readFileSync(new URL("overlay/overlay.d.ts", import.meta.url), "utf8") });
    },
  };
}

// The embeddable overlay and its loader (overlay/src/script.ts), one classic script a
// deployer's page loads from the instance: `/_clax/overlay.js`, global
// `Clax`; and the same entry as one ES module, `/_clax/overlay.mjs`, for
// bundlers. Neither loads anything lazily. Its components' CSS is extracted and written into the script
// (componentCssIntoScript), for the overlay's shadow root, never the page's
// <head>.
export default defineConfig({
  plugins: [svelte({ configFile: fileURLToPath(new URL("svelte.config.js", import.meta.url)) }), componentCssIntoScript(Object.values(OUTPUTS)), oneFileWithTypes()],
  define: { "process.env.NODE_ENV": JSON.stringify("production") },
  build: {
    outDir: "dist/_clax", emptyOutDir: false,
    lib: { entry: "overlay/src/script.ts", formats: ["iife", "es"], name: "Clax", fileName: format => OUTPUTS[format as keyof typeof OUTPUTS] },
    rollupOptions: { output: { inlineDynamicImports: true } },
    minify: true, sourcemap: false, cssCodeSplit: false,
  },
});
