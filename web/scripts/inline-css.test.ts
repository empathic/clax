// The shell build's inline-CSS plugin, against small two-entry builds: it
// inlines each entry's CSS and refuses CSS that an entry would load only
// lazily, judged per entry.
import { spawnSync } from "node:child_process";
import { mkdtempSync, readdirSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";

let root = "";
afterEach(() => { if (root) rmSync(root, { recursive: true, force: true }); root = ""; });

// The build runs in a child Node (esbuild needs Node's own TextEncoder, which jsdom replaces).
const BUILD = `
import { build } from "vite";
const { inlineCss } = await import(process.argv[2]);
const root = process.argv[1];
await build({ root, configFile: false, logLevel: "silent", plugins: [inlineCss()],
  build: { outDir: root + "/dist", rollupOptions: { input: { a: root + "/a.html", b: root + "/b.html" } } } });
`;

/** Builds entries a.html and b.html (each runs its a.js or b.js) from `files`;
 * returns the build's exit status, its errors, and the built HTML and CSS file names. */
function run(files: Record<string, string>) {
  // The real path: Vite names the HTML outputs relative to the root, and macOS links /var to /private/var.
  root = realpathSync(mkdtempSync(join(tmpdir(), "clax-inline-css-")));
  for (const e of ["a", "b"]) writeFileSync(join(root, `${e}.html`), `<!doctype html><html><head><script type="module" src="./${e}.js"></script></head><body></body></html>`);
  for (const [name, text] of Object.entries(files)) writeFileSync(join(root, name), text);
  const r = spawnSync(process.execPath, ["--input-type=module", "-e", BUILD, root, join(__dirname, "../vite.shell.config.ts")], { cwd: join(__dirname, ".."), encoding: "utf8" });
  const built = r.status === 0 ? readdirSync(join(root, "dist"), { recursive: true }).map(String) : [];
  const html = (n: string) => readFileSync(join(root, "dist", n), "utf8");
  return { status: r.status, stderr: r.stderr, built, html };
}

const SHARED = {
  "shared.js": `import "./x.css"; export const v = () => document.title; console.log("shared");`,
  "x.css": `.shared-rule { color: red }`,
  "lazy.js": `import { v } from "./shared.js"; console.log("lazy", v());`,
};

/** Each test runs a child Node (a whole Vite build): bounded work that takes about a
 * second on an idle machine and many times that on a loaded one. The limit
 * is there to end a run that hangs, not to judge its speed. */
const CHILD_TIMEOUT_MS = 60_000;

describe("inlineCss", { timeout: CHILD_TIMEOUT_MS }, () => {
  it("inlines each entry's CSS when every chunk with CSS is static in the entries that load it", () => {
    const r = run({ ...SHARED, "a.js": `import { v } from "./shared.js"; console.log("a", v());`, "b.js": `import { v } from "./shared.js"; console.log("b", v()); import("./lazy.js");` });
    expect(r.status, r.stderr).toBe(0);
    for (const n of ["a.html", "b.html"]) expect(r.html(n)).toContain(".shared-rule");
    expect(r.built.filter(f => f.endsWith(".css"))).toEqual([]);
  });

  it("refuses a chunk that one entry imports statically and another loads only lazily", () => {
    const r = run({ ...SHARED, "a.js": `import { v } from "./shared.js"; console.log("a", v());`, "b.js": `import("./lazy.js");` });
    expect(r.status).not.toBe(0);
    expect(r.stderr).toMatch(/is loaded lazily by assets\/b-[^ ]*\.js and imports CSS/);
  });

  it("refuses a lazy chunk with CSS of its own", () => {
    const r = run({ "a.js": `console.log(1);`, "b.js": `import("./lazy.js");`, "lazy.js": `import "./y.css"; console.log(2);`, "y.css": `.y { color: blue }` });
    expect(r.status).not.toBe(0);
    expect(r.stderr).toMatch(/is loaded lazily by assets\/b-[^ ]*\.js/);
  });
});
