import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// The embeddable overlay (overlay/src/main.ts), one classic script a
// deployer's page loads from the instance: `/_clax/overlay.js`, global
// `Clax`. Its components' CSS is injected where they mount, the overlay's
// shadow root, never into the page's <head>.
export default defineConfig({
  plugins: [svelte({ configFile: fileURLToPath(new URL("svelte.config.js", import.meta.url)), compilerOptions: { css: "injected" } })],
  define: { "process.env.NODE_ENV": JSON.stringify("production") },
  build: {
    outDir: "dist/_clax", emptyOutDir: false,
    lib: { entry: "overlay/src/main.ts", formats: ["iife"], name: "Clax", fileName: () => "overlay.js" },
    minify: true, sourcemap: false,
  },
});
