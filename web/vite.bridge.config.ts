import { defineConfig } from "vite";
export default defineConfig({
  build: {
    outDir: "dist/_clax", emptyOutDir: false,
    lib: { entry: "bridge/src/bridge.ts", name: "claxBridge", formats: ["iife"], fileName: () => "bridge.js" },
    minify: true, sourcemap: false,
  },
});
