import { afterEach, describe, expect, it, vi } from "vitest";

describe("NameSaver", () => {
  afterEach(() => { vi.unstubAllGlobals(); vi.resetModules(); });
  it("saves only after the initial lookup answered, skips an unchanged name, and keeps a name the viewer typed first", async () => {
    const calls: string[] = [];
    let answer!: (r: Response) => void;
    vi.stubGlobal("fetch", vi.fn((url: string, init?: RequestInit) => {
      if (url === "/api/token") return Promise.resolve(new Response("{}", { status: 403 }));
      calls.push(`${init?.method ?? "GET"} ${url}`);
      if (!init?.method) return new Promise<Response>(r => { answer = r; });
      return Promise.resolve(new Response(JSON.stringify({ viewer: { public_id: "u_1", display_name: "Ada", created_at: "x" } })));
    }));
    const { NameSaver } = await import("./viewer-name-model");
    const shown: string[] = [];
    const s = new NameSaver(() => {});
    s.load(n => shown.push(n));
    s.edit();
    s.save("Ada");
    await new Promise(r => setTimeout(r, 10));
    expect(calls).toEqual(["GET /api/viewers/me"]);
    answer(new Response(JSON.stringify({ viewer: { public_id: "u_1", display_name: "Bo", created_at: "x" } })));
    await new Promise(r => setTimeout(r, 10));
    expect(shown).toEqual([]);
    expect(calls).toEqual(["GET /api/viewers/me", "PUT /api/viewers/me"]);
    s.save("Ada");
    await new Promise(r => setTimeout(r, 10));
    expect(calls).toHaveLength(2);
  });
  it("shows the stored name when the viewer did not type, and reports the viewer after a load and after a save", async () => {
    const viewer = (name: string) => ({ public_id: "u_1", display_name: name, created_at: "x" });
    vi.stubGlobal("fetch", vi.fn((url: string, init?: RequestInit) =>
      Promise.resolve(url === "/api/token" ? new Response("{}", { status: 403 }) : new Response(JSON.stringify({ viewer: viewer(init?.method ? "Cy" : "Bo") })))));
    const { NameSaver } = await import("./viewer-name-model");
    const shown: string[] = [];
    const viewers: string[] = [];
    const s = new NameSaver(() => {}, v => viewers.push(v.display_name ?? ""));
    s.load(n => shown.push(n));
    await new Promise(r => setTimeout(r, 10));
    expect(shown).toEqual(["Bo"]);
    expect(viewers).toEqual(["Bo"]);
    s.save(" Cy ");
    await new Promise(r => setTimeout(r, 10));
    expect(viewers).toEqual(["Bo", "Cy"]);
  });
});
