// The built SDK package (web/dist-sdk, scripts/build-sdk.mjs), as an app
// that installs it gets it, from the tarball `npm pack` makes of it, which
// is what npm would publish: its package.json, each export resolved by Node
// and imported, the overlay run from its own bundle, and its declarations
// read by TypeScript as a consumer's. Run by `pnpm run test:sdk` after the
// build (//web:sdk_package under Bazel), not by the unit tests.
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, isAbsolute, join, relative, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { afterAll, beforeAll, describe, expect, it } from "vitest";

const web = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const pkgDir = join(web, "dist-sdk");
const ts = createRequire(join(web, "package.json"))("typescript") as typeof import("typescript");

interface Manifest {
  name: string;
  version: string;
  license: string;
  type: string;
  sideEffects: boolean;
  exports: Record<string, { types: string; default: string } | string>;
  [k: string]: unknown;
}

/** What each export gives at run time. */
const RUNTIME: Record<string, string[]> = {
  ".": ["ClaxError", "ReporterClient"],
  "./overlay": ["overlay"],
  "./service": ["ServiceClient"],
};

let manifest: Manifest;
let app: string;
/** The package as installed in the app, from its tarball. */
let pkg: string;

beforeAll(() => {
  if (!existsSync(join(pkgDir, "package.json"))) throw new Error("web/dist-sdk has no package.json: run node scripts/build-sdk.mjs first");
  manifest = JSON.parse(readFileSync(join(pkgDir, "package.json"), "utf8"));
  // An app with the package installed under its name: what `npm pack`
  // puts in the tarball npm would publish, extracted (no links). npm runs
  // offline, with its cache in the app's directory: under Bazel the Node
  // toolchain's (CLAX_NPM_CLI, relative to the runfiles root), else the
  // `npm` on PATH.
  app = mkdtempSync(join(tmpdir(), "clax-sdk-app-"));
  const cli = process.env.CLAX_NPM_CLI;
  const runfiles = join(process.env.JS_BINARY__RUNFILES ?? "", process.env.JS_BINARY__WORKSPACE ?? "_main");
  const [cmd, pre] = cli ? [process.execPath, [isAbsolute(cli) ? cli : join(runfiles, cli)]] : ["npm", []];
  const packed = JSON.parse(execFileSync(cmd, [...pre, "pack", "--json", "--offline", "--ignore-scripts", "--pack-destination", app], {
    cwd: pkgDir, encoding: "utf8", env: { ...process.env, npm_config_cache: join(app, ".npm"), npm_config_update_notifier: "false" },
  }));
  pkg = join(app, "node_modules", ...manifest.name.split("/"));
  mkdirSync(pkg, { recursive: true });
  execFileSync("tar", ["-xzf", join(app, packed[0].filename), "-C", pkg, "--strip-components=1"]);
  manifest = JSON.parse(readFileSync(join(pkg, "package.json"), "utf8"));
  writeFileSync(join(app, "package.json"), JSON.stringify({ name: "app", private: true, type: "module" }));
});
afterAll(() => { if (app) rmSync(app, { recursive: true, force: true }); });

const files = (dir: string): string[] =>
  readdirSync(dir, { withFileTypes: true }).flatMap(e => (e.isDirectory() ? files(join(dir, e.name)) : [join(dir, e.name)]));

describe("the SDK package", () => {
  it("names itself, its license and its entries, and depends on nothing", () => {
    expect(manifest.name).toBe("@empathic/clax-sdk");
    expect(manifest.version).toMatch(/^\d+\.\d+\.\d+/);
    expect(manifest.license).toBe("LGPL-2.1-only");
    expect(manifest.publishConfig).toEqual({ access: "public" });
    expect(manifest.author).toEqual({ name: "Empathic, Inc.", email: "alex@empathic.dev" });
    expect(manifest.homepage).toBe("https://github.com/empathic/clax/tree/main/web/sdk#readme");
    expect(manifest.bugs).toEqual({ url: "https://github.com/empathic/clax/issues" });
    expect(manifest.keywords).toContain("clax");
    expect(manifest.type).toBe("module");
    expect(manifest.sideEffects).toBe(false);
    for (const k of ["dependencies", "peerDependencies", "optionalDependencies", "bundleDependencies"]) expect(manifest).not.toHaveProperty(k);
    expect(Object.keys(manifest.exports).sort()).toEqual([".", "./overlay", "./package.json", "./service"]);
    expect(manifest.types).toBe("./types/sdk/src/index.d.ts");
    expect(manifest.typesVersions).toEqual({ "*": { overlay: ["./types/overlay/src/main.d.ts"], service: ["./types/sdk/src/service.d.ts"] } });
    for (const k of Object.keys(RUNTIME)) {
      const e = manifest.exports[k];
      if (typeof e === "string") throw new Error(`${k} has no conditions`);
      expect(Object.keys(e)).toEqual(["types", "default"]);
      expect(existsSync(join(pkg, e.types)), e.types).toBe(true);
      expect(existsSync(join(pkg, e.default)), e.default).toBe(true);
    }
  });

  it("carries its README", () => {
    expect(readFileSync(join(pkg, "README.md"), "utf8")).toBe(readFileSync(join(web, "sdk/README.md"), "utf8"));
  });

  it("carries its license text and the bundled packages' notices", () => {
    expect(readFileSync(join(pkg, "LICENSE"), "utf8")).toContain("GNU Lesser General Public License, version 2.1 only");
    expect(readFileSync(join(pkg, "LICENSES/LGPL-2.1-only.txt"), "utf8")).toBe(readFileSync(join(web, "../LICENSES/LGPL-2.1-only.txt"), "utf8"));
    const notices = readFileSync(join(pkg, "THIRD-PARTY-NOTICES.txt"), "utf8");
    for (const dep of ["svelte", "modern-screenshot"]) expect(notices).toMatch(new RegExp(`^${dep} \\S+ \\(MIT\\)$`, "m"));
  });

  it("resolves each export by name in Node, which imports it with no DOM", () => {
    const probe = join(app, "probe.mjs");
    writeFileSync(probe, `const out = {};
for (const k of ${JSON.stringify(Object.keys(RUNTIME))}) out[k] = Object.keys(await import(${JSON.stringify(manifest.name)} + k.slice(1))).sort();
console.log(JSON.stringify(out));
`);
    const out = JSON.parse(execFileSync(process.execPath, [probe], { cwd: app, encoding: "utf8" }));
    expect(out).toEqual(RUNTIME);
  });

  it("changes nothing on import, and runs the overlay with its components' CSS from its own bundle", async () => {
    const before = { html: document.documentElement.outerHTML, globals: Object.keys(globalThis).sort() };
    const mod = await import(/* @vite-ignore */ pathToFileURL(join(pkg, "overlay.js")).href);
    expect({ html: document.documentElement.outerHTML, globals: Object.keys(globalThis).sort() }).toEqual(before);

    const handle = mod.overlay({ instance: "https://clax.example", queue: "q1", auth: async () => { throw new Error("no backend"); } });
    const root = document.querySelector("clax-overlay")?.shadowRoot;
    if (!root) throw new Error("the overlay added no shadow root");
    // jsdom has no constructed sheets: the overlay falls back to a <style> (bridge/src/sheet.ts).
    const css = [...(root.adoptedStyleSheets ?? []).flatMap(s => [...s.cssRules].map(r => r.cssText)), ...[...root.querySelectorAll("style")].map(s => s.textContent)].join("\n");
    expect(css).toMatch(/\.launcher\.svelte-/);
    handle.close();
    expect(document.querySelector("clax-overlay")).toBeNull();
    expect(readFileSync(join(pkg, "overlay.js"), "utf8")).not.toContain("__CLAX_OVERLAY_COMPONENT_CSS__");
  });

  it("keeps class fields and private members as written, for the app's bundler to lower if it must", () => {
    for (const f of files(pkg).filter(p => p.endsWith(".js"))) expect(readFileSync(f, "utf8"), relative(pkg, f)).not.toMatch(/__private(Get|Set|Add|Method)\b|__publicField\b/);
  });

  it("keeps its declarations inside the package, importing no other package", () => {
    const decls = files(join(pkg, "types")).filter(f => f.endsWith(".d.ts"));
    expect(decls.length).toBeGreaterThan(0);
    for (const f of decls) {
      expect(relative(join(pkg, "types"), f), "a declaration of web/bridge, web/overlay or web/sdk").toMatch(/^(bridge|overlay|sdk)\//);
      const text = readFileSync(f, "utf8");
      expect(text, f).not.toMatch(/^\/\/\/\s*<reference/m);
      for (const [, spec] of text.matchAll(/(?:\bfrom\s+|\bimport\s*\(\s*)["']([^"']+)["']/g)) {
        expect(spec, `${relative(pkg, f)} imports ${spec}`).toMatch(/^\.\.?\/.*\.js$/);
        const target = resolve(dirname(f), spec.replace(/\.js$/, ".d.ts"));
        expect(relative(pkg, target).startsWith(".."), `${relative(pkg, f)} imports ${spec}, outside the package`).toBe(false);
        expect(existsSync(target), `${relative(pkg, f)} imports ${spec}, which is missing`).toBe(true);
      }
    }
  });

  it.each([
    ["nodenext", ts.ModuleResolutionKind.NodeNext, ts.ModuleKind.NodeNext],
    ["bundler", ts.ModuleResolutionKind.Bundler, ts.ModuleKind.ESNext],
    // TypeScript's `node` (node10), which reads no `exports`.
    ["node10", ts.ModuleResolutionKind.Node10, ts.ModuleKind.CommonJS],
  ])(
    "type-checks an app's use of every entry under moduleResolution %s", (_, moduleResolution, module) => {
      const file = join(app, "app.ts");
      writeFileSync(file, `import { ClaxError, ReporterClient, type NewReport, type Report, type ReporterAuth, type ReporterToken } from "@empathic/clax-sdk";
import { overlay, type OverlayHandle, type OverlayOptions } from "@empathic/clax-sdk/overlay";
import { ServiceClient, type ServiceOptions } from "@empathic/clax-sdk/service";

const auth: ReporterAuth = async (publicKey: string): Promise<ReporterToken> => ({ token: publicKey, expires_at: "" });
const opts: OverlayOptions = { instance: "https://clax.example", queue: "q1", auth };
const handle: OverlayHandle = overlay(opts);
handle.close();
const client = new ReporterClient({ instance: opts.instance, queue: opts.queue, auth });
const report: NewReport = { url: "https://app.example/", title: "Home", anchor: null, body: "Hi" };
export const filed: Promise<Report> = client.fileReport(report);
export const status = (e: unknown): number | null => (e instanceof ClaxError ? e.status : null);
const service: ServiceOptions = { instance: "https://clax.example", queue: "q1", token: "cxq_", privateKey: "" };
export const minted: Promise<ReporterToken> = new ServiceClient(service).mintReporterToken({ reporter_id: "u1", origin: "https://app.example", public_key: "" });
// @ts-expect-error: the overlay's options need an auth.
overlay({ instance: "https://clax.example", queue: "q1" });
`);
      const program = ts.createProgram([file], {
        module, moduleResolution, target: ts.ScriptTarget.ES2022, lib: ["lib.es2022.d.ts", "lib.dom.d.ts"], types: [], strict: true, noEmit: true,
      });
      const diagnostics = ts.getPreEmitDiagnostics(program);
      expect(ts.formatDiagnostics(diagnostics, { getCanonicalFileName: f => f, getCurrentDirectory: () => app, getNewLine: () => "\n" })).toBe("");
    });
});
