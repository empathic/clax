import { defineConfig } from "vite";
import preact from "@preact/preset-vite";
export default defineConfig({
  root: "shell", base: "/", plugins: [preact()],
  build: { outDir: "../dist", emptyOutDir: false, assetsDir: "_clax/shell", rollupOptions: { input: "shell/index.html" } },
  server: { proxy: { "/api": "http://127.0.0.1:7481", "/c": "http://127.0.0.1:7481", "/_blob": "http://127.0.0.1:7481", "/healthz": "http://127.0.0.1:7481" } },
});
