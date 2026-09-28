import { defineConfig } from "vite";
import preact from "@preact/preset-vite";
export default defineConfig({
  root: "shell", base: "/_artifax/shell/", plugins: [preact()],
  build: { outDir: "../dist", emptyOutDir: false, assetsDir: "shell", rollupOptions: { input: "shell/index.html" } },
  server: { proxy: { "/api": "http://127.0.0.1:7480", "/c": "http://127.0.0.1:7480", "/_blob": "http://127.0.0.1:7480", "/healthz": "http://127.0.0.1:7480" } },
});
