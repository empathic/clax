import { afterEach, describe, expect, it, vi } from "vitest";
import { freshModules } from "../test/modules";

describe("NameSaver", () => {
  afterEach(async () => { vi.unstubAllGlobals(); await freshModules(); });
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
  /** A daemon whose lookup answers an unnamed visitor at once and whose
   * name saves wait for `answer`: on a hosted one, a save sent before the
   * first answers (no guest cookie yet) is refused 409 `name_taken`. */
  function hosted() {
    const puts: string[] = [];
    const answers: (() => void)[] = [];
    vi.stubGlobal("fetch", vi.fn((url: string, init?: RequestInit) => {
      if (url === "/api/token") return Promise.resolve(new Response("{}", { status: 403 }));
      const viewer = (name: string | null) => new Response(JSON.stringify({ viewer: { public_id: "u_1", display_name: name, created_at: "x" } }));
      if (!init?.method) return Promise.resolve(viewer(null));
      const name = JSON.parse(String(init.body)).display_name as string;
      const taken = answers.length > 0;
      puts.push(name);
      return new Promise<Response>(r => answers.push(() => r(taken
        ? new Response(JSON.stringify({ error: { code: "name_taken", message: `the name '${name}' is taken` } }), { status: 409 })
        : viewer(name))));
    }));
    let notice: string | null = null;
    const setNotice = (u: string | null | ((prev: string | null) => string | null)) => { notice = typeof u === "function" ? u(notice) : u; };
    return { puts, setNotice, notice: () => notice, answer: () => answers.shift()!() };
  }
  const settle = () => new Promise(r => setTimeout(r, 0));

  it("a save of the name being saved sends no second request and shares its outcome", async () => {
    const d = hosted();
    const { NameSaver } = await import("./viewer-name-model");
    const viewers: string[] = [];
    const s = new NameSaver(d.setNotice, v => viewers.push(v.display_name ?? ""));
    s.load(() => {});
    s.edit();
    s.save("Mia");
    await settle();
    s.save("Mia ");
    await settle();
    expect(d.puts).toEqual(["Mia"]);
    d.answer();
    await vi.waitFor(() => expect(viewers).toEqual(["", "Mia"]));
    await settle();
    expect(d.puts).toEqual(["Mia"]);
    expect(d.notice()).toBeNull();
  });

  it("a different name typed while one is being saved is sent after it answers, and is the one kept", async () => {
    const d = hosted();
    const { NameSaver } = await import("./viewer-name-model");
    const viewers: string[] = [];
    const s = new NameSaver(d.setNotice, v => viewers.push(v.display_name ?? ""));
    s.load(() => {});
    s.edit();
    s.save("Mia");
    await settle();
    s.save("Mira");
    await settle();
    expect(d.puts).toEqual(["Mia"]);
    d.answer();
    await vi.waitFor(() => expect(d.puts).toEqual(["Mia", "Mira"]));
    d.answer();
    await vi.waitFor(() => expect(viewers).toEqual(["", "Mia", "Mira"]));
    expect(d.notice()).toBeNull();
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
