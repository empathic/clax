import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

// The daemon attributes a request by its `x-clax-*` headers (audit spec
// §5.2, §6.9). The shell and the bridge run beside page code, so none of
// those headers may carry a value a page supplies: the only one they send is
// the shell's literal `x-clax-via: page` on a page's own publish.
function sources(dir: string): string[] {
  return readdirSync(dir).flatMap(name => {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) return sources(p);
    return /\.(ts|svelte|js)$/.test(name) && !/\.test\.ts$/.test(name) ? [p] : [];
  });
}

describe("x-clax request headers", () => {
  it("are only the shell's literal page marker", () => {
    const found: string[] = [];
    for (const dir of ["..", "../../bridge/src", "../../extension/src"]) {
      for (const file of sources(join(__dirname, dir))) {
        const text = readFileSync(file, "utf8");
        for (const m of text.matchAll(/["'`]x-clax-[a-z-]+["'`]\s*:?\s*[^,}\n]*/gi)) found.push(m[0].trim());
      }
    }
    expect(found).toEqual([`"x-clax-via": "page"`]);
  });
});
