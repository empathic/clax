import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { extensionManifest } from "./extension-manifest.mjs";

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
