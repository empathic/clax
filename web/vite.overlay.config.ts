import { fileURLToPath } from "node:url";
import { defineConfig, type Plugin } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

/** The string literal in overlay/src/main.ts that the components' CSS replaces. */
const CSS_SLOT = "__CLAX_OVERLAY_COMPONENT_CSS__";

/** The two builds of the one entry: the classic script and its ES module twin. */
const OUTPUTS = { iife: "overlay.js", es: "overlay.mjs" } as const;

/** Moves the CSS the build extracts from the Svelte components into
 * each output (overlay.js, overlay.mjs), as the string `CSS_SLOT` stood
 * for, and emits no .css file:
 * the overlay adopts it as a constructed sheet in its shadow root, which a
 * page's CSP `style-src` without 'unsafe-inline' allows, where the <style>
 * elements Svelte's injected CSS makes are refused. */
function componentCssIntoScript(): Plugin {
  return {
    name: "clax-overlay-component-css",
    apply: "build",
    enforce: "post",
    generateBundle(_, bundle) {
      const sheets = Object.values(bundle).filter(f => f.type === "asset" && f.fileName.endsWith(".css"));
      const css = sheets.map(f => (f.type === "asset" ? String(f.source) : "")).join("\n");
      for (const f of sheets) delete bundle[f.fileName];
      const chunks = Object.values(bundle).filter(f => f.type === "chunk");
      const js = chunks.find(f => (Object.values(OUTPUTS) as string[]).includes(f.fileName));
      // One file per output: an IIFE cannot split, and the module is built the same.
      if (chunks.length !== 1) return this.error(`the overlay built ${chunks.length} chunks; it must be one file`);
      const slot = new RegExp(`(["'\`])${CSS_SLOT}\\1`);
      if (js?.type !== "chunk" || !slot.test(js.code)) return this.error(`${js?.fileName ?? "the overlay"} has no ${CSS_SLOT} for the components' CSS`);
      if (!css) return this.error("the overlay's components compiled to no CSS");
      js.code = js.code.replace(slot, () => JSON.stringify(css));
    },
  };
}

// The embeddable overlay (overlay/src/main.ts), one classic script a
// deployer's page loads from the instance: `/_clax/overlay.js`, global
// `Clax`; and the same entry as one ES module, `/_clax/overlay.mjs`, for
// bundlers. Neither loads anything lazily. Its components' CSS is extracted and written into the script
// (componentCssIntoScript), for the overlay's shadow root, never the page's
// <head>.
export default defineConfig({
  plugins: [svelte({ configFile: fileURLToPath(new URL("svelte.config.js", import.meta.url)) }), componentCssIntoScript()],
  define: { "process.env.NODE_ENV": JSON.stringify("production") },
  build: {
    outDir: "dist/_clax", emptyOutDir: false,
    lib: { entry: "overlay/src/main.ts", formats: ["iife", "es"], name: "Clax", fileName: format => OUTPUTS[format as keyof typeof OUTPUTS] },
    rollupOptions: { output: { inlineDynamicImports: true } },
    minify: true, sourcemap: false, cssCodeSplit: false,
  },
});
