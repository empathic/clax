import { defineConfig } from "vite";
export default defineConfig({
  build: {
    outDir: "dist/_artifax", emptyOutDir: false,
    lib: { entry: "bridge/src/bridge.ts", name: "artifaxBridge", formats: ["iife"], fileName: () => "bridge.js" },
    minify: true, sourcemap: false,
  },
});
