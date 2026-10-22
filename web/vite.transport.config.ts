import { defineConfig } from "vite";

// The app's transport (shell/src/transport.ts), one classic script the core
// writes into the shell documents it serves in the Clax app, so it runs
// before the shell's modules; a browser's shell never loads it.
export default defineConfig({
  build: {
    outDir: "dist/_clax", emptyOutDir: false,
    lib: { entry: "shell/src/transport-main.ts", formats: ["iife"], name: "claxTransportScript", fileName: () => "transport.js" },
    minify: true, sourcemap: false,
  },
});
