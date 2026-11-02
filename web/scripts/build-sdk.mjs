// Builds the SDK package (@empathic/clax-sdk) into web/dist-sdk/: a
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
// chunks/, so a class such as ClaxError is the same in every entry. Nothing
// is minified (the consumer's bundler minifies) and nothing is left to a
// dependency: the package declares none. Each entry has declarations
// under types/, emitted by TypeScript from the sources and kept to what the
// entries' declarations import, with relative specifiers ending in `.js`
// so a consumer's `moduleResolution` of `bundler` or `nodenext` reads them;
// `types` and `typesVersions` name them for `node` (node10).
// Its README is web/sdk/README.md. The package's license is the license of
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
import { dirname, join, resolve, sep } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { cleanOut } from "./extension-clean.mjs";
import { componentCssIntoScript } from "./overlay-component-css.mjs";

/** The package's name on npm. */
export const PACKAGE_NAME = "@empathic/clax-sdk";
/** The package's license: web/bridge's, web/overlay's and web/sdk's in LICENSE. */
const LICENSE = "LGPL-2.1-only";
/** The directories, under web/, of the Clax sources the package may bundle: those LICENSE puts under `LICENSE`. */
const LICENSED_DIRS = ["bridge", "overlay", "sdk"];

/** The package's entries: its export, the file it builds to and the source. */
export const ENTRIES = [
  { path: ".", name: "index", src: "sdk/src/index.ts" },
  { path: "./overlay", name: "overlay", src: "overlay/src/main.ts" },
  { path: "./service", name: "service", src: "sdk/src/service.ts" },
];

const web = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repo = resolve(web, "..");
const out = join(web, "dist-sdk");
const ts = createRequire(join(web, "package.json"))("typescript");
const types = src => `types/${src.replace(/\.ts$/, ".d.ts")}`;

function packageJson() {
  const version = readFileSync(join(repo, "Cargo.toml"), "utf8").match(/^\[workspace\.package\][^[]*?^version\s*=\s*"([^"]+)"/m)?.[1];
  if (!version) throw new Error("no [workspace.package] version in Cargo.toml");
  return {
    name: PACKAGE_NAME,
    version,
    description: "Clax's report queue SDK: the reporter client, the embeddable overlay and the service client",
    keywords: ["clax", "reports", "feedback", "overlay", "sdk"],
    license: LICENSE,
    author: { name: "Empathic, Inc.", email: "alex@empathic.dev" },
    homepage: "https://github.com/empathic/clax/tree/main/web/sdk#readme",
    bugs: { url: "https://github.com/empathic/clax/issues" },
    repository: { type: "git", url: "git+https://github.com/empathic/clax.git", directory: "web" },
    // A scoped package publishes as private unless told otherwise.
    publishConfig: { access: "public" },
    type: "module",
    sideEffects: false,
    exports: {
      ...Object.fromEntries(ENTRIES.map(e => [e.path, { types: `./${types(e.src)}`, default: `./${e.name}.js` }])),
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

/** Fails unless `file`, a path under `root` (web/), lies in one of
 * `LICENSED_DIRS`. */
function assertLicensed(file, root) {
  if (LICENSED_DIRS.some(d => file.startsWith(join(root, d) + sep))) return;
  throw new Error(`the package bundles ${file}, outside web/${LICENSED_DIRS.join(", web/")}, whose license LICENSE does not give the package`);
}

/** The npm packages the bundle took modules from (`moduleIds`, Rollup's),
 * with their license texts. Fails on a Clax source file outside
 * `LICENSED_DIRS` of `root` (web/), and on a package with no license file. */
export function thirdPartyNotices(moduleIds, root = web) {
  const nm = `${sep}node_modules${sep}`;
  const roots = new Set();
  for (const id of moduleIds) {
    const at = id.lastIndexOf(nm);
    if (at < 0) {
      const file = id.split("?")[0];
      if (!file.startsWith("\0")) assertLicensed(file, root);
      continue;
    }
    const rest = id.slice(at + nm.length).split(sep);
    roots.add(id.slice(0, at + nm.length) + rest.slice(0, rest[0].startsWith("@") ? 2 : 1).join(sep));
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
 * directive. */
export function keepDeclarations(emitted, root = web) {
  const kept = new Map();
  const visit = file => {
    if (kept.has(file)) return;
    assertLicensed(join(root, file), root);
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
      return `${lead}${q}${spec}${target.endsWith("/index.d.ts") ? "/index" : ""}.js${q}`;
    }));
  };
  for (const e of ENTRIES) visit(e.src.replace(/\.ts$/, ".d.ts"));
  return kept;
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
  const kept = keepDeclarations(emitted);
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
 * `import type` alone (sdk/src/types.ts, overlay/src/options.ts): every
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
      outDir: out, emptyOutDir: false, minify: false, sourcemap: false, cssCodeSplit: false,
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
}
