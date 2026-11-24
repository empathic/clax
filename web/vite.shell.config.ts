import { fileURLToPath } from "node:url";
import { defineConfig, type Plugin } from "vite";
import type { OutputAsset } from "rollup";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { appIcons } from "./scripts/extension-icons.mjs";
// The root is shell/, so the plugin would not find svelte.config.js (runes mode) by itself.
const SVELTE_CONFIG = fileURLToPath(new URL("svelte.config.js", import.meta.url));

const escape = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/** Replaces each entry's stylesheet link with the stylesheet itself, so no
 * request blocks the first render, and drops the separate CSS files. */
export function inlineCss(): Plugin {
  return {
    name: "clax-inline-css",
    apply: "build",
    enforce: "post",
    generateBundle(_options, bundle) {
      const sheets = Object.values(bundle).filter((f): f is OutputAsset => f.type === "asset" && f.fileName.endsWith(".css"));
      // An entry's HTML links the CSS of the chunks it imports statically, so
      // that CSS is inlined below. A chunk an entry loads only lazily would
      // have its CSS fetched by Vite's preload helper from a file this plugin
      // deletes, or not at all: refuse it. The check is per entry, since a
      // chunk one entry imports statically has its CSS only in that entry's
      // HTML. A lazy chunk may import a chunk the same entry already loads
      // statically: that chunk's CSS is in the entry's HTML, so it passes.
      const chunks = new Map(Object.values(bundle).flatMap(c => (c.type === "chunk" ? [[c.fileName, c] as const] : [])));
      const closure = (from: string[], into: Set<string>) => {
        const walk = (name: string) => {
          if (into.has(name)) return;
          into.add(name);
          for (const i of chunks.get(name)?.imports ?? []) walk(i);
        };
        for (const f of from) walk(f);
        return into;
      };
      for (const entry of [...chunks.values()].filter(c => c.isEntry)) {
        const eager = closure([entry.fileName], new Set());
        // Every chunk a dynamic import reaches, with what it imports, at any depth.
        const lazy = new Set<string>();
        const queue = [...eager].flatMap(n => chunks.get(n)?.dynamicImports ?? []);
        for (let n = queue.pop(); n !== undefined; n = queue.pop()) {
          if (lazy.has(n)) continue;
          for (const m of closure([n], new Set())) {
            if (lazy.has(m)) continue;
            lazy.add(m);
            queue.push(...(chunks.get(m)?.dynamicImports ?? []));
          }
        }
        for (const name of lazy) {
          if (!eager.has(name) && chunks.get(name)?.viteMetadata?.importedCss.size) throw new Error(`${name} is loaded lazily by ${entry.fileName} and imports CSS that ${entry.fileName}'s HTML does not carry; the shell's CSS must come from its HTML entries`);
        }
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
      for (const css of sheets) delete bundle[css.fileName];
      // So the manifest names no deleted file.
      for (const c of chunks.values()) c.viteMetadata?.importedCss.clear();
    },
  };
}

/** Emits the installed web app's PNG icons (scripts/extension-icons.mjs)
 * under `_clax/app/`, where the manifest and the entries' heads name them. */
export function appIconFiles(): Plugin {
  return {
    name: "clax-app-icons",
    apply: "build",
    generateBundle() {
      for (const [name, source] of Object.entries(appIcons())) this.emitFile({ type: "asset", fileName: `_clax/app/${name}`, source });
    },
  };
}

/** The gallery entry's lazy chunks its inline `clax-page` script preloads
 * (`<link rel="modulepreload">`), by the module each chunk is: `gallery`
 * on every page of the entry but the sign-in pages, `q` and `summary` on
 * the gallery for a browser with an inbox, and each page's own chunk on
 * that page (`inbox` with `q`). */
export const PRELOAD: Record<string, string> = {
  gallery: "/src/ui/gallery-working.ts",
  q: "/src/q/index.ts",
  summary: "/src/q/GallerySummary.svelte",
  inbox: "/src/q/InboxPage.svelte",
  tags: "/src/ui/TagsPage.svelte",
  queues: "/src/queues/Queues.svelte",
  settings: "/src/settings/Settings.svelte",
};

/** Writes each `PRELOAD` chunk's file, with the files it imports that the
 * entry (and, for any but `gallery`, the gallery's lazy module) has not
 * loaded already, into `index.html`'s `/*clax:preload*\/{}`, so the lazy
 * chunks every load needs download beside the entry instead of after it. */
export function preloadLazy(): Plugin {
  return {
    name: "clax-preload-lazy",
    apply: "build",
    enforce: "post",
    generateBundle(_options, bundle) {
      const html = bundle["index.html"];
      if (!html || html.type !== "asset") return;
      const chunks = Object.values(bundle).flatMap(c => (c.type === "chunk" ? [c] : []));
      const byName = new Map(chunks.map(c => [c.fileName, c]));
      const closure = (from: string, into = new Set<string>()) => {
        if (into.has(from)) return into;
        into.add(from);
        for (const i of byName.get(from)?.imports ?? []) closure(i, into);
        return into;
      };
      const entry = chunks.find(c => c.isEntry && c.facadeModuleId?.endsWith("/shell/index.html"));
      if (!entry) throw new Error("no chunk for shell/index.html");
      const loaded = closure(entry.fileName);
      const fileOf = (id: string) => {
        const c = chunks.find(x => x.facadeModuleId?.endsWith(id)) ?? chunks.find(x => x.isDynamicEntry && x.moduleIds.some(m => m.endsWith(id)));
        if (!c) throw new Error(`${id}, which index.html preloads, is in no chunk`);
        return c.fileName;
      };
      const gallery = closure(fileOf(PRELOAD.gallery));
      const map: Record<string, string[]> = {};
      for (const [key, id] of Object.entries(PRELOAD)) {
        const files = key === "gallery" ? gallery : closure(fileOf(id));
        map[key] = [...files].filter(f => !loaded.has(f) && (key === "gallery" || !gallery.has(f))).map(f => `/${f}`);
      }
      const text = String(html.source);
      const MARK = "/*clax:preload*/{}";
      if (!text.includes(MARK)) throw new Error(`index.html lost ${MARK}`);
      html.source = text.replace(MARK, () => JSON.stringify(map));
    },
  };
}

// CLAX_TEST_CLOCK=1 builds the browser tests' shell, whose clock a test can
// advance (shell/src/clock.ts); every other build leaves the hook out.
export const TEST_CLOCK = process.env.CLAX_TEST_CLOCK === "1";

export default defineConfig({
  root: "shell", base: "/", plugins: [svelte({ configFile: SVELTE_CONFIG }), inlineCss(), appIconFiles(), preloadLazy()],
  define: { __CLAX_TEST_CLOCK__: JSON.stringify(TEST_CLOCK) },
  build: {
    outDir: "../dist", emptyOutDir: false, assetsDir: "_clax/shell", manifest: true,
    rollupOptions: { input: { index: "shell/index.html", artifact: "shell/artifact.html" } },
  },
  server: { proxy: { "/api": "http://127.0.0.1:7481", "/c": "http://127.0.0.1:7481", "/_blob": "http://127.0.0.1:7481", "/healthz": "http://127.0.0.1:7481" } },
});
