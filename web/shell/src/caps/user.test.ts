import { afterEach, describe, expect, it, vi } from "vitest";
import type { CapEnv } from "./host";
import { avatarFor, colorFor, rootWrite, userHandler } from "./user";

const ME = "u_00000000000000000000aa";
const OTHER = "u_00000000000000000000bb";

function env(token: string | null, name: string | null, declared: Record<string, unknown> = { user: { scopes: ["profile"] } }) {
  return { aid: "7q3k9mzx2b4t", token, declared, viewer: async () => ({ publicId: ME, name }) } as unknown as CapEnv;
}

describe("user in the shell", () => {
  afterEach(() => { vi.unstubAllGlobals(); });

  it("answers identity and ownership from the view", async () => {
    const owner = userHandler(env("tok", "Alex"), null as never);
    expect(await owner.call("isOwner", [])).toBe(true);
    expect(await owner.call("canEdit", [])).toBe(true);
    expect(await owner.call("id", [])).toBe(ME);
    expect(await owner.call("name", [])).toBe("Alex");
    expect(await owner.call("email", [])).toBeNull();
    const me = (await owner.call("me", [])) as Record<string, unknown>;
    expect(me).toMatchObject({ id: ME, name: "Alex", email: null, isOwner: true, canEdit: true, color: colorFor(ME) });
    expect(String(me.avatarUrl)).toMatch(/^data:image\/svg\+xml/);
    const lan = userHandler(env(null, null), null as never);
    expect([await lan.call("isOwner", []), await lan.call("canEdit", []), await lan.call("name", [])]).toEqual([false, false, ""]);
    const noScope = userHandler(env("tok", "Alex", { user: {} }), null as never);
    expect(await noScope.call("name", [])).toBe("");
    expect(((await noScope.call("me", [])) as { name: string }).name).toBe("");
  });

  it("without the declaration, the universal members answer and the rest resolve the all-absent values", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => { throw new Error("no lookups without the declaration"); }));
    const h = userHandler(env("tok", "Alex", {}), null as never);
    expect([await h.call("isOwner", []), await h.call("canEdit", []), await h.call("can", ["files.write"])]).toEqual([true, true, true]);
    expect(await h.call("me", [])).toMatchObject({ id: null, name: "", email: null, isOwner: true, canEdit: true, color: colorFor(null) });
    expect([await h.call("id", []), await h.call("avatarUrl", []), await h.call("name", [])]).toEqual([null, null, ""]);
    expect(await h.call("profiles", [[OTHER]])).toMatchObject({ [OTHER]: { id: OTHER, name: "", isMe: false, guest: false } });
    expect(await h.call("search", ["sa"])).toEqual([]);
  });

  it("can() follows the level and the declared root write rule", async () => {
    const rules = { user: {}, db: { rules: [{ path: "", write: "admin" }] } };
    expect(await userHandler(env("tok", null, rules), null as never).call("can", ["data.write"])).toBe(true);
    expect(await userHandler(env(null, "Sam", rules), null as never).call("can", ["data.write"])).toBe(false);
    expect(await userHandler(env(null, "Sam", { user: {} }), null as never).call("can", ["data.write"])).toBe(true);
    expect(await userHandler(env(null, null, { user: {} }), null as never).call("can", ["data.write"])).toBe(false);
    expect(await userHandler(env(null, "Sam"), null as never).call("can", ["files.write"])).toBe(false);
    expect(await userHandler(env("tok", null), null as never).call("can", ["assets.write"])).toBe(true);
    expect(await userHandler(env("tok", null), null as never).call("can", ["launch.rockets"])).toBe(false);
    expect(rootWrite({})).toBe("interact");
  });

  it("profiles resolves every ID it is given, batched, unknown ones unresolved", async () => {
    const urls: string[] = [];
    vi.stubGlobal("fetch", vi.fn(async (url: string) => {
      urls.push(url);
      return new Response(JSON.stringify({ viewers: [{ id: OTHER, display_name: "Sam" }] }));
    }));
    const h = userHandler(env("tok", "Alex"), null as never);
    const p = (await h.call("profiles", [[OTHER, "junk", OTHER]])) as Record<string, Record<string, unknown>>;
    expect(Object.keys(p).sort()).toEqual(["junk", OTHER]);
    expect(p[OTHER]).toMatchObject({ id: OTHER, name: "Sam", isMe: false, guest: false, email: null });
    expect(p.junk).toMatchObject({ name: "", guest: false });
    expect(urls).toEqual([`/api/viewers?ids=${OTHER}`]);
    await h.call("profiles", [OTHER]);
    expect(urls).toHaveLength(1);
    expect(avatarFor("", colorFor(null))).toContain(encodeURIComponent("?"));
  });

  it("search is for the owner shell only and never rejects", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ viewers: [{ id: OTHER, display_name: "Sam" }] }))));
    expect(await userHandler(env(null, "Alex"), null as never).call("search", ["sa"])).toEqual([]);
    const hits = (await userHandler(env("tok", "Alex"), null as never).call("search", ["sa"])) as { id: string; name: string }[];
    expect(hits.map(h => [h.id, h.name])).toEqual([[OTHER, "Sam"]]);
    const seeded = (await userHandler(env("tok", "Alex"), null as never).call("search", [""])) as { id: string }[];
    expect(seeded[0].id).toBe(ME);
    vi.stubGlobal("fetch", vi.fn(async () => { throw new Error("down"); }));
    expect(await userHandler(env("tok", "Alex"), null as never).call("search", ["x"])).toEqual([]);
  });

  it("hostile or oversized input makes no request and still resolves", async () => {
    const fetchSpy = vi.fn(async () => new Response(JSON.stringify({ viewers: [] })));
    vi.stubGlobal("fetch", fetchSpy);
    const h = userHandler(env("tok", "Alex"), null as never);
    const hostile = ["u_00000000000000000000bb,u_00000000000000000000cc", "../../api/artifacts", "u_0000000000000000000000&q=a", `${OTHER}#`, OTHER.toUpperCase()];
    const p = (await h.call("profiles", [hostile])) as Record<string, { name: string }>;
    expect(Object.keys(p).sort()).toEqual([...hostile].sort());
    expect(Object.values(p).every(x => x.name === "")).toBe(true);
    expect(await h.call("profiles", [{ length: 3 }])).toEqual({});
    expect(await h.call("profiles", [[1, null, OTHER]])).toHaveProperty(OTHER);
    expect(fetchSpy).toHaveBeenCalledTimes(1);
    fetchSpy.mockClear();
    expect(await h.call("search", ["x".repeat(500)])).toEqual([]);
    expect(await h.call("search", [{ toString: () => "a" }])).toEqual([]);
    expect(await userHandler(env("tok", "Alex", { user: {} }), null as never).call("search", ["sa"])).toEqual([]);
    expect(fetchSpy).not.toHaveBeenCalled();
    await h.call("search", ["a&ids=b"]);
    expect(fetchSpy.mock.calls[0]).toEqual(["/api/viewers?q=a%26ids%3Db", expect.anything()]);
  });

  it("an absurd list resolves every entry and looks up a bounded number", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const fetchSpy = vi.fn(async () => new Response(JSON.stringify({ viewers: [] })));
    vi.stubGlobal("fetch", fetchSpy);
    const ids = Array.from({ length: 5000 }, (_, i) => `u_${i.toString(16).padStart(22, "0")}`);
    const p = (await userHandler(env("tok", "Alex"), null as never).call("profiles", [ids])) as Record<string, unknown>;
    expect(Object.keys(p)).toHaveLength(5000);
    expect(fetchSpy.mock.calls.length).toBeLessThanOrEqual(16);
    expect(warn).toHaveBeenCalledTimes(1);
    warn.mockRestore();
  });

  it("coalesces concurrent lookups of the same ID", async () => {
    const fetchSpy = vi.fn(async () => new Response(JSON.stringify({ viewers: [{ id: OTHER, display_name: "Sam" }] })));
    vi.stubGlobal("fetch", fetchSpy);
    const h = userHandler(env("tok", "Alex"), null as never);
    const [a, b] = (await Promise.all([h.call("profiles", [[OTHER]]), h.call("profiles", [[OTHER]])])) as Record<string, { name: string }>[];
    expect([a[OTHER].name, b[OTHER].name]).toEqual(["Sam", "Sam"]);
    expect(fetchSpy).toHaveBeenCalledTimes(1);
  });

  it("a failed viewer lookup reads as no identity, never a rejection", async () => {
    const e = { ...env("tok", "Alex"), viewer: async () => { throw new Error("down"); } } as unknown as CapEnv;
    const h = userHandler(e, null as never);
    expect(await h.call("me", [])).toMatchObject({ id: null, name: "", isOwner: true });
    expect([await h.call("id", []), await h.call("name", []), await h.call("can", ["data.write"])]).toEqual([null, "", true]);
    expect(await h.call("search", [""])).toEqual([]);
  });

  it("reset forgets resolved names, and a disposed handler fetches nothing", async () => {
    const fetchSpy = vi.fn(async () => new Response(JSON.stringify({ viewers: [{ id: OTHER, display_name: "Sam" }] })));
    vi.stubGlobal("fetch", fetchSpy);
    const h = userHandler(env("tok", "Alex"), null as never);
    await h.call("profiles", [[OTHER]]);
    expect(((await h.call("search", [""])) as unknown[]).length).toBe(2);
    h.reset?.();
    expect(((await h.call("search", [""])) as unknown[]).length).toBe(1);
    h.dispose?.();
    fetchSpy.mockClear();
    expect(await h.call("profiles", [[OTHER]])).toMatchObject({ [OTHER]: { name: "" } });
    expect(await h.call("search", ["sa"])).toEqual([]);
    expect(fetchSpy).not.toHaveBeenCalled();
  });

  it("avatars escape and color every ID the same way", () => {
    const svg = decodeURIComponent(avatarFor("<b> &x", "#123456").replace(/^data:image\/svg\+xml;utf8,/, ""));
    expect(svg).not.toContain("<b>");
    expect(svg).toContain("&#60;&#38;");
    expect(colorFor(OTHER)).toBe(colorFor(OTHER));
    expect(colorFor(null)).toMatch(/^#[0-9a-f]{6}$/);
  });
});
