import { describe, expect, it, vi } from "vitest";
import { Grants, grantsKey, type PromptAnswer } from "./grants";

class MemoryStorage {
  data = new Map<string, string>();
  getItem(k: string) { return this.data.get(k) ?? null; }
  setItem(k: string, v: string) { this.data.set(k, v); }
}

const declared = { comments: {}, db: {}, downloads: {}, assets: {} };
const make = (answer: PromptAnswer, storage = new MemoryStorage(), owner = true) => {
  const ask = vi.fn(async () => answer);
  return { g: new Grants(grantsKey("7q3k9mzx2b4t", "u_00000000000000000000aa"), storage as unknown as Storage, declared, owner, ask), ask, storage };
};

describe("Grants", () => {
  it("reports states by declaration, owner, and consent", () => {
    const { g } = make("allow");
    expect(g.state("comments")).toBe("prompt");
    expect(g.state("db")).toBe("granted");
    expect(g.state("downloads")).toBe("granted");
    expect(g.state("user")).toBe("granted");
    expect(g.state("room")).toBe("unavailable");
    expect(g.state("permissions")).toBe("unavailable");
    expect(g.state("comments:thread")).toBe("unavailable");
    expect(g.all()).toEqual({ comments: "prompt", db: "granted", downloads: "granted", user: "granted", assets: "granted" });
    expect(make("allow", new MemoryStorage(), false).g.state("assets")).toBe("unavailable");
  });

  it("asks once, batches names, and persists a grant per viewer and artifact", async () => {
    const { g, ask, storage } = make("allow");
    await Promise.all([g.request(["comments", "db"]), g.request(["comments"])]);
    expect(ask).toHaveBeenCalledTimes(1);
    expect(g.state("comments")).toBe("granted");
    expect(JSON.parse(storage.getItem(grantsKey("7q3k9mzx2b4t", "u_00000000000000000000aa"))!)).toEqual(["comments"]);
    const again = make("deny", storage);
    expect(again.g.state("comments")).toBe("granted");
    expect(make("deny", storage).g.state("comments")).toBe("granted");
  });

  it("keeps a denial or a dismissal final for the page load, without asking again", async () => {
    const denied = make("deny");
    await denied.g.request(["comments"]);
    await denied.g.request(["comments"]);
    expect(denied.ask).toHaveBeenCalledTimes(1);
    expect(denied.g.state("comments")).toBe("denied");
    expect(denied.g.refusal("comments")).toBe("forbidden");
    const dismissed = make("dismiss");
    await dismissed.g.request(["comments"]);
    expect(dismissed.g.state("comments")).toBe("denied");
    expect(dismissed.g.refusal("comments")).toBe("consent_required");
    expect(denied.storage.data.size).toBe(0);
  });

  it("works when storage throws", async () => {
    const broken = { getItem() { throw new Error("blocked"); }, setItem() { throw new Error("blocked"); } };
    const g = new Grants("k", broken as unknown as Storage, declared, true, async () => "allow");
    await g.request(["comments"]);
    expect(g.state("comments")).toBe("granted");
  });
});
