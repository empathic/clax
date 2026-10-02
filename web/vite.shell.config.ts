import { fileURLToPath } from "node:url";
import { defineConfig, type Plugin } from "vite";
import type { OutputAsset } from "rollup";
import { svelte } from "@sveltejs/vite-plugin-svelte";
// The root is shell/, so the plugin would not find svelte.config.js (runes mode) by itself.
const SVELTE_CONFIG = fileURLToPath(new URL("svelte.config.js", import.meta.url));

const escape = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/** Replaces each entry's stylesheet link with the stylesheet itself, so no
 * request blocks the first render, and drops the separate CSS files. */
function inlineCss(): Plugin {
  return {
    name: "clax-inline-css",
    apply: "build",
    enforce: "post",
    generateBundle(_options, bundle) {
      const sheets = Object.values(bundle).filter((f): f is OutputAsset => f.type === "asset" && f.fileName.endsWith(".css"));
      // The CSS of chunks an entry imports statically is linked from its HTML,
      // so it is inlined below. A lazily loaded chunk's CSS would be fetched by
      // Vite's preload helper from a file this plugin deletes: refuse it.
      const chunks = new Map(Object.values(bundle).flatMap(c => (c.type === "chunk" ? [[c.fileName, c] as const] : [])));
      const lazy = new Set<string>();
      const visit = (name: string) => {
        if (lazy.has(name)) return;
        lazy.add(name);
        for (const i of chunks.get(name)?.imports ?? []) visit(i);
      };
      for (const c of chunks.values()) if (c.isDynamicEntry) visit(c.fileName);
      // A chunk an entry also imports statically has its CSS inlined already.
      const eager = new Set<string>();
      const walk = (name: string) => {
        if (eager.has(name)) return;
        eager.add(name);
        for (const i of chunks.get(name)?.imports ?? []) walk(i);
      };
      for (const c of chunks.values()) if (c.isEntry) walk(c.fileName);
      for (const name of eager) lazy.delete(name);
      for (const name of lazy) {
        if (chunks.get(name)?.viteMetadata?.importedCss.size) throw new Error(`${name} is loaded lazily and imports CSS; the shell's CSS must come from its HTML entries`);
      }
      for (const f of Object.values(bundle)) {
        if (f.type !== "asset" || !f.fileName.endsWith(".html")) continue;
        let html = String(f.source);
        for (const css of sheets) {
          const text = String(css.source);
          if (/<\/style/i.test(text)) throw new Error(`${css.fileName} contains "</style" and cannot be inlined`);
          html = html.replace(new RegExp(`<link rel="stylesheet"[^>]*href="/${escape(css.fileName)}"[^>]*>`), () => `<style>${text}</style>`);
        }
        if (html.includes(`<link rel="stylesheet"`)) throw new Error(`${f.fileName} still links a stylesheet`);
        f.source = html;
      }
      // No chunk may name a sheet deleted below (a dynamic import's preload list would).
      for (const c of chunks.values()) for (const css of sheets) if (c.code.includes(css.fileName)) throw new Error(`${c.fileName} names ${css.fileName}, which is inlined and deleted`);
      for (const css of sheets) delete bundle[css.fileName];
      // So the manifest names no deleted file.
      for (const c of chunks.values()) c.viteMetadata?.importedCss.clear();
    },
  };
}

export default defineConfig({
  root: "shell", base: "/", plugins: [svelte({ configFile: SVELTE_CONFIG }), inlineCss()],
  build: {
    outDir: "../dist", emptyOutDir: false, assetsDir: "_clax/shell", manifest: true,
    rollupOptions: { input: { index: "shell/index.html", artifact: "shell/artifact.html" } },
  },
  server: { proxy: { "/api": "http://127.0.0.1:7481", "/c": "http://127.0.0.1:7481", "/_blob": "http://127.0.0.1:7481", "/healthz": "http://127.0.0.1:7481" } },
});
