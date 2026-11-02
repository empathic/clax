import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { extensionManifest, stamped } from "./extension-manifest.mjs";

let root = "";
afterEach(() => { if (root) rmSync(root, { recursive: true, force: true }); root = ""; });

/** A scratch web/ with the committed manifest, a Cargo.toml one level up, and `key` as key.pub.b64 when given. */
function scratch(key?: string) {
  root = mkdtempSync(join(tmpdir(), "clax-ext-manifest-"));
  const web = join(root, "web");
  mkdirSync(join(web, "extension/key"), { recursive: true });
  writeFileSync(join(root, "Cargo.toml"), `[workspace]\nmembers = []\n\n[workspace.package]\nversion = "1.2.3"\n`);
  writeFileSync(join(web, "extension/manifest.json"), JSON.stringify({ manifest_version: 3, name: "Clax", version: "0.0.0", optional_host_permissions: ["http://*/*", "https://*/*"] }));
  if (key !== undefined) writeFileSync(join(web, "extension/key/key.pub.b64"), key);
  return `${web}/`;
}

describe("extensionManifest", () => {
  it("takes the workspace's version and adds no key and no host permissions to the release build", () => {
    const m = extensionManifest(scratch(), { test: false });
    expect(m.version).toBe("1.2.3");
    expect(m).not.toHaveProperty("key");
    expect(m).not.toHaveProperty("host_permissions");
    expect(m.optional_host_permissions).toEqual(["http://*/*", "https://*/*"]);
  });

  it("grants every origin in the test build", () => {
    expect(extensionManifest(scratch(), { test: true }).host_permissions).toEqual(["<all_urls>"]);
  });

  it("carries the committed public key, trimmed, in both builds", () => {
    const web = scratch("MIIBIjANBgkq\n");
    expect(extensionManifest(web, { test: false }).key).toBe("MIIBIjANBgkq");
    expect(extensionManifest(web, { test: true }).key).toBe("MIIBIjANBgkq");
  });

  it("ignores an empty key file, as the daemon does", () => {
    expect(extensionManifest(scratch(" \n"), { test: false })).not.toHaveProperty("key");
  });
});

describe("stamped", () => {
  /** A scratch build directory holding `files` (path → contents) and a manifest.json. */
  function build(files: Record<string, string>) {
    const dir = mkdtempSync(join(tmpdir(), "clax-ext-stamp-"));
    for (const [p, body] of Object.entries(files)) {
      mkdirSync(join(dir, p, ".."), { recursive: true });
      writeFileSync(join(dir, p), body);
    }
    writeFileSync(join(dir, "manifest.json"), "{\"anything\": true}");
    return dir;
  }
  const dirs: string[] = [];
  afterEach(() => { for (const d of dirs.splice(0)) rmSync(d, { recursive: true, force: true }); });
  const stamp = (files: Record<string, string>, m: Parameters<typeof stamped>[1] = { version: "1.2.3" }) => {
    const d = build(files);
    dirs.push(d);
    return stamped(d, m).version_name;
  };

  it("names the version and twelve hex digits", () => {
    expect(stamp({ "sw.js": "a" })).toMatch(/^1\.2\.3 \([0-9a-f]{12}\)$/);
  });

  it("is the same for the same files, whatever manifest.json held and whatever version_name the input had", () => {
    expect(stamp({ "sw.js": "a", "icons/16.png": "i" })).toBe(stamp({ "icons/16.png": "i", "sw.js": "a" }));
    expect(stamp({ "sw.js": "a" }, { version: "1.2.3", version_name: "old" })).toBe(stamp({ "sw.js": "a" }));
  });

  it("changes with any file's contents or path, an added file, and the manifest", () => {
    const base = stamp({ "sw.js": "a", "icons/16.png": "i" });
    expect(stamp({ "sw.js": "b", "icons/16.png": "i" })).not.toBe(base);
    expect(stamp({ "sw.js": "a", "icons/32.png": "i" })).not.toBe(base);
    expect(stamp({ "sw.js": "a", "icons/16.png": "i", "x.js": "" })).not.toBe(base);
    expect(stamp({ "sw.js": "a", "icons/16.png": "i" }, { version: "1.2.3", permissions: ["alarms"] })).not.toBe(base);
    // A path and its contents do not run together.
    expect(stamp({ "ab": "c" })).not.toBe(stamp({ "a": "bc" }));
  });
});
