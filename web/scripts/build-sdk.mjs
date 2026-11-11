// Builds the SDK package (@empathic/clax-client-sdk) into web/dist-sdk/: a
// self-contained npm package another app installs or links. Its entries:
//
//   "."          sdk/src/index.ts     the reporter client, ClaxError and the
//                                     queue routes' types; safe for a page
//   "./overlay"  overlay/src/main.ts  the embeddable overlay, Svelte bundled,
//                                     its components' CSS in its script
//                                     (overlay-component-css.mjs) as in
//                                     /_clax/overlay.js
//   "./service"  sdk/src/service.ts   the service client, for a backend
//
// Each entry is an ES module; code two entries share is one chunk under
// chunks/, so a class such as ClaxError is the same in every entry. The
// service entry, which a backend loads and which needs no DOM, is also one
// CommonJS file, service.cjs, under its `require` condition, for a
// CommonJS backend on a Node that cannot require an ES module (before
// 20.19 and 22.12); its ClaxError is its own, so such a backend takes
// ClaxError from the service entry too. The JavaScript is not minified
// (the consumer's bundler minifies it); the overlay's CSS, which reaches
// its script as string literals no consumer's bundler can minify, is
// minified here, by esbuild as in /_clax/overlay.js. Nothing is left to a
// dependency: the package declares none. Each entry has declarations
// under types/, emitted by TypeScript from the sources and kept to what the
// entries' declarations import, with relative specifiers ending in `.js`
// so a consumer's `moduleResolution` of `bundler` or `nodenext` reads them
// (and, for service.cjs, a copy of the service entry's as `.d.cts` with
// specifiers ending in `.cjs`, which TypeScript reads as CommonJS);
// `types` and `typesVersions` name them for `node` (node10).
// Its README is web/sdk/README.md, and its package.json is web/sdk/package.json
// (MANIFEST) with the fields the build derives. The package's license is the license of
// web/bridge, web/overlay and web/sdk in the repository's LICENSE, and the
// build fails on a bundled source file, or a declaration, of Clax's from
// anywhere else; THIRD-PARTY-NOTICES.txt holds the license of each npm
// package it bundles.
//
//   node scripts/build-sdk.mjs            build once
//   node scripts/build-sdk.mjs --watch    build, then again on each change
//
// Imported, it builds nothing (and loads no Vite): its tests
// (build-sdk.test.ts) call the license guard, the notices' collection and
// the declarations' on fixtures.
import { existsSync, mkdirSync, readdirSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, isAbsolute, join, resolve, sep } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { cleanOut } from "./extension-clean.mjs";
import { componentCssIntoScript } from "./overlay-component-css.mjs";

const web = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repo = resolve(web, "..");
const out = join(web, "dist-sdk");

/**
 * The package's manifest as written by hand, web/sdk/package.json: its
 * name, its version (a semver line of its own, not Clax's, since the
 * package only calls an instance's report-queue routes; a pull request to
 * main raises it and `just release-sdk` publishes it once prod carries
 * it), its license and the rest of its metadata. It is `private`, so the
 * source directory is never published; packageJson() adds what the build
 * derives. It is also the nearest package.json of web/sdk/src, which Vite
 * and Node read in Clax's own builds of those sources (the shell and
 * /_clax/overlay.js), so it sets nothing that changes how they bundle:
 * `sideEffects` is the published manifest's alone.
 */
export const MANIFEST = JSON.parse(readFileSync(join(web, "sdk", "package.json"), "utf8"));
/** The package's name on npm. */
export const PACKAGE_NAME = MANIFEST.name;
/** The package's license: web/bridge's, web/overlay's and web/sdk's in LICENSE. */
const LICENSE = MANIFEST.license;
/** The directories, under web/, of the Clax sources the package may bundle: those LICENSE puts under `LICENSE`. */
const LICENSED_DIRS = ["bridge", "overlay", "sdk"];

/** The package's entries: its export, the file it builds to, the source,
 * and whether it is also built as CommonJS (`<name>.cjs`). */
export const ENTRIES = [
  { path: ".", name: "index", src: "sdk/src/index.ts" },
  { path: "./overlay", name: "overlay", src: "overlay/src/main.ts" },
  { path: "./service", name: "service", src: "sdk/src/service.ts", cjs: true },
];

const ts = createRequire(join(web, "package.json"))("typescript");
const types = src => `types/${src.replace(/\.ts$/, ".d.ts")}`;
/** A declaration's CommonJS twin: `a.d.ts` as `a.d.cts`. */
const cjsTypes = file => file.replace(/\.d\.ts$/, ".d.cts");

/** An entry's `exports` conditions: its declarations and ES module, and
 * for a CommonJS entry the same under `import` and `require`. */
function conditions(e) {
  const esm = { types: `./${types(e.src)}`, default: `./${e.name}.js` };
  if (!e.cjs) return esm;
  return { import: esm, require: { types: `./${cjsTypes(types(e.src))}`, default: `./${e.name}.cjs` } };
}

/** The fields web/sdk/package.json may set, each published as written
 * (but `private`): a manifest that sets any other is refused, so a field
 * the build derives, a dependency (the package bundles them), a lifecycle
 * script or an executable never reaches the published package unseen. */
export const MANIFEST_FIELDS = ["name", "version", "private", "description", "keywords", "license", "author", "contributors", "homepage", "bugs", "repository", "funding", "publishConfig", "type"];

/** The published package.json: `manifest` without `private`, and its
 * entries' files and declarations. Fails on a manifest that sets a field
 * outside `MANIFEST_FIELDS` or is not `private`. */
export function packageJson(manifest = MANIFEST) {
  const other = Object.keys(manifest).filter(k => !MANIFEST_FIELDS.includes(k));
  if (other.length) throw new Error(`web/sdk/package.json sets ${other.join(", ")}, which it may not: it sets only ${MANIFEST_FIELDS.join(", ")}, and the build derives the rest`);
  if (manifest.private !== true) throw new Error("web/sdk/package.json must be private: the published manifest is the build's");
  const { private: _, ...meta } = manifest;
  return {
    ...meta,
    // Importing an entry changes nothing until it is called (the package's
    // test checks the overlay's), so an app's bundler may drop what it
    // does not use.
    sideEffects: false,
    exports: {
      ...Object.fromEntries(ENTRIES.map(e => [e.path, conditions(e)])),
      "./package.json": "./package.json",
    },
    // For TypeScript's `node` (node10) resolution, which reads no `exports`:
    // the main entry's declarations, and each subpath's.
    types: `./${types(ENTRIES.find(e => e.path === ".").src)}`,
    typesVersions: { "*": Object.fromEntries(ENTRIES.filter(e => e.path !== ".").map(e => [e.path.slice(2), [`./${types(e.src)}`]])) },
  };
}

/** LICENSE: the package's notice, then where its license texts are. */
function licenseNotice() {
  return `${PACKAGE_NAME}
Copyright (C) 2026 Empathic, Inc.
Contact: Alex Kesling <alex@empathic.dev>

This package is built from Clax's web/bridge, web/overlay and web/sdk, and is
licensed under the GNU Lesser General Public License, version 2.1 only
(${LICENSE}); no later version applies. Its full text is in
LICENSES/${LICENSE}.txt. The packages it bundles keep their own licenses,
which THIRD-PARTY-NOTICES.txt holds. The Clax repository's LICENSE
(https://github.com/empathic/clax) says which license covers each part of
Clax.

This program is distributed in the hope that it will be useful, but WITHOUT
ANY WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS
FOR A PARTICULAR PURPOSE. See the license text for details.
`;
}

/** `path` with `/` separators, as Vite writes module IDs on every platform. */
const slashed = path => path.split(sep).join("/");

/** Fails unless `file`, a path under `root` (web/) with `/` separators,
 * lies in one of `LICENSED_DIRS`. */
function assertLicensed(file, root) {
  if (LICENSED_DIRS.some(d => file.startsWith(`${slashed(join(root, d))}/`))) return;
  throw new Error(`the package bundles ${file}, outside web/${LICENSED_DIRS.join(", web/")}, whose license LICENSE does not give the package`);
}

/** The npm packages the bundle took modules from (`moduleIds`, Rollup's,
 * with `/` separators), with their license texts. A virtual module's ID
 * starts with `\0`: one that wraps a file (such as a CommonJS module's
 * `?commonjs-exports`) counts as that file, and one of no file (a helper
 * such as `\0commonjsHelpers.js`) carries nothing. Fails on a Clax source
 * file outside `LICENSED_DIRS` of `root` (web/), and on a package with no
 * license file. */
export function thirdPartyNotices(moduleIds, root = web) {
  const nm = "/node_modules/";
  const roots = new Set();
  for (const id of moduleIds) {
    const file = id.replace(/^\0/, "").split("?")[0];
    const at = file.lastIndexOf(nm);
    if (at < 0) {
      if (!id.startsWith("\0") || isAbsolute(file)) assertLicensed(file, root);
      continue;
    }
    const rest = file.slice(at + nm.length).split("/");
    roots.add(file.slice(0, at + nm.length) + rest.slice(0, rest[0].startsWith("@") ? 2 : 1).join("/"));
  }
  const sections = [...roots].sort().map(dir => {
    const pkg = JSON.parse(readFileSync(join(dir, "package.json"), "utf8"));
    const file = readdirSync(dir).find(f => /^(licen[cs]e|copying)(\.|$)/i.test(f));
    if (!file) throw new Error(`${pkg.name} has no license file to carry`);
    return `${pkg.name} ${pkg.version} (${pkg.license})\n\n${readFileSync(join(dir, file), "utf8").trim()}\n`;
  });
  return `The packages ${PACKAGE_NAME} bundles, and their licenses.\n\n${sections.join(`\n${"-".repeat(78)}\n\n`)}`;
}

const formatDiagnostics = d => ts.formatDiagnostics(d, { getCanonicalFileName: f => f, getCurrentDirectory: () => web, getNewLine: () => "\n" });

/** The declaration emit's compiler options: web/tsconfig.json's, which the
 * type check uses, with what an emit of the package's declarations needs. */
function declarationOptions() {
  const { config, error } = ts.readConfigFile(join(web, "tsconfig.json"), ts.sys.readFile);
  if (error) throw new Error(formatDiagnostics([error]));
  const parsed = ts.parseJsonConfigFileContent(config, ts.sys, web);
  if (parsed.errors.length) throw new Error(formatDiagnostics(parsed.errors));
  return {
    ...parsed.options,
    // tsconfig.json only type-checks: emit, and declarations alone.
    noEmit: false, declaration: true, emitDeclarationOnly: true,
    // types/ mirrors web/, whatever the program's common source directory.
    rootDir: web, outDir: "/types",
    // No ambient package's types (tsconfig.json's vite/client, node and
    // chrome): a declaration that needed one would carry a reference
    // directive to a package the SDK package does not depend on.
    types: [],
  };
}

/** The declarations the package keeps of TypeScript's `emitted` (each
 * declaration's path under `root`, web/, to its text): the entries' and
 * those they reach, each relative specifier resolved to a declaration file
 * the package carries and written with `.js` (the sources import their
 * siblings without an extension, which `nodenext` does not resolve). Fails
 * on a declaration of a source outside `LICENSED_DIRS`, a specifier that
 * names another package or a file with no declarations, and a reference
 * directive. With `cjs`, it keeps those of the CommonJS entries alone,
 * each as `.d.cts` with its specifiers ending in `.cjs`. */
export function keepDeclarations(emitted, root = web, { cjs = false } = {}) {
  const kept = new Map();
  const ext = cjs ? ".cjs" : ".js";
  const visit = file => {
    if (kept.has(file)) return;
    assertLicensed(slashed(join(root, file)), root);
    const text = emitted.get(file);
    if (text === undefined) throw new Error(`no declarations for ${file}`);
    kept.set(file, "");
    if (/^\/\/\/\s*<reference/m.test(text)) throw new Error(`${file} has a reference directive`);
    kept.set(file, text.replace(/(\bfrom\s+|\bimport\s*\(\s*|\bimport\s+)(["'])([^"']+)\2/g, (all, lead, q, spec) => {
      if (!spec.startsWith(".")) throw new Error(`${file} imports ${spec}; the package's declarations import only their own files`);
      const base = join(dirname(file), spec).split(sep).join("/");
      const target = [`${base}.d.ts`, `${base}/index.d.ts`].find(f => emitted.has(f));
      if (!target) throw new Error(`${file} imports ${spec}, which has no declarations`);
      visit(target);
      return `${lead}${q}${spec}${target.endsWith("/index.d.ts") ? "/index" : ""}${ext}${q}`;
    }));
  };
  for (const e of ENTRIES) if (!cjs || e.cjs) visit(e.src.replace(/\.ts$/, ".d.ts"));
  return cjs ? new Map([...kept].map(([file, text]) => [cjsTypes(file), text])) : kept;
}

/** The program the declarations are emitted from: the entries' sources and
 * what they import. */
const declarationProgram = () => ts.createProgram(ENTRIES.map(e => join(web, e.src)), declarationOptions());

/** Writes the entries' declarations from `program` under types/, in place
 * of what was there (keepDeclarations). Fails on a declaration TypeScript
 * cannot emit. */
function emitTypes(program) {
  const emitted = new Map();
  const result = program.emit(undefined, (file, text) => emitted.set(file.slice("/types/".length), text));
  if (result.diagnostics.length) throw new Error(formatDiagnostics(result.diagnostics));
  const kept = [...keepDeclarations(emitted), ...keepDeclarations(emitted, web, { cjs: true })];
  rmSync(join(out, "types"), { recursive: true, force: true });
  for (const [file, text] of kept) {
    const to = join(out, "types", file);
    mkdirSync(dirname(to), { recursive: true });
    writeFileSync(to, text);
  }
}

function writeMetadata(moduleIds) {
  writeFileSync(join(out, "package.json"), `${JSON.stringify(packageJson(), null, 2)}\n`);
  writeFileSync(join(out, "README.md"), readFileSync(join(web, "sdk", "README.md")));
  writeFileSync(join(out, "LICENSE"), licenseNotice());
  mkdirSync(join(out, "LICENSES"), { recursive: true });
  writeFileSync(join(out, "LICENSES", `${LICENSE}.txt`), readFileSync(join(repo, "LICENSES", `${LICENSE}.txt`)));
  writeFileSync(join(out, "THIRD-PARTY-NOTICES.txt"), thirdPartyNotices(moduleIds));
}

/** Writes everything but the JavaScript once each bundle is written, with
 * the modules the bundle took (for THIRD-PARTY-NOTICES.txt), and removes a
 * chunk an earlier build of `--watch` wrote that this one did not. Rollup
 * watches the bundle's modules only, which leaves out one reached by
 * `import type` alone (such as sdk/src/types.ts): every
 * source the declarations are emitted from is watched too. */
let program;
const packageFiles = {
  name: "clax-sdk-package-files",
  apply: "build",
  buildStart() {
    program = declarationProgram();
    for (const f of program.getSourceFiles()) if (!f.fileName.includes("/node_modules/")) this.addWatchFile(f.fileName);
  },
  writeBundle(_, bundle) {
    const chunks = join(out, "chunks");
    for (const f of existsSync(chunks) ? readdirSync(chunks) : []) if (!(`chunks/${f}` in bundle)) rmSync(join(chunks, f), { recursive: true });
    const ids = new Set(Object.values(bundle).flatMap(c => (c.type === "chunk" ? Object.keys(c.modules) : [])));
    writeMetadata(ids);
    emitTypes(program);
  },
};

if (process.argv[1] && import.meta.url === pathToFileURL(realpathSync(process.argv[1])).href) {
  // Vite only here: its esbuild refuses to load under a test's jsdom.
  const { build } = await import("vite");
  const { svelte } = await import("@sveltejs/vite-plugin-svelte");
  const watch = process.argv.includes("--watch");
  cleanOut(out);
  const result = await build({
    configFile: false, logLevel: watch ? "info" : "warn", publicDir: false, root: web,
    // No `svelte/internal/disclose-version`: importing the overlay must not
    // write to `window`.
    plugins: [svelte({ configFile: join(web, "svelte.config.js"), compilerOptions: { discloseVersion: false } }), componentCssIntoScript("overlay.js"), packageFiles],
    define: { "process.env.NODE_ENV": JSON.stringify("production") },
    build: {
      outDir: out, emptyOutDir: false, minify: false, cssMinify: "esbuild", sourcemap: false, cssCodeSplit: false,
      // The oldest browsers the package's code runs in already: those with
      // Object.hasOwn (web/bridge/src/describe.ts), which /_clax/overlay.js
      // needs too. Vite's default target lowers the syntax for older ones,
      // which gain nothing by it, turning Svelte's class fields and private
      // members into WeakMap helpers an app's bundler cannot undo; an app
      // that targets older browsers lowers the package in its own build.
      target: ["chrome93", "edge93", "firefox92", "safari15.4"],
      lib: { entry: Object.fromEntries(ENTRIES.map(e => [e.name, join(web, e.src)])), formats: ["es"], fileName: (_, name) => `${name}.js` },
      rollupOptions: { output: { chunkFileNames: "chunks/[name].js" } },
      watch: watch ? {} : null,
    },
  });
  if (watch) result.on("event", e => { if (e.code === "ERROR") console.error(e.error); });
  // The CommonJS entries, each one file of its own (no chunk is shared
  // with the ES modules), built after them so that their chunks/ and
  // metadata stand; the license guard holds for what they bundle too.
  const cjs = await build({
    configFile: false, logLevel: watch ? "info" : "warn", publicDir: false, root: web,
    plugins: [{ name: "clax-sdk-cjs-licensed", apply: "build", generateBundle(_, bundle) { thirdPartyNotices(Object.values(bundle).flatMap(c => (c.type === "chunk" ? Object.keys(c.modules) : []))); } }],
    define: { "process.env.NODE_ENV": JSON.stringify("production") },
    build: {
      outDir: out, emptyOutDir: false, minify: false, sourcemap: false, copyPublicDir: false,
      // Node 20, the oldest the service entry runs in.
      target: "node20",
      lib: { entry: Object.fromEntries(ENTRIES.filter(e => e.cjs).map(e => [e.name, join(web, e.src)])), formats: ["cjs"], fileName: (_, name) => `${name}.cjs` },
      rollupOptions: { output: { exports: "named" } },
      watch: watch ? {} : null,
    },
  });
  if (watch) cjs.on("event", e => { if (e.code === "ERROR") console.error(e.error); });
}
