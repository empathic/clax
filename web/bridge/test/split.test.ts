// The eager bridge must not pull in what the lazy parts carry, and a part must
// not import state the eager bridge owns (a part is a separate build).
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const SRC = resolve(dirname(fileURLToPath(import.meta.url)), "../src");
const ALIAS: Record<string, string> = { "clax-bridge-parts": resolve(SRC, "parts-url.ts") };

/** Modules `file` imports at run time (type-only imports excluded), resolved to files or bare names. */
function runtimeImports(file: string): string[] {
  const text = readFileSync(file, "utf8");
  const out: string[] = [];
  const re = /^\s*(?:import|export)\s+(?!type\b)(?:[^"';]*?\sfrom\s+)?"([^"]+)"/gm;
  for (const m of text.matchAll(re)) {
    const spec = m[1];
    if (ALIAS[spec]) out.push(ALIAS[spec]);
    else if (spec.startsWith(".")) out.push(resolve(dirname(file), spec.endsWith(".ts") ? spec : `${spec}.ts`));
    else out.push(spec);
  }
  return out;
}

function closure(entry: string): Set<string> {
  const seen = new Set<string>();
  const walk = (f: string) => {
    if (seen.has(f)) return;
    seen.add(f);
    if (f.endsWith(".ts")) for (const g of runtimeImports(f)) walk(g);
  };
  walk(entry);
  return seen;
}

const rel = (f: string) => f.startsWith(SRC) ? f.slice(SRC.length + 1) : f;

describe("the bridge split", () => {
  it("keeps comment mode, anchoring, areas, clips and capability members out of the eager bridge", () => {
    const eager = [...closure(resolve(SRC, "bridge.ts"))].map(rel);
    const lazy = ["anchor.ts", "area.ts", "clip.ts", "comment-mode.ts", "target.ts", "text-walk.ts", "sha256.ts", "modern-screenshot"];
    expect(eager.filter(f => lazy.includes(f) || f.startsWith("caps/") || (f.startsWith("parts/") && f !== "parts/types.ts"))).toEqual([]);
  });
  it("lets no part import the eager bridge's state", () => {
    for (const part of ["comment", "clip", "caps"]) {
      const files = [...closure(resolve(SRC, `parts/${part}.ts`))].map(rel);
      expect(files, part).not.toContain("comments-context.ts");
      expect(files, part).not.toContain("bridge.ts");
    }
  });
});
