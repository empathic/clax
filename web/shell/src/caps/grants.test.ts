import { describe, expect, it, vi } from "vitest";
import type { Declared } from "./availability";
import { Grants, grantsKey, type Prompt, type PromptAnswer } from "./grants";

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

  it("grants a declared room with no consent and lists it", () => {
    const g = new Grants(grantsKey("7q3k9mzx2b4t", "u_00000000000000000000aa"), new MemoryStorage() as unknown as Storage, { room: {} }, false, vi.fn(async () => "deny" as const));
    expect(g.state("room")).toBe("granted");
    expect(g.all()).toMatchObject({ room: "granted" });
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

  it("names each prompt's kind and offers an always choice, since an Allow is kept", async () => {
    const asked = async (names: string[], d: Declared = declared, served = {}) => {
      const ask = vi.fn(async (_p: Prompt) => "deny" as const);
      await new Grants("k", new MemoryStorage() as unknown as Storage, d, true, ask, served).request(names);
      return ask.mock.calls[0][0];
    };
    expect(await asked(["comments"])).toMatchObject({ kind: "comments", always: "Always for this app" });
    expect(await asked(["comments", "sample"], { comments: {}, sample: {} }, { sample: true })).toMatchObject({ kind: "capability", always: "Always for this app" });
    // The daemon keeps sample's grant once caps/sample.ts saves the Allow.
    expect(await asked(["sample"], { sample: {} }, { sample: true })).toMatchObject({ kind: "sample", always: "Always for this app" });
  });

  it("works when storage throws", async () => {
    const broken = { getItem() { throw new Error("blocked"); }, setItem() { throw new Error("blocked"); } };
    const g = new Grants("k", broken as unknown as Storage, declared, true, async () => "allow");
    await g.request(["comments"]);
    expect(g.state("comments")).toBe("granted");
  });

  it("sample: the owner's browser only, when served; asked in its own dialog; held here for this view only (the daemon keeps it)", async () => {
    const storage = new MemoryStorage();
    const ask = vi.fn(async (_p: Prompt) => "allow" as const);
    const key = grantsKey("7q3k9mzx2b4t", "u_00000000000000000000aa");
    const g = new Grants(key, storage as unknown as Storage, { sample: {} }, true, ask, { sample: true });
    expect(g.state("sample")).toBe("prompt");
    await g.request(["sample"]);
    expect(ask.mock.calls[0][0]).toMatchObject({ title: "Let this app ask Claude?", body: expect.stringContaining("Anthropic API key") });
    expect(g.state("sample")).toBe("granted");
    // A new view starts at prompt: the allow was never stored here.
    const fresh = new Grants(key, storage as unknown as Storage, { sample: {} }, true, ask, { sample: true });
    expect(fresh.state("sample")).toBe("prompt");
    fresh.allow("sample");
    expect(fresh.state("sample")).toBe("granted");
    fresh.forget("sample");
    expect(fresh.state("sample")).toBe("prompt");
    expect(storage.getItem(key) ?? "[]").not.toContain("sample");
    expect(new Grants(key, storage as unknown as Storage, { sample: {} }, true, ask).state("sample")).toBe("unavailable");
    expect(new Grants(key, storage as unknown as Storage, { sample: {} }, false, ask, { sample: true }).state("sample")).toBe("unavailable");
  });

  it("revoke_clears_a_stored_grant", async () => {
    const { g, storage } = make("allow");
    await g.request(["comments"]);
    expect(g.state("comments")).toBe("granted");
    g.revoke("comments");
    expect(g.state("comments")).toBe("prompt");
    expect(JSON.parse(storage.getItem(grantsKey("7q3k9mzx2b4t", "u_00000000000000000000aa"))!)).toEqual([]);
    expect(make("deny", storage).g.state("comments")).toBe("prompt");
  });
});
