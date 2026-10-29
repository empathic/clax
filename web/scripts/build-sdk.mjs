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
// so a consumer's `moduleResolution` of `bundler` or `nodenext` reads them.
// Its README is web/sdk/README.md. The package's license is the license of
// web/bridge, web/overlay and web/sdk in the repository's LICENSE, and the
// build fails on a bundled source file of Clax's from anywhere else;
// THIRD-PARTY-NOTICES.txt holds the license of each npm package it bundles.
//
//   node scripts/build-sdk.mjs            build once
//   node scripts/build-sdk.mjs --watch    build, then again on each change
import { mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { build } from "vite";
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

const web = fileURLToPath(new URL("..", import.meta.url));
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

/** The npm packages the bundle took modules from, with their license
 * texts. Fails on a Clax source file outside `LICENSED_DIRS`. */
function thirdPartyNotices(moduleIds) {
  const nm = `${sep}node_modules${sep}`;
  const roots = new Set();
  for (const id of moduleIds) {
    const at = id.lastIndexOf(nm);
    if (at < 0) {
      const file = id.split("?")[0];
      if (file.startsWith("\0") || LICENSED_DIRS.some(d => file.startsWith(join(web, d) + sep))) continue;
      throw new Error(`the package bundles ${file}, outside web/${LICENSED_DIRS.join(", web/")}, whose license LICENSE does not give the package`);
    }
    const rest = id.slice(at + nm.length).split(sep);
    roots.add(id.slice(0, at + nm.length) + rest.slice(0, rest[0].startsWith("@") ? 2 : 1).join(sep));
  }
  const sections = [...roots].sort().map(root => {
    const pkg = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
    const file = readdirSync(root).find(f => /^(licen[cs]e|copying)(\.|$)/i.test(f));
    if (!file) throw new Error(`${pkg.name} has no license file to carry`);
    return `${pkg.name} ${pkg.version} (${pkg.license})\n\n${readFileSync(join(root, file), "utf8").trim()}\n`;
  });
  return `The packages ${PACKAGE_NAME} bundles, and their licenses.\n\n${sections.join(`\n${"-".repeat(78)}\n\n`)}`;
}

/** The entries' declarations: TypeScript's for each source the entries'
 * declarations reach, under types/, each relative specifier resolved to
 * a declaration file the package carries and written with `.js`. Fails on
 * a declaration TypeScript cannot emit, a specifier that leaves the
 * package's sources or names another package, and a reference directive. */
function emitTypes() {
  const program = ts.createProgram(ENTRIES.map(e => join(web, e.src)), {
    target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext, moduleResolution: ts.ModuleResolutionKind.Bundler,
    lib: ["lib.es2022.d.ts", "lib.dom.d.ts", "lib.dom.iterable.d.ts"], types: [],
    strict: true, skipLibCheck: true, declaration: true, emitDeclarationOnly: true, rootDir: web, outDir: "/types",
  });
  const emitted = new Map();
  const result = program.emit(undefined, (file, text) => emitted.set(file.slice("/types/".length), text));
  if (result.diagnostics.length) {
    throw new Error(ts.formatDiagnostics(result.diagnostics, { getCanonicalFileName: f => f, getCurrentDirectory: () => web, getNewLine: () => "\n" }));
  }
  const kept = new Map();
  const visit = file => {
    if (kept.has(file)) return;
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
 * the modules the bundle took (for THIRD-PARTY-NOTICES.txt). */
const packageFiles = {
  name: "clax-sdk-package-files",
  apply: "build",
  writeBundle(_, bundle) {
    const ids = new Set(Object.values(bundle).flatMap(c => (c.type === "chunk" ? Object.keys(c.modules) : [])));
    writeMetadata(ids);
    emitTypes();
  },
};

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
    lib: { entry: Object.fromEntries(ENTRIES.map(e => [e.name, join(web, e.src)])), formats: ["es"], fileName: (_, name) => `${name}.js` },
    rollupOptions: { output: { chunkFileNames: "chunks/[name].js" } },
    watch: watch ? {} : null,
  },
});
if (watch) result.on("event", e => { if (e.code === "ERROR") console.error(e.error); });
