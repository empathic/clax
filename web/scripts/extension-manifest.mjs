// The manifest a build of the Chrome extension ships: the committed
// web/extension/manifest.json with the workspace's version (from the
// Cargo.toml beside web/), the committed public key as `key` when
// web/extension/key/key.pub.b64 holds one (so the extension's ID is the
// key's, the same one the daemon derives; crates/clax-core/build.rs), and,
// in the test build only, `host_permissions: ["<all_urls>"]`.
//
// `stamped` adds the build's identity as `version_name`: the version and
// the first twelve hex digits of a SHA-256 over every other file of the
// build and the manifest itself. Two builds with the same files have the
// same stamp; any changed file changes it. The worker compares the stamp of
// the manifest Chrome loaded with the one on disk to learn that
// `clax extension install` has put a newer build in place (sw/update.ts).
import { createHash } from "node:crypto";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join, relative } from "node:path";

/** `web` is the web/ directory, with a trailing slash. */
export function extensionManifest(web, { test }) {
  const cargo = readFileSync(`${web}../Cargo.toml`, "utf8");
  const version = cargo.match(/^version\s*=\s*"(\d+\.\d+\.\d+)/m)?.[1];
  if (!version) throw new Error("no workspace version in Cargo.toml");
  const manifest = JSON.parse(readFileSync(`${web}extension/manifest.json`, "utf8"));
  manifest.version = version;
  const keyFile = `${web}extension/key/key.pub.b64`;
  const key = existsSync(keyFile) ? readFileSync(keyFile, "utf8").trim() : "";
  if (key) manifest.key = key;
  if (test) manifest.host_permissions = ["<all_urls>"];
  return manifest;
}

/** Every file under `dir` but its top-level manifest.json, by path relative to `dir`, sorted. */
function filesOf(dir) {
  const out = [];
  const walk = d => {
    for (const e of readdirSync(d, { withFileTypes: true })) {
      const p = join(d, e.name);
      if (e.isDirectory()) walk(p);
      else if (e.isFile()) out.push(relative(dir, p).split("\\").join("/"));
    }
  };
  walk(dir);
  return out.filter(p => p !== "manifest.json").sort();
}

/** `manifest` with `version_name` set to `<version> (<12 hex>)`, the stamp of
 * the build in `dir` (every file there but manifest.json) and of `manifest`
 * without any `version_name`. */
export function stamped(dir, manifest) {
  const { version_name: _, ...plain } = manifest;
  const h = createHash("sha256");
  const part = bytes => { h.update(`${bytes.length}\n`); h.update(bytes); };
  part(Buffer.from(JSON.stringify(plain)));
  for (const p of filesOf(dir)) { part(Buffer.from(p)); part(readFileSync(join(dir, p))); }
  return { ...plain, version_name: `${plain.version} (${h.digest("hex").slice(0, 12)})` };
}
