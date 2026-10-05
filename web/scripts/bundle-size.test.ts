import { spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

let root = "";
afterEach(() => { if (root) rmSync(root, { recursive: true, force: true }); root = ""; });

/** A scratch web/ with a copy of the script, a minimal build and `budget` as the budget file. */
const ENTRY = "<head><script id=\"clax-early\"></script><!--clax:boot--></head><body><!--clax:frame--><h1>Clax</h1></body>";

/** The extension's release manifest as the build writes it. */
const EXT_MANIFEST = { manifest_version: 3, name: "Clax", version: "1.0.0", optional_host_permissions: ["http://*/*", "https://*/*"] };

/** `fonts` are font files put in dist (none by default); `index` is the gallery entry; `extFiles` are extra files put in dist-extension. */
function run(budget: Record<string, unknown>, artifact = ENTRY, args: string[] = [], { fonts = [] as string[], index = "<p>gallery</p>", extManifest = EXT_MANIFEST as Record<string, unknown>, extFiles = [] as string[] } = {}) {
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
  dist("_clax/shell/a.js", "a");
  dist(".vite/manifest.json", JSON.stringify({ "index.html": { file: "_clax/shell/a.js" }, "artifact.html": { file: "_clax/shell/a.js" } }));
  // The parts: clip and caps import the comment part's file.
  for (const f of ["comment-1.js", "clip-1.js", "caps-1.js", "room-1.js", "sample-1.js"]) dist(`_clax/bridge/${f}`, f.repeat(50));
  dist("_clax/bridge/.vite/manifest.json", JSON.stringify({
    "c.ts": { file: "comment-1.js", name: "comment", isEntry: true },
    "k.ts": { file: "clip-1.js", name: "clip", isEntry: true, imports: ["c.ts"] },
    "p.ts": { file: "caps-1.js", name: "caps", isEntry: true, imports: ["c.ts"] },
    "r.ts": { file: "room-1.js", name: "room", isEntry: true },
    "s.ts": { file: "sample-1.js", name: "sample", isEntry: true },
  }));
  // The extension: the side panel names its script and stylesheet; the composer names one script.
  mkdirSync(join(web, "dist-extension/assets"), { recursive: true });
  const ext = (p: string, s: string) => { mkdirSync(join(web, "dist-extension", p, ".."), { recursive: true }); writeFileSync(join(web, "dist-extension", p), s); };
  ext("manifest.json", JSON.stringify(extManifest));
  for (const f of ["sw.js", "loader.js", "overlay.js"]) ext(f, f.repeat(20));
  ext("assets/panel.js", "panel-script ".repeat(400));
  ext("assets/panel.css", "panel-style ".repeat(300));
  ext("assets/composer.js", "composer");
  ext("sidepanel.html", `<script type="module" src="./assets/panel.js"></script><link rel="stylesheet" href="./assets/panel.css">`);
  ext("composer.html", `<script type="module" src="./assets/composer.js"></script>`);
  for (const f of extFiles) ext(f, "x");
  writeFileSync(join(web, "perf/bundle-budget.json"), JSON.stringify(budget));
  return spawnSync(process.execPath, [join(web, "scripts/bundle-size.mjs"), ...args], { encoding: "utf8" });
}

describe("bundle-size.mjs", () => {
  const full = { gallery: 10_000, artifact: 10_000, bridge: 10_000, bridgeBaseline: 10_000, partComment: 10_000, partClip: 10_000, partCaps: 10_000, partRoom: 10_000, partSample: 10_000, extLoader: 10_000, extOverlay: 10_000, extWorker: 10_000, extComposer: 10_000, extPanel: 10_000 };

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
    const sizes = (r: ReturnType<typeof run>) => Object.fromEntries([...r.stdout.matchAll(/(loader|overlay|worker|composer|panel) (\d+)/g)].map(m => [m[1], Number(m[2])]));
    const s = sizes(run(full));
    expect(s.panel).toBeGreaterThan(s.composer);
    for (const k of ["loader", "overlay", "worker", "composer", "panel"]) expect(s[k], k).toBeGreaterThan(0);
    for (const [key, size] of [["extPanel", s.panel], ["extComposer", s.composer], ["extLoader", s.loader], ["extOverlay", s.overlay], ["extWorker", s.worker]] as const) {
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

  it("never records the extension's ceilings, nor adds a missing one", () => {
    expect(run(full, ENTRY, ["--record"]).status).toBe(0);
    const recorded = JSON.parse(readFileSync(join(root, "web/perf/bundle-budget.json"), "utf8"));
    for (const k of ["extLoader", "extOverlay", "extWorker", "extComposer", "extPanel"]) expect(recorded[k], k).toBe(10_000);
    expect(recorded.gallery).toBeLessThan(10_000);
    rmSync(root, { recursive: true, force: true });
    const { extPanel: _, ...noPanel } = full;
    const r = run(noPanel, ENTRY, ["--record"]);
    expect(r.status).toBe(1);
    expect(r.stderr).toContain("extPanel");
  });

  it.each(["gallery", "artifact", "bridge", "bridgeBaseline", "partComment", "partClip", "partCaps", "partRoom", "partSample", "extLoader", "extOverlay", "extWorker", "extComposer", "extPanel"])("fails when the %s budget is missing or not a number", k => {
    for (const budget of [Object.fromEntries(Object.entries(full).filter(([key]) => key !== k)), { ...full, [k]: "10000" }]) {
      const r = run(budget);
      expect(r.status).toBe(1);
      expect(r.stderr).toContain(`lacks a numeric budget for: ${k}`);
      rmSync(root, { recursive: true, force: true });
    }
  });
});
