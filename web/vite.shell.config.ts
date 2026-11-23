import { fileURLToPath } from "node:url";
import { defineConfig, type Plugin } from "vite";
import type { OutputAsset } from "rollup";
import { svelte } from "@sveltejs/vite-plugin-svelte";
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
          if (!eager.has(name) && chunks.get(name)?.viteMetadata?.importedCss.size) throw new Error(`${name} (imported by ${[...chunks.values()].filter(c => c.imports.includes(name) || c.dynamicImports.includes(name)).map(c => `${c.fileName} [${c.moduleIds.filter(m => m.includes("/shell/src/")).slice(0, 4).map(m => m.replace(/^.*\/shell\//, "")).join(" ")}]`).join(", ")}) is loaded lazily by ${entry.fileName} and imports CSS that ${entry.fileName}'s HTML does not carry; the shell's CSS must come from its HTML entries`);
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

/** The gallery entry's lazy chunks its inline `clax-page` script preloads
 * (`<link rel="modulepreload">`), by the modules each is: `gallery`
 * on every page of the entry but the sign-in pages, `q` and `summary` on
 * the gallery for a browser with an inbox, each page's own chunk on
 * that page (`inbox` with `q`), and `legal` (the Terms prompt) where the
 * bootstrap block says the person has not agreed to them. The sign-in
 * pages preload `signin` and the landing page `landing`, alone
 * (`ALONE`: without the gallery's). */
export const PRELOAD: Record<string, string | string[]> = {
  gallery: "/src/ui/gallery-working.ts",
  q: "/src/q/index.ts",
  summary: "/src/q/GallerySummary.svelte",
  inbox: "/src/q/InboxPage.svelte",
  tags: "/src/ui/TagsPage.svelte",
  queues: "/src/queues/Queues.svelte",
  settings: "/src/settings/Settings.svelte",
  legal: ["/src/hosted/legal.ts", "/src/hosted/LegalPrompt.svelte"],
  signin: "/src/hosted/SignIn.svelte",
  // The landing page reads its block with the sign-in pages' reader (`gallery-main.ts`).
  landing: ["/src/hosted/Landing.svelte", "/src/hosted/signin-boot.ts"],
};
/** The `PRELOAD` keys a page preloads without the gallery's lazy module. */
const ALONE = new Set(["signin", "landing"]);

/** The artifact entry's lazy chunks its inline `clax-preload` script
 * preloads, by the modules each is: the top bar's controls' code (`bar`,
 * `ui/more-menu.svelte.ts` asks it as the entry starts) on every load but
 * the app's inspect view; Publish's (`publish`) where the browser holds the
 * local owner's proof; the thread list's (`sidebar`) where the threads open
 * docked (900 px and wider), with the haiku list it fetches (`fetch`, a
 * `preload` the build adds); and the question module (`q`, with the view's
 * loader of it) for a browser with an inbox: the bootstrap block's
 * `deciders.inbox`, else the owner's proof; and ClaxBot's part (`bot`)
 * where the block says ClaxBot is on for this browser or the artifact has
 * runs. */
export const ARTIFACT_PRELOAD: Record<string, string | string[]> = {
  bar: ["/src/ui/MoreMenu.svelte", "/src/ui/Who.svelte", "/src/ui/Clone.svelte", "/src/ui/ForkButton.svelte", "/src/ui/GetInClax.svelte", "/src/ui/Share.svelte", "/src/ui/PermissionsPanel.svelte"],
  publish: "/src/ui/Publish.svelte",
  sidebar: "/src/ui/ShellSidebar.svelte",
  q: ["/src/q/index.ts", "/src/view/artifact-questions.ts"],
  bot: "/src/bot/ArtifactBot.svelte",
};

/** Writes each preloaded chunk's file, with the files it imports that the
 * entry has not loaded already, into its HTML's `/*clax:preload*\/{}`: the
 * gallery entry's `PRELOAD` into `index.html` (for any key but `gallery`
 * and the `ALONE` ones, less what the gallery's lazy module loads), the
 * artifact entry's `ARTIFACT_PRELOAD` into `artifact.html`, so the lazy
 * chunks a load needs download beside the entry instead of after it. */
export function preloadLazy(): Plugin {
  return {
    name: "clax-preload-lazy",
    apply: "build",
    enforce: "post",
    generateBundle(_options, bundle) {
      const chunks = Object.values(bundle).flatMap(c => (c.type === "chunk" ? [c] : []));
      const byName = new Map(chunks.map(c => [c.fileName, c]));
      const closure = (from: string, into = new Set<string>()) => {
        if (into.has(from)) return into;
        into.add(from);
        for (const i of byName.get(from)?.imports ?? []) closure(i, into);
        return into;
      };
      const fileOf = (id: string, html: string) => {
        const c = chunks.find(x => x.facadeModuleId?.endsWith(id)) ?? chunks.find(x => x.isDynamicEntry && x.moduleIds.some(m => m.endsWith(id)));
        if (!c) throw new Error(`${id}, which ${html} preloads, is in no chunk`);
        return c.fileName;
      };
      const write = (html: string, map: Record<string, string[]>) => {
        const asset = bundle[html];
        if (!asset || asset.type !== "asset") return;
        const text = String(asset.source);
        const MARK = "/*clax:preload*/{}";
        if (!text.includes(MARK)) throw new Error(`${html} lost ${MARK}`);
        asset.source = text.replace(MARK, () => JSON.stringify(map));
      };
      const entryOf = (html: string) => {
        const e = chunks.find(c => c.isEntry && c.facadeModuleId?.endsWith(`/shell/${html}`));
        if (!e) throw new Error(`no chunk for shell/${html}`);
        return closure(e.fileName);
      };
      if (bundle["index.html"]) {
        const loaded = entryOf("index.html");
        const gallery = closure(fileOf(PRELOAD.gallery as string, "index.html"));
        const map: Record<string, string[]> = {};
        for (const [key, ids] of Object.entries(PRELOAD)) {
          const files = new Set<string>();
          for (const id of [ids].flat()) closure(fileOf(id, "index.html"), files);
          map[key] = [...files].filter(f => !loaded.has(f) && (key === "gallery" || ALONE.has(key) || !gallery.has(f))).map(f => `/${f}`);
        }
        write("index.html", map);
      }
      if (bundle["artifact.html"]) {
        const loaded = entryOf("artifact.html");
        const map: Record<string, string[]> = {};
        for (const [key, ids] of Object.entries(ARTIFACT_PRELOAD)) {
          const files = new Set<string>();
          for (const id of [ids].flat()) closure(fileOf(id, "artifact.html"), files);
          map[key] = [...files].filter(f => !loaded.has(f)).map(f => `/${f}`);
        }
        // The haiku list the thread list's chunk fetches with it (`view/haiku-list.ts`), preloaded with it.
        const haiku = Object.values(bundle).find(f => f.type === "asset" && /\/haiku-[\w-]+\.json$/.test(`/${f.fileName}`));
        if (!haiku) throw new Error("no haiku list in the build, which artifact.html preloads");
        map.fetch = [`/${haiku.fileName}`];
        write("artifact.html", map);
      }
    },
  };
}

// CLAX_TEST_CLOCK=1 builds the browser tests' shell, whose clock a test can
// advance (shell/src/clock.ts); every other build leaves the hook out.
export const TEST_CLOCK = process.env.CLAX_TEST_CLOCK === "1";

export default defineConfig({
  root: "shell", base: "/", plugins: [svelte({ configFile: SVELTE_CONFIG }), inlineCss(), preloadLazy()],
  define: { __CLAX_TEST_CLOCK__: JSON.stringify(TEST_CLOCK) },
  build: {
    outDir: "../dist", emptyOutDir: false, assetsDir: "_clax/shell", manifest: true,
    rollupOptions: { input: { index: "shell/index.html", artifact: "shell/artifact.html" } },
  },
  server: { proxy: { "/api": "http://127.0.0.1:7481", "/c": "http://127.0.0.1:7481", "/_blob": "http://127.0.0.1:7481", "/healthz": "http://127.0.0.1:7481" } },
});
