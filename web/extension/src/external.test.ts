// Nothing outside the extension can message it: the manifest declares no
// `externally_connectable` (an empty one only draws a load warning), so no web
// page can connect, and no part of the extension listens for other
// extensions' messages, so theirs are dropped.
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const root = join(__dirname, "..");

function sources(dir: string): string[] {
  return readdirSync(dir).flatMap(name => {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) return sources(path);
    return /\.(ts|svelte)$/.test(name) && !name.endsWith(".test.ts") ? [path] : [];
  });
}

describe("external messaging", () => {
  it("the manifest declares no externally_connectable", () => {
    const manifest = JSON.parse(readFileSync(join(root, "manifest.json"), "utf8"));
    expect(manifest).not.toHaveProperty("externally_connectable");
  });

  it("no source listens for other extensions' messages", () => {
    const listeners = sources(join(root, "src")).filter(f => /on(Message|Connect)External\b/.test(readFileSync(f, "utf8")));
    expect(listeners).toEqual([]);
  });
});
