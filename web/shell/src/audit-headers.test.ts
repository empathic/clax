import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

// The daemon attributes a request by its `x-clax-*` headers (audit spec
// §5.2, §6.9). The shell and the bridge run beside page code, so none of
// those headers may carry a value a page supplies: the only ones they send are
// the shell's literal `x-clax-via: page` on a page's own writes, which makes
// the daemon count every author of the page in them (multi-person spec
// §8.2, M11), and `x-clax-reader` naming the artifact the view serves on
// `db.artifact(id)`'s reads (caps/cross-read.ts), never an ID the page sends.
function sources(dir: string): string[] {
  return readdirSync(dir).flatMap(name => {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) return sources(p);
    return /\.(ts|svelte|js)$/.test(name) && !/\.test\.ts$/.test(name) ? [p] : [];
  });
}

const MARKER = `"x-clax-via": "page"`;
const read = (p: string) => readFileSync(join(__dirname, p), "utf8");

/** Capability handlers that write on a page's behalf, and the helpers they write through. */
const PAGE_WRITERS = ["caps/artifact.ts", "caps/assets.ts", "caps/db.ts", "caps/comments.ts", "threads.ts"];
/** Capability handlers that send a write method and why they need no marker. */
const NOT_PAGE_CONTENT: Record<string, string> = {
  "caps/sample.ts": "a model request: it writes no content of the page's into the store",
  "caps/cross-read.ts": "the owner's read grant, saved after the owner allows it in the shell's prompt: no content of the page's",
  "caps/net.ts": "the owner's network grant, saved after the owner allows it, and a fetch the daemon makes for the page: no content of the page's into the store",
};

describe("x-clax request headers", () => {
  it("are only the shell's literal page marker", () => {
    const found: string[] = [];
    for (const dir of ["..", "../../bridge/src", "../../extension/src"]) {
      for (const file of sources(join(__dirname, dir))) {
        const text = readFileSync(file, "utf8");
        for (const m of text.matchAll(/["'`]x-clax-[a-z-]+["'`]\s*:?\s*[^,}\n]*/gi)) found.push(m[0].trim());
      }
    }
    expect(found.length).toBeGreaterThan(0);
    expect(new Set(found)).toEqual(new Set([MARKER, `"x-clax-reader": reader`]));
    expect(readFileSync(join(__dirname, "caps/cross-read.ts"), "utf8")).toMatch(/\n  const reader = env\.aid;\n/);
  });

  it("mark every write a capability handler makes for a page", () => {
    for (const p of PAGE_WRITERS) expect(read(p), p).toContain(MARKER);
    // A handler that sends a write method is a page writer or says why not.
    const caps = readdirSync(join(__dirname, "caps")).filter(n => n.endsWith(".ts") && !n.endsWith(".test.ts"));
    for (const name of caps) {
      const p = `caps/${name}`;
      const writes = /method:\s*"(POST|PUT|PATCH|DELETE)"|fetch\(url,\s*\{\s*method[,\s]/.test(read(p));
      if (writes) expect(PAGE_WRITERS.includes(p) || p in NOT_PAGE_CONTENT, `${p} writes: mark it or say why not`).toBe(true);
    }
  });

  it("mark a thread the comments capability starts", () => {
    expect(read("caps/comments.ts")).toContain("viaPage: true");
    expect(read("threads.ts")).toMatch(/input\.viaPage \? \{ "x-clax-via": "page" \}/);
  });
});
