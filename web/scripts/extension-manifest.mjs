// The manifest a build of the Chrome extension ships: the committed
// web/extension/manifest.json with the workspace's version (from the
// Cargo.toml beside web/), the committed public key as `key` when
// web/extension/key/key.pub.b64 holds one (so the extension's ID is the
// key's, the same one the daemon derives; crates/clax-core/build.rs), and,
// in the test build only, `host_permissions: ["<all_urls>"]`.
import { existsSync, readFileSync } from "node:fs";

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
