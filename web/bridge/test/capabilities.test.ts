import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { CAPABILITY_METHODS } from "../src/capabilities";

/** A contract file (resolved from this file: under jsdom `import.meta.url` is
 * not a file URL) with its comments removed, so parentheses and braces in prose cannot confuse the scan. */
const contract = (file: string) =>
  readFileSync(resolve(__dirname, `../../contract/0.2.61/${file}.d.ts`), "utf8")
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .replace(/^\s*\/\/.*$/gm, "");

/** Function names declared in a `namespace` (`function name(`). */
const functions = (src: string) => [...new Set([...src.matchAll(/^\s*function (\w+)\(/gm)].map(m => m[1]))].sort();

/** Method names of the block that starts at `header` (`name(` at the block's first indent level). */
function members(src: string, header: string): string[] {
  const start = src.indexOf(header);
  expect(start, header).toBeGreaterThanOrEqual(0);
  let depth = 0;
  let i = src.indexOf("{", start);
  const begin = i;
  for (; i < src.length; i++) {
    if (src[i] === "{") depth++;
    else if (src[i] === "}" && --depth === 0) break;
  }
  const body = src.slice(begin + 1, i);
  const out = new Set<string>();
  let level = 0;
  for (const line of body.split("\n")) {
    const m = level === 0 ? line.match(/^\s*(?:readonly\s+)?(\w+)(?:<[^>]*>)?\(/) : null;
    if (m) out.add(m[1]);
    for (const ch of line) { if (ch === "{" || ch === "(") level++; else if (ch === "}" || ch === ")") level--; }
  }
  return [...out].sort();
}

describe("the namespace method lists match the 0.2.61 contract", () => {
  it.each([
    ["permissions", functions(contract("permissions"))],
    ["artifact", functions(contract("artifact"))],
    ["downloads", functions(contract("downloads"))],
    ["user", functions(contract("user"))],
    ["comments", members(contract("comments"), "interface Comments {")],
    ["assets", members(contract("assets"), "interface Assets {")],
    ["db", members(contract("db"), "type DB = {")],
  ] as const)("%s", (name, expected) => {
    expect([...CAPABILITY_METHODS[name]].sort()).toEqual(expected);
  });
});
