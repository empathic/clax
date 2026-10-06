import { afterEach, describe, expect, it, vi } from "vitest";
import { flushSync } from "svelte";
import GallerySites from "./ui/GallerySites.svelte";
import { mount } from "./test/svelte";

const A = "http://localhost:7702", B = "http://localhost:7703", C = "http://localhost:3000";
const at = (o: string, t: string) => ({ origin: o, joined_at: "t", last_used_at: t });
const joined = { site: { key: A, name: B, joined: true, origins: [at(B, "t2"), at(A, "t1")] }, pages: 3, threads: 5 };
const lone = { site: { key: C, name: C, joined: false, origins: [{ origin: C, joined_at: null, last_used_at: null }] }, pages: 1, threads: 1 };

type Call = { url: string; method: string; body?: unknown; auth?: string | null };
function stub(joins: { remaining: number; moved: string[] }[]) {
  const calls: Call[] = [];
  vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
    const auth = new Headers(init?.headers).get("authorization");
    calls.push({ url, method: init?.method ?? "GET", body: init?.body ? JSON.parse(init.body as string) : undefined, auth });
    const ok = (v: unknown) => new Response(JSON.stringify(v), { status: 200 });
    if (url === "/api/token") return ok({ token: "tok" });
    if (url === "/api/live/sites") return ok({ sites: [joined, lone] });
    if (url === "/api/live/sites/join") return ok(joins.shift() ?? { remaining: 0, moved: [] });
    if (url === "/api/live/sites/split") return ok({ split: true });
    return new Response("{}", { status: 404 });
  }));
  return calls;
}
const settle = async () => { for (let i = 0; i < 20; i++) { await new Promise(r => setTimeout(r, 0)); flushSync(); } };
afterEach(() => { vi.unstubAllGlobals(); document.body.innerHTML = ""; });

describe("GallerySites", () => {
  it("has one entry per site, a joined one named after its newest address and listing the others", async () => {
    stub([]);
    const m = mount(GallerySites, {});
    await settle();
    const entries = [...m.root.querySelectorAll(".site")].map(e => e.querySelector("strong")?.textContent);
    expect(entries).toEqual(["localhost:7703", "localhost:3000"]);
    expect(m.root.querySelector(".site .also")?.textContent).toBe("also localhost:7702");
    expect(m.root.textContent).toContain("3 pages · 5 comment threads");
  });

  it("joins a site to another from its menu, a batch at a time, with the token, and splits an address off", async () => {
    const calls = stub([{ remaining: 1, moved: ["t1"] }, { remaining: 0, moved: ["t2"] }]);
    const m = mount(GallerySites, {});
    await settle();
    const select = m.root.querySelector<HTMLSelectElement>(`select[aria-label="Same app as, for localhost:3000"]`)!;
    expect([...select.options].map(o => o.textContent)).toEqual(["Choose a site", "localhost:7703"]);
    select.value = A;
    select.dispatchEvent(new Event("change", { bubbles: true }));
    flushSync();
    const lastMenu = [...m.root.querySelectorAll(".site")][1];
    const join = [...lastMenu.querySelectorAll("button")].find(b => b.textContent === "Join")!;
    expect(join.disabled).toBe(false);
    join.click();
    await settle();
    const joins = calls.filter(c => c.url === "/api/live/sites/join");
    expect(joins.map(c => c.body)).toEqual([{ origin: C, with: A }, { origin: C, with: A }]);
    expect(joins.every(c => c.auth === "Bearer tok")).toBe(true);
    expect(m.root.textContent).toContain("Joined: localhost:3000 and localhost:7703 are one site, 2 threads merged.");
    [...m.root.querySelectorAll("button")].find(b => b.textContent === "Split localhost:7702 off")!.click();
    await settle();
    expect(calls.filter(c => c.url === "/api/live/sites/split").map(c => c.body)).toEqual([{ origin: A }]);
  });

  it("shows nothing when the daemon refuses the listing", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response("{}", { status: 403 })));
    const m = mount(GallerySites, {});
    await settle();
    expect(m.root.querySelector("section")).toBeNull();
  });
});
