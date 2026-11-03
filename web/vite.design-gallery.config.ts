import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// The design gallery (design-gallery/, design/README.md "The gallery"): a
// development page, built alone into dist-design-gallery/ with relative
// URLs, so its static files can be published as a Clax artifact and opened
// in a WebView. No production build reads this config. Components inject
// their CSS where they mount, so the overlay's panels style their own
// shadow root as they do on a deployer's page.
export default defineConfig({
  root: fileURLToPath(new URL("design-gallery", import.meta.url)),
  base: "./",
  plugins: [svelte({ configFile: fileURLToPath(new URL("svelte.config.js", import.meta.url)), compilerOptions: { css: "injected" } })],
  define: { __CLAX_TEST_CLOCK__: "false", __CLAX_EXT_TEST__: "false" },
  build: { outDir: fileURLToPath(new URL("dist-design-gallery", import.meta.url)), emptyOutDir: true, sourcemap: false },
});
