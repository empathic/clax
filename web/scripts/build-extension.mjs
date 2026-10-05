// Builds the Clax Chrome extension (spec 2026-10-05 §6.4) twice: into
// dist-extension/ (the release build `clax extension install` embeds) and
// dist-extension-test/ (the browser tests' build, which adds
// `host_permissions: ["<all_urls>"]` and the worker's test hook). The
// worker is one ES module; the loader and the overlay are classic scripts
// (content scripts cannot be modules); the composer and the side panel are
// HTML pages. The manifest is extension-manifest.mjs's. Nothing under
// web/extension/key/ is copied: the key reaches a build only as the
// manifest's `key`.
import { lstatSync, mkdirSync, readdirSync, rmSync, unlinkSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { build } from "vite";
import { drawIcons } from "./extension-icons.mjs";
import { extensionManifest } from "./extension-manifest.mjs";

const web = fileURLToPath(new URL("..", import.meta.url));
const root = `${web}extension/`;

/** Empties `out` but for its .gitkeep, never following a symlink out of it. */
function clean(out) {
  let s = null;
  try { s = lstatSync(out); } catch (e) { if (e.code !== "ENOENT") throw e; }
  if (s?.isSymbolicLink()) throw new Error(`${out} is a symlink; refusing to build outside web/. Replace it with a real directory.`);
  mkdirSync(out, { recursive: true });
  for (const e of readdirSync(out, { withFileTypes: true })) {
    if (e.name === ".gitkeep") continue;
    if (e.isSymbolicLink()) unlinkSync(`${out}/${e.name}`);
    else rmSync(`${out}/${e.name}`, { recursive: true, force: true });
  }
}

async function variant(out, test) {
  clean(out);
  const shared = { configFile: false, logLevel: "warn", publicDir: false, define: { __CLAX_EXT_TEST__: JSON.stringify(test), __CLAX_TEST_CLOCK__: "false" } };
  const scripts = [["sw", "sw/main.ts", "es"], ["loader", "content/loader.ts", "iife"], ["overlay", "content/overlay.ts", "iife"]];
  for (const [name, entry, format] of scripts) {
    await build({ ...shared, build: { outDir: out, emptyOutDir: false, minify: true, sourcemap: false,
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
