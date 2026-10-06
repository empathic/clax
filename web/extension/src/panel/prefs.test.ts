import { describe, expect, it } from "vitest";
import { loadPrefs, savePrefs } from "./prefs";

function area() {
  const data: Record<string, unknown> = {};
  return { data, get: async (k: string) => (k in data ? { [k]: structuredClone(data[k]) } : {}), set: async (v: Record<string, unknown>) => { Object.assign(data, structuredClone(v)); } };
}

describe("site prefs", () => {
  it("keeps the filter and the collapsed groups per site", async () => {
    const a = area();
    expect(await loadPrefs(a, "http://localhost:5173")).toEqual({ filter: "all", collapsed: [] });
    savePrefs(a, "http://localhost:5173", { filter: "open", collapsed: ["/users/:id"] });
    await Promise.resolve();
    expect(await loadPrefs(a, "http://localhost:5173")).toEqual({ filter: "open", collapsed: ["/users/:id"] });
    expect(await loadPrefs(a, "http://localhost:3000")).toEqual({ filter: "all", collapsed: [] });
  });

  it("reads anything else, or storage that fails, as the defaults", async () => {
    const a = area();
    a.data["site-prefs:http://x"] = { filter: "deleted", collapsed: [1, "/a", null] };
    expect(await loadPrefs(a, "http://x")).toEqual({ filter: "all", collapsed: ["/a"] });
    const broken = { get: async () => { throw new Error("no"); }, set: async () => { throw new Error("no"); } };
    expect(await loadPrefs(broken, "http://x")).toEqual({ filter: "all", collapsed: [] });
    expect(() => savePrefs(broken, "http://x", { filter: "open", collapsed: [] })).not.toThrow();
    expect(await loadPrefs(undefined, "http://x")).toEqual({ filter: "all", collapsed: [] });
  });
});
