import { spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createRequire } from "node:module";
import { afterEach, describe, expect, it } from "vitest";
import { CHILD_TIMEOUT_MS } from "../shell/src/test/timeouts";

/** The script's deflate (pako has no type declarations). */
const pako = createRequire(import.meta.url)("pako") as { gzip(data: string, o: { level: number }): Uint8Array };

let root = "";
afterEach(() => { if (root) rmSync(root, { recursive: true, force: true }); root = ""; });

/** A scratch web/ with a copy of the script, a minimal build and `budget` as the budget file. */
const ENTRY = "<head><script id=\"clax-early\"></script><!--clax:boot--></head><body><!--clax:frame--><h1>Clax</h1></body>";

/** The extension's release manifest as the build writes it. */
const EXT_MANIFEST = { manifest_version: 3, name: "Clax", version: "1.0.0", optional_host_permissions: ["http://*/*", "https://*/*"] };

/** `fonts` are font files put in dist (none by default); `index` is the gallery entry; `extFiles` are extra files put in dist-extension; `extBodies` are files written there with the given content. */
function run(budget: Record<string, unknown>, artifact = ENTRY, args: string[] = [], { fonts = [] as string[], index = "<p>gallery</p>", extManifest = EXT_MANIFEST as Record<string, unknown>, extFiles = [] as string[], extBodies = {} as Record<string, string> } = {}) {
  root = mkdtempSync(join(tmpdir(), "clax-bundle-size-"));
  const web = join(root, "web");
  mkdirSync(join(web, "scripts"), { recursive: true });
  mkdirSync(join(web, "perf"));
  mkdirSync(join(web, "dist/.vite"), { recursive: true });
  mkdirSync(join(web, "dist/_clax/shell"), { recursive: true });
  mkdirSync(join(web, "dist/_clax/bridge/.vite"), { recursive: true });
  copyFileSync(join(__dirname, "bundle-size.mjs"), join(web, "scripts/bundle-size.mjs"));
  // The script imports its deflate (pako) from web/node_modules.
  symlinkSync(join(__dirname, "../node_modules"), join(web, "node_modules"), "dir");
  const dist = (p: string, s: string) => writeFileSync(join(web, "dist", p), s);
  dist("index.html", index);
  for (const f of fonts) { mkdirSync(join(web, "dist", f, ".."), { recursive: true }); dist(f, "wOF2"); }
  dist("artifact.html", artifact);
  dist("_clax/bridge.js", "bridge");
  dist("_clax/transport.js", "transport");
  dist("_clax/shell/a.js", "a");
  // The question module imports the gallery's entry chunk, the gallery's
  // lazy module's own import (which the artifact view has not loaded), and
  // one file of its own.
  for (const f of ["w.js", "as.js", "s.js", "q.js", "qd.js", "h.js", "hd.js"]) dist(`_clax/shell/${f}`, f.repeat(100));
  // The People and Queues pages import the gallery's lazy module's import too, and one file of their own.
  for (const f of ["p.js", "pd.js", "u.js", "ud.js"]) dist(`_clax/shell/${f}`, f.repeat(100));
  dist(".vite/manifest.json", JSON.stringify({
    "index.html": { file: "_clax/shell/a.js" }, "artifact.html": { file: "_clax/shell/a.js", dynamicImports: ["src/app/host.ts"] },
    "src/ui/gallery-working.ts": { file: "_clax/shell/w.js", isDynamicEntry: true, imports: ["index.html", "_s.js"], dynamicImports: ["_index-q.js", "src/people/People.svelte", "src/queues/Queues.svelte"] },
    "src/people/People.svelte": { file: "_clax/shell/p.js", isDynamicEntry: true, imports: ["index.html", "_s.js", "_pd.js"] },
    "_pd.js": { file: "_clax/shell/pd.js" },
    "src/queues/Queues.svelte": { file: "_clax/shell/u.js", isDynamicEntry: true, imports: ["index.html", "_s.js", "_ud.js"] },
    "_ud.js": { file: "_clax/shell/ud.js" },
    "src/view/artifact-stream.ts": { file: "_clax/shell/as.js", isDynamicEntry: true, imports: ["artifact.html"], dynamicImports: ["_index-q.js"] },
    "src/ui/MoreMenu.svelte": { file: "_clax/shell/as.js", isDynamicEntry: true },
    "src/ui/Who.svelte": { file: "_clax/shell/as.js", isDynamicEntry: true },
    "_s.js": { file: "_clax/shell/s.js" },
    "_index-q.js": { file: "_clax/shell/q.js", name: "index", isDynamicEntry: true, imports: ["index.html", "_s.js", "_qd.js"] },
    "_qd.js": { file: "_clax/shell/qd.js" },
    // App mode's host: the artifact entry's dynamic import, with one import
    // the entry has not loaded.
    "src/app/host.ts": { file: "_clax/shell/h.js", name: "host", isDynamicEntry: true, imports: ["artifact.html", "_hd.js"] },
    "_hd.js": { file: "_clax/shell/hd.js" },
  }));
  // The parts: clip and caps import the comment part's file.
  for (const f of ["comment-1.js", "clip-1.js", "caps-1.js", "origin-1.js", "room-1.js", "sample-1.js", "live-1.js"]) dist(`_clax/bridge/${f}`, f.repeat(50));
  dist("_clax/bridge/.vite/manifest.json", JSON.stringify({
    "c.ts": { file: "comment-1.js", name: "comment", isEntry: true },
    "k.ts": { file: "clip-1.js", name: "clip", isEntry: true, imports: ["c.ts"] },
    "p.ts": { file: "caps-1.js", name: "caps", isEntry: true, imports: ["c.ts"] },
    "o.ts": { file: "origin-1.js", name: "origin", isEntry: true },
    "r.ts": { file: "room-1.js", name: "room", isEntry: true },
    "s.ts": { file: "sample-1.js", name: "sample", isEntry: true },
    "l.ts": { file: "live-1.js", name: "live", isEntry: true },
  }));
  // The extension: the side panel names its script and stylesheet; the composer names one script.
  mkdirSync(join(web, "dist-extension/assets"), { recursive: true });
  const ext = (p: string, s: string) => { mkdirSync(join(web, "dist-extension", p, ".."), { recursive: true }); writeFileSync(join(web, "dist-extension", p), s); };
  ext("manifest.json", JSON.stringify(extManifest));
  for (const f of ["sw.js", "overlay.js"]) ext(f, f.repeat(20));
  ext("assets/panel.js", "panel-script ".repeat(400));
  ext("assets/panel.css", "panel-style ".repeat(300));
  ext("assets/composer.js", "composer");
  ext("sidepanel.html", `<script type="module" src="./assets/panel.js"></script><link rel="stylesheet" href="./assets/panel.css">`);
  ext("composer.html", `<script type="module" src="./assets/composer.js"></script>`);
  for (const f of extFiles) ext(f, "x");
  for (const [f, body] of Object.entries(extBodies)) ext(f, body);
  // The SDK package: its overlay entry imports the main entry, which imports a
  // chunk; the service entry is no part of the overlay's size.
  const sdk = (p: string, s: string) => { mkdirSync(join(web, "dist-sdk", p, ".."), { recursive: true }); writeFileSync(join(web, "dist-sdk", p), s); };
  sdk("overlay.js", `import { R } from "./index.js";\n${"overlay ".repeat(200)}`);
  sdk("index.js", `import { C } from "./chunks/client.js";\nexport { R };\n${"index ".repeat(100)}`);
  sdk("chunks/client.js", `export { C };\n${"client ".repeat(100)}`);
  sdk("service.js", `import { C } from "./chunks/client.js";\n${"service ".repeat(100)}`);
  writeFileSync(join(web, "perf/bundle-budget.json"), JSON.stringify(budget));
  return spawnSync(process.execPath, [join(web, "scripts/bundle-size.mjs"), ...args], { encoding: "utf8" });
}

describe("bundle-size.mjs", { timeout: CHILD_TIMEOUT_MS }, () => {
  const full = { gallery: 10_000, artifact: 10_000, bridge: 10_000, transport: 10_000, bridgeBaseline: 10_000, partComment: 10_000, partClip: 10_000, partCaps: 10_000, partOrigin: 10_000, partRoom: 10_000, partSample: 10_000, partLive: 10_000, extOverlay: 10_000, extWorker: 10_000, extComposer: 10_000, extPanel: 10_000, questions: 10_000, people: 10_000, queues: 10_000, host: 10_000, sdkOverlay: 10_000 };

  it("passes within budget", () => {
    expect(run(full).status).toBe(0);
  });

  it("fails when the artifact entry lost a marker, or runs the early script after the page", () => {
    for (const html of [
      ENTRY.replace("<!--clax:frame-->", ""),
      ENTRY.replace("<h1>Clax</h1>", ""),
      ENTRY.replace("<script id=\"clax-early\"></script>", ""),
      ENTRY.replace("<script id=\"clax-early\"></script>", "").replace("</body>", "<script id=\"clax-early\"></script></body>"),
      ENTRY.replace("<script id=\"clax-early\"></script><!--clax:boot-->", "<!--clax:boot--><script id=\"clax-early\"></script>"),
    ]) {
      const r = run(full, html);
      expect(r.status, html).toBe(1);
      expect(r.stderr).toContain("artifact.html");
      rmSync(root, { recursive: true, force: true });
    }
  });

  it("fails when a size is over its budget", () => {
    const r = run({ ...full, gallery: 1 });
    expect(r.status).toBe(1);
    expect(r.stderr).toContain("gallery");
  });

  it("counts a part with the files it imports, against the part's own budget", () => {
    const sizes = (r: ReturnType<typeof run>) => Object.fromEntries([...r.stdout.matchAll(/(comment|clip|caps) (\d+)/g)].map(m => [m[1], Number(m[2])]));
    const s = sizes(run(full));
    expect(s.clip).toBeGreaterThan(s.comment);
    rmSync(root, { recursive: true, force: true });
    const r = run({ ...full, partClip: s.clip - 1 });
    expect(r.status).toBe(1);
    expect(r.stderr).toContain("partClip");
  });

  it("counts app mode's host chunk with what it imports beyond the artifact entry, against its own budget", () => {
    const r0 = run(full);
    const host = Number(/\bhost (\d+)/.exec(r0.stdout)?.[1]);
    const one = (f: string) => pako.gzip(f.repeat(100), { level: 9 }).length;
    expect(host).toBe(one("h.js") + one("hd.js"));
    rmSync(root, { recursive: true, force: true });
    const r = run({ ...full, host: host - 1 });
    expect(r.status).toBe(1);
    expect(r.stderr).toContain("host");
    rmSync(root, { recursive: true, force: true });
    const { host: _, ...noHost } = full;
    const m = run(noHost);
    expect(m.status).toBe(1);
    expect(m.stderr).toContain("lacks a numeric budget for: host");
  });

  it("fails when the build carries a font file, wherever it is", () => {
    for (const f of ["_clax/fonts/a.woff2", "_clax/shell/b.woff", "c.ttf", "_clax/bridge/d.otf"]) {
      const r = run(full, ENTRY, [], { fonts: [f] });
      expect(r.status, f).toBe(1);
      expect(r.stderr).toContain("is a font file");
      rmSync(root, { recursive: true, force: true });
    }
  });

  it("fails when an entry preloads a font", () => {
    const r = run(full, ENTRY, [], { index: `<link rel="preload" href="/_clax/fonts/x.woff2" as="font"><p>gallery</p>` });
    expect(r.status).toBe(1);
    expect(r.stderr).toContain("preloads a font");
  });

  it("finds a font preload whatever the attribute order and quoting, and lets a module preload through", () => {
    for (const link of [
      `<link href="/_clax/fonts/x.woff2" rel="preload" as="font">`,
      `<link rel='preload' href='/_clax/fonts/x.woff2' as='font'>`,
      `<link as=font href=/_clax/fonts/x.woff2 rel=preload crossorigin>`,
      `<link rel="preload" href="/f" as="font" type="font/woff2">`,
    ]) {
      const r = run(full, ENTRY, [], { index: `${link}<p>gallery</p>` });
      expect(r.status, link).toBe(1);
      expect(r.stderr).toContain("preloads a font");
      rmSync(root, { recursive: true, force: true });
    }
    expect(run(full, ENTRY, [], { index: `<link rel="modulepreload" href="/_clax/shell/a.js"><link rel="icon" href="/_clax/mark.svg"><p>gallery</p>` }).status).toBe(0);
  });

  it("fails when an @font-face fetches a font, swap or not, and passes a local-only face", () => {
    const face = (body: string) => ENTRY.replace("</head>", `<style>@font-face { ${body} }</style></head>`);
    for (const body of [`font-family: P; src: url("/_clax/fonts/a.woff2");`, `font-family: P; font-display: swap; src: url("/_clax/fonts/a.woff2");`]) {
      const r = run(full, face(body));
      expect(r.status, body).toBe(1);
      expect(r.stderr).toContain("fetches a font");
      rmSync(root, { recursive: true, force: true });
    }
    expect(run(full, face(`font-family: F; src: local("Menlo");`)).status).toBe(0);
  });

  it("counts the app's transport script against its own budget", () => {
    const t = Number(/\btransport (\d+)/.exec(run(full).stdout)?.[1]);
    expect(t).toBe(pako.gzip("transport", { level: 9 }).length);
    rmSync(root, { recursive: true, force: true });
    const r = run({ ...full, transport: t - 1 });
    expect(r.status).toBe(1);
    expect(r.stderr).toContain("transport");
  });

  it("adds a missing part budget when recording, never a missing baseline", () => {
    const { partCaps: _, ...noCaps } = full;
    expect(run(noCaps).status).toBe(1);
    rmSync(root, { recursive: true, force: true });
    expect(run(noCaps, ENTRY, ["--record"]).status).toBe(0);
    rmSync(root, { recursive: true, force: true });
    const { bridgeBaseline: __, ...noBaseline } = full;
    expect(run(noBaseline, ENTRY, ["--record"]).status).toBe(1);
  });

  it("counts each extension page with the scripts and stylesheets it names, against its own budget", () => {
    const sizes = (r: ReturnType<typeof run>) => Object.fromEntries([...r.stdout.matchAll(/(overlay|worker|composer|panel) (\d+)/g)].map(m => [m[1], Number(m[2])]));
    const s = sizes(run(full));
    expect(s.panel).toBeGreaterThan(s.composer);
    for (const k of ["overlay", "worker", "composer", "panel"]) expect(s[k], k).toBeGreaterThan(0);
    for (const [key, size] of [["extPanel", s.panel], ["extComposer", s.composer], ["extOverlay", s.overlay], ["extWorker", s.worker]] as const) {
      rmSync(root, { recursive: true, force: true });
      const r = run({ ...full, [key]: size - 1 });
      expect(r.status, key).toBe(1);
      expect(r.stderr).toContain(key);
    }
  });

  it("fails when the extension's release manifest grants hosts or declares content scripts", () => {
    for (const extra of [{ host_permissions: ["<all_urls>"] }, { content_scripts: [{ matches: ["<all_urls>"], js: ["loader.js"] }] }]) {
      const r = run(full, ENTRY, [], { extManifest: { ...EXT_MANIFEST, ...extra } });
      expect(r.status, JSON.stringify(extra)).toBe(1);
      expect(r.stderr).toContain(Object.keys(extra)[0]);
      rmSync(root, { recursive: true, force: true });
    }
  });

  it("fails when the extension's build carries a key file", () => {
    for (const f of ["key/key.pub.b64", "key.pub.b64", "key/.gitkeep"]) {
      const r = run(full, ENTRY, [], { extFiles: [f] });
      expect(r.status, f).toBe(1);
      expect(r.stderr).toContain("is a key file");
      rmSync(root, { recursive: true, force: true });
    }
    expect(run(full, ENTRY, [], { extFiles: ["assets/keyboard.js", "monkey.js"] }).status).toBe(0);
  });

  it("fails when a file of the extension's release build carries the test hook", () => {
    for (const [f, body] of [["sw.js", "globalThis.claxTest={comment}"], ["assets/panel-2.js", "if(__CLAX_EXT_TEST__)x()"], ["overlay.js", "self.claxTest=1"]]) {
      const r = run(full, ENTRY, [], { extFiles: [], extBodies: { [f]: body } });
      expect(r.status, f).toBe(1);
      expect(r.stderr).toContain("test hook");
      rmSync(root, { recursive: true, force: true });
    }
  });

  it("counts the question module with what it imports beyond its larger host's loaded chunks", () => {
    const size = (r: ReturnType<typeof run>) => Number(/questions (\d+)/.exec(r.stdout)?.[1]);
    const q = size(run(full));
    const one = (f: string) => pako.gzip(f.repeat(100), { level: 9 }).length;
    // The artifact view has not loaded s.js; the gallery has.
    expect(q).toBe(one("q.js") + one("qd.js") + one("s.js"));
    rmSync(root, { recursive: true, force: true });
    const r = run({ ...full, questions: q - 1 });
    expect(r.status).toBe(1);
    expect(r.stderr).toContain("questions");
  });

  it("counts the People page with what it imports beyond the gallery and its lazy module", () => {
    const size = (r: ReturnType<typeof run>) => Number(/people (\d+)/.exec(r.stdout)?.[1]);
    const p = size(run(full));
    const one = (f: string) => pako.gzip(f.repeat(100), { level: 9 }).length;
    expect(p).toBe(one("p.js") + one("pd.js"));
    rmSync(root, { recursive: true, force: true });
    const r = run({ ...full, people: p - 1 });
    expect(r.status).toBe(1);
    expect(r.stderr).toContain("people");
  });

  it("counts the Queues page with what it imports beyond the gallery and its lazy module", () => {
    const size = (r: ReturnType<typeof run>) => Number(/queues (\d+)/.exec(r.stdout)?.[1]);
    const u = size(run(full));
    const one = (f: string) => pako.gzip(f.repeat(100), { level: 9 }).length;
    expect(u).toBe(one("u.js") + one("ud.js"));
    rmSync(root, { recursive: true, force: true });
    const r = run({ ...full, queues: u - 1 });
    expect(r.status).toBe(1);
    expect(r.stderr).toContain("queues");
  });

  it("counts the SDK package's overlay entry with the files it imports, against its own budget", () => {
    const r0 = run(full);
    const size = Number(/\bsdk package: overlay entry (\d+)/.exec(r0.stdout)?.[1]);
    const file = (f: string) => pako.gzip(readFileSync(join(root, "web/dist-sdk", f), "utf8"), { level: 9 }).length;
    expect(size).toBe(file("overlay.js") + file("index.js") + file("chunks/client.js"));
    rmSync(root, { recursive: true, force: true });
    const r = run({ ...full, sdkOverlay: size - 1 });
    expect(r.status).toBe(1);
    expect(r.stderr).toContain("sdkOverlay");
  });

  it("never records the extension's ceilings or the question module's, nor adds a missing one", () => {
    expect(run(full, ENTRY, ["--record"]).status).toBe(0);
    const recorded = JSON.parse(readFileSync(join(root, "web/perf/bundle-budget.json"), "utf8"));
    for (const k of ["extOverlay", "extWorker", "extComposer", "extPanel", "questions", "people", "queues", "sdkOverlay"]) expect(recorded[k], k).toBe(10_000);
    expect(recorded.gallery).toBeLessThan(10_000);
    rmSync(root, { recursive: true, force: true });
    const { extPanel: _, ...noPanel } = full;
    const r = run(noPanel, ENTRY, ["--record"]);
    expect(r.status).toBe(1);
    expect(r.stderr).toContain("extPanel");
  });

  it.each(["gallery", "artifact", "bridge", "transport", "bridgeBaseline", "partComment", "partClip", "partCaps", "partOrigin", "partRoom", "partSample", "partLive", "extOverlay", "extWorker", "extComposer", "extPanel", "sdkOverlay"])("fails when the %s budget is missing or not a number", k => {
    for (const budget of [Object.fromEntries(Object.entries(full).filter(([key]) => key !== k)), { ...full, [k]: "10000" }]) {
      const r = run(budget);
      expect(r.status).toBe(1);
      expect(r.stderr).toContain(`lacks a numeric budget for: ${k}`);
      rmSync(root, { recursive: true, force: true });
    }
  });
});
