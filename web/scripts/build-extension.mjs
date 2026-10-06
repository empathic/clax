// Builds the Clax Chrome extension (spec 2026-10-05 §6.4) twice: into
// dist-extension/ (the release build `clax extension install` embeds) and
// dist-extension-test/ (the browser tests' build, which adds
// `host_permissions: ["<all_urls>"]` and the worker's test hook). The
// worker is one ES module; the overlay is a classic script
// (content scripts cannot be modules); the composer and the side panel are
// HTML pages. The manifest is extension-manifest.mjs's. Nothing under
// web/extension/key/ is copied: the key reaches a build only as the
// manifest's `key`.
import { writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { build, transformWithEsbuild } from "vite";
import { cleanOut } from "./extension-clean.mjs";
import { drawIcons } from "./extension-icons.mjs";
import { extensionManifest } from "./extension-manifest.mjs";

const web = fileURLToPath(new URL("..", import.meta.url));
/** Vite minifies an ES library's identifiers and syntax but keeps its
 * whitespace and comments (for the pure annotations a library's consumer
 * might use): the worker is no one's library, so it is minified whole. */
const minifyWhole = { name: "minify-whole", renderChunk: async code => (await transformWithEsbuild(code, "sw.js", { minify: true, format: "esm" })).code };
const root = `${web}extension/`;

async function variant(out, test) {
  cleanOut(out);
  const shared = { configFile: false, logLevel: "warn", publicDir: false, define: { __CLAX_EXT_TEST__: JSON.stringify(test), __CLAX_TEST_CLOCK__: "false" } };
  const scripts = [["sw", "sw/main.ts", "es"], ["overlay", "content/overlay.ts", "iife"]];
  for (const [name, entry, format] of scripts) {
    await build({ ...shared, plugins: format === "es" ? [minifyWhole] : [], build: { outDir: out, emptyOutDir: false, minify: true, sourcemap: false,
      lib: { entry: `${root}src/${entry}`, formats: [format], name: `clax_${name}`, fileName: () => `${name}.js` } } });
  }
  await build({ ...shared, root, base: "./", plugins: [svelte({ configFile: `${web}svelte.config.js` })],
    build: { outDir: out, emptyOutDir: false, minify: true, sourcemap: false, modulePreload: { polyfill: false },
      rollupOptions: { input: { composer: `${root}composer.html`, sidepanel: `${root}sidepanel.html` } } } });
  writeFileSync(`${out}/manifest.json`, `${JSON.stringify(extensionManifest(web, { test }), null, 2)}\n`);
  drawIcons(`${out}/icons`);
}

await variant(`${web}dist-extension`, false);
await variant(`${web}dist-extension-test`, true);
