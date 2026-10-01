import { defineConfig } from "vite";
import preact from "@preact/preset-vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
export default defineConfig({
  root: "shell", base: "/", plugins: [preact(), svelte()],
  build: { outDir: "../dist", emptyOutDir: false, assetsDir: "_clax/shell", rollupOptions: { input: "shell/index.html" } },
  server: { proxy: { "/api": "http://127.0.0.1:7480", "/c": "http://127.0.0.1:7480", "/_blob": "http://127.0.0.1:7480", "/healthz": "http://127.0.0.1:7480" } },
});
