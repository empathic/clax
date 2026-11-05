// What worker-build 0.8.7 does after rustc, for a Bazel action
// (bazel/wasm/worker.bzl): from the Worker's wasm module to the directory
// wrangler deploys (index.js, index_bg.wasm, package.json, worker/shim.mjs).
//
//   node worker_bundle.mjs --wasm M --out DIR --wasm-bindgen B --wasm-opt O
//       --esbuild E --shim SHIM --sdk-version V --package-json JSON
//
// The steps and their arguments are worker-build's for `--release` on a
// module target with panic recovery (its src/main.rs and src/build/mod.rs):
// wasm-bindgen, wasm-opt, the producers section naming the workers-rs SDK,
// the shim with a handler per exported entry point and a proxy per exported
// class, bundled by esbuild.

import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, readdirSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

const args = {};
for (let i = 2; i < process.argv.length; i += 2) {
  args[process.argv[i].replace(/^--/, "")] = process.argv[i + 1];
}
for (const k of ["wasm", "out", "wasm-bindgen", "wasm-opt", "esbuild", "shim", "sdk-version", "package-json"]) {
  if (!args[k]) throw new Error(`worker_bundle.mjs: --${k} is required`);
}
const out = resolve(args.out);
const run = (bin, argv, cwd) => execFileSync(resolve(bin), argv, { cwd, stdio: ["ignore", "inherit", "inherit"] });
mkdirSync(out, { recursive: true });

// wasm-bindgen, as worker-build runs it.
run(args["wasm-bindgen"], [
  resolve(args.wasm), "--out-dir", out, "--no-typescript", "--target", "module", "--out-name", "index",
  "--experimental-reset-state-function", "--force-enable-abort-handler",
]);

// wasm-opt -O, with worker-build's explicit feature set (no encoding
// workerd does not read yet) and the names section kept.
const features = ["mutable-globals", "sign-ext", "nontrapping-float-to-int", "bulk-memory", "multivalue",
  "reference-types", "exception-handling"];
for (const f of readdirSync(out).filter((f) => f.endsWith(".wasm"))) {
  const path = join(out, f), tmp = path.replace(/\.wasm$/, ".wasm-opt.wasm");
  run(args["wasm-opt"], [path, "-o", tmp, "-O", ...features.map((x) => `--enable-${x}`), "--debuginfo"]);
  renameSync(tmp, path);
}

writeFileSync(join(out, "package.json"), JSON.stringify(JSON.parse(args["package-json"]), null, 2));

// The producers section names the SDK (worker-build's src/producers.rs).
function leb(n) {
  const b = [];
  do { let x = n & 0x7f; n >>>= 7; if (n) x |= 0x80; b.push(x); } while (n);
  return Buffer.from(b);
}
function readLeb(buf, pos) {
  let result = 0, shift = 0;
  for (;;) {
    const byte = buf[pos.at++];
    if (byte === undefined) throw new Error("invalid LEB128 value");
    result |= (byte & 0x7f) << shift;
    if (!(byte & 0x80)) return result >>> 0;
    shift += 7;
    if (shift >= 32) throw new Error("LEB128 value overflow");
  }
}
const name = (s) => Buffer.concat([leb(Buffer.byteLength(s)), Buffer.from(s)]);
function readName(buf, pos, limit) {
  const len = readLeb(buf, pos), end = pos.at + len;
  if (end > limit) throw new Error("invalid wasm name");
  const s = buf.subarray(pos.at, end).toString("utf8");
  pos.at = end;
  return s;
}
function withSdk(buf, version) {
  if (buf.length < 8 || buf.readUInt32BE(0) !== 0x0061736d || buf.readUInt32LE(4) !== 1) throw new Error("invalid wasm header");
  let section = null;
  const pos = { at: 8 };
  while (pos.at < buf.length) {
    const start = pos.at, id = buf[pos.at++], len = readLeb(buf, pos), end = pos.at + len;
    if (end > buf.length) throw new Error("invalid wasm section length");
    if (id === 0) {
      const p = { at: pos.at };
      if (readName(buf, p, end) === "producers") { section = { start, end, content: p.at }; break; }
    }
    pos.at = end;
  }
  const entries = [];
  if (section) {
    const p = { at: section.content };
    for (let fields = readLeb(buf, p); fields > 0; fields--) {
      const field = readName(buf, p, section.end);
      for (let values = readLeb(buf, p); values > 0; values--) {
        entries.push({ field, name: readName(buf, p, section.end), version: readName(buf, p, section.end) });
      }
    }
    if (p.at !== section.end) throw new Error("invalid producers section");
  }
  if (!entries.some((e) => e.field === "sdk" && e.name === "workers-rs")) {
    entries.push({ field: "sdk", name: "workers-rs", version });
  }
  const fields = [...new Set(entries.map((e) => e.field))];
  const content = [leb(fields.length)];
  for (const field of fields) {
    const these = entries.filter((e) => e.field === field);
    content.push(name(field), leb(these.length), ...these.flatMap((e) => [name(e.name), name(e.version)]));
  }
  const payload = Buffer.concat([name("producers"), ...content]);
  const encoded = Buffer.concat([Buffer.from([0]), leb(payload.length), payload]);
  return section
    ? Buffer.concat([buf.subarray(0, section.start), encoded, buf.subarray(section.end)])
    : Buffer.concat([buf, encoded]);
}
for (const f of readdirSync(out).filter((f) => f.endsWith(".wasm"))) {
  writeFileSync(join(out, f), withSdk(readFileSync(join(out, f)), args["sdk-version"]));
}

// The shim: a handler on the entry point for each function index.js exports
// (fetch, queue, scheduled, email and connect take env and ctx), and each
// exported class behind the proxy that reinitializes after a critical error.
const index = readFileSync(join(out, "index.js"), "utf8");
const exportDecls = [...index.matchAll(/export /g)]
  .filter((m) => m.index === 0 || /[\s;}]/.test(index[m.index - 1]))
  .map((m) => index.slice(m.index));
const SYSTEM = ["__wbg_reset_state", "__worker_init_state"];
const fns = [], classes = [];
for (const line of exportDecls) {
  let m;
  if ((m = line.match(/^export (?:async )?function\s*([^(]*)\(/))) {
    if (!SYSTEM.includes(m[1].trim())) fns.push(m[1].trim());
  } else if ((m = line.match(/^export \{[^}]*? as ([^}]*)\}/))) {
    if (!SYSTEM.includes(m[1].trim())) fns.push(m[1].trim());
  } else if ((m = line.match(/^export class ([^{]*)\{/))) {
    classes.push(m[1].trim());
  } else if ((m = line.match(/^export var ([^=]*)=\s*class/))) {
    classes.push(m[1].trim());
  }
}
const handlers = fns.map((f) => ["fetch", "queue", "scheduled", "email", "connect"].includes(f)
  ? `Entrypoint.prototype.${f} = function ${f} (arg) {\n  return exports.${f}.call(this, arg, this.env, this.ctx);\n}\n`
  : `Entrypoint.prototype.${f} = exports.${f};\n`).join("");
let shim = readFileSync(args.shim, "utf8").replace("$HANDLERS", handlers);
for (const c of classes) shim += `export const ${c} = new Proxy(exports.${c}, classProxyHooks);\n`;
writeFileSync(join(out, "shim.js"), shim);

run(args.esbuild, ["--external:./index_bg.wasm", "--external:cloudflare:*", "--format=esm", "--bundle", "./shim.js",
  "--outfile=index.js", "--allow-overwrite", "--minify", "--log-level=warning"], out);
writeFileSync(join(out, "index.js"), readFileSync(join(out, "index.js"), "utf8").replaceAll("import source ", "import "));
rmSync(join(out, "shim.js"));
rmSync(join(out, "snippets"), { recursive: true, force: true });

mkdirSync(join(out, "worker"), { recursive: true });
writeFileSync(join(out, "worker", "shim.mjs"),
  "// Use index.js directly, this file provided for backwards compat\n// with former shim.mjs only.\n" +
  "export * from '../index.js';\nexport { default } from '../index.js';\n");
