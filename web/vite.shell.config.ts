import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
// The root is shell/, so the plugin would not find svelte.config.js (runes mode) by itself.
const SVELTE_CONFIG = fileURLToPath(new URL("svelte.config.js", import.meta.url));
export default defineConfig({
  root: "shell", base: "/", plugins: [svelte({ configFile: SVELTE_CONFIG })],
  build: { outDir: "../dist", emptyOutDir: false, assetsDir: "_clax/shell", rollupOptions: { input: "shell/index.html" } },
  server: { proxy: { "/api": "http://127.0.0.1:7481", "/c": "http://127.0.0.1:7481", "/_blob": "http://127.0.0.1:7481", "/healthz": "http://127.0.0.1:7481" } },
});
