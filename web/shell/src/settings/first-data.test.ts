import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { tick } from "svelte";
import { settle } from "../people/fixtures";
import { WEEK_END, WEEK_START, adminPerson, meBot } from "../bot/fixtures";
import { mount } from "../test/svelte";
import { Reveal } from "./first-data";
import type { Ctx } from "./screens";

// The browser's context is given, not asked: each test names who it is.
let who: Ctx = { hosted: false, owner: true, member: true, admin: true, bot: true };
vi.mock("./context", () => ({ context: () => Promise.resolve(who) }));
// The Audit log's chunk and the Usage tab's chunk do not load.
vi.mock("../bot/AuditPage.svelte", () => { throw new Error("chunk"); });
vi.mock("../bot/AdminUsage.svelte", () => { throw new Error("chunk"); });
const { default: Settings } = await import("./Settings.svelte");
const { forget } = await import("../bot/me");

/** A gate a fetch waits on until the test opens it. */
function gate() {
  let open!: () => void;
  const shut = new Promise<void>(r => { open = r; });
  return { shut, open };
}
/** Answers `GET <path>` (any query) with its route's body, once its gate (if any) opens; else 404. */
function stubFetch(routes: Record<string, unknown>, gates: Record<string, Promise<void>> = {}): string[] {
  const asked: string[] = [];
  vi.stubGlobal("fetch", vi.fn(async (url: string) => {
    const path = url.split("?")[0];
    asked.push(path);
    await gates[path];
    const r = routes[path];
    if (r === undefined) return new Response(JSON.stringify({ error: { code: "not_found", message: "not found" } }), { status: 404 });
    return new Response(JSON.stringify(r));
  }));
  return asked;
}

beforeEach(() => { who = { hosted: false, owner: true, member: true, admin: true, bot: true }; forget(); });
afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); history.replaceState(null, "", "/"); });

describe("Reveal", () => {
  it("reveals once after it is armed, with nothing holding it", async () => {
    const ready = vi.fn();
    const r = new Reveal(ready);
    await tick();
    expect(ready).not.toHaveBeenCalled();
    r.arm();
    await tick();
    expect(ready).toHaveBeenCalledTimes(1);
    r.arm();
    r.hold()();
    await tick();
    expect(ready).toHaveBeenCalledTimes(1);
  });

  it("waits for every hold, and a second release of one changes nothing", async () => {
    const ready = vi.fn();
    const r = new Reveal(ready);
    const a = r.hold();
    const b = r.hold();
    r.arm();
    await tick();
    a();
    a();
    await tick();
    expect(ready).not.toHaveBeenCalled();
    b();
    await tick();
    expect(ready).toHaveBeenCalledTimes(1);
  });

  it("counts a hold made in the same flush as the release that caused it", async () => {
    const ready = vi.fn();
    const r = new Reveal(ready);
    const a = r.hold();
    r.arm();
    a();
    // A part the release mounts holds before the check after the DOM settles.
    const b = r.hold();
    await tick();
    expect(ready).not.toHaveBeenCalled();
    b();
    await tick();
    expect(ready).toHaveBeenCalledTimes(1);
  });
});

describe("Settings' reveal", () => {
  it("waits for Your ClaxBot's fetches, the week's use the last", async () => {
    history.replaceState(null, "", "/settings/claxbot");
    const use = gate();
    stubFetch({ "/api/me/bot": meBot(), "/api/me/usage": { week: { start: WEEK_START, end: WEEK_END }, runs: [], totals: [] } }, { "/api/me/usage": use.shut });
    const onready = vi.fn();
    const m = mount(Settings, { onready });
    await vi.waitFor(() => expect(m.root.querySelector("#me-w")).not.toBeNull());
    await settle();
    expect(onready).not.toHaveBeenCalled();
    use.open();
    await vi.waitFor(() => expect(onready).toHaveBeenCalledTimes(1));
  });

  it("waits for an admin tab's own first data, its body mounted once the role is known", async () => {
    history.replaceState(null, "", "/settings/roles");
    const role = gate();
    const list = gate();
    const asked = stubFetch({ "/api/me/bot": meBot({ role: "admin" }), "/api/admin/people": [adminPerson()] }, { "/api/me/bot": role.shut, "/api/admin/people": list.shut });
    const onready = vi.fn();
    const m = mount(Settings, { onready });
    await settle();
    expect(onready).not.toHaveBeenCalled();
    role.open();
    // The tab's body, mounted, asks for its list.
    await vi.waitFor(() => expect(asked).toContain("/api/admin/people"));
    await settle();
    expect(onready).not.toHaveBeenCalled();
    list.open();
    await vi.waitFor(() => expect(onready).toHaveBeenCalledTimes(1));
    expect(m.root.textContent).toContain("Sam");
  });

  it("is revealed when a screen's fetch fails, with what it says", async () => {
    history.replaceState(null, "", "/settings/remotes");
    stubFetch({});
    const onready = vi.fn();
    const m = mount(Settings, { onready });
    await vi.waitFor(() => expect(onready).toHaveBeenCalledTimes(1));
    expect(m.root.querySelector(".rmutes [role=alert]")).not.toBeNull();
  });

  it("is revealed, saying so, when the screen's chunk does not load", async () => {
    history.replaceState(null, "", "/settings/audit");
    stubFetch({});
    const onready = vi.fn();
    const m = mount(Settings, { onready });
    await vi.waitFor(() => expect(onready).toHaveBeenCalledTimes(1));
    expect(m.root.querySelector(".stg-body [role=alert]")!.textContent).toContain("This screen could not load.");
  });

  it("is revealed, saying so, when an admin tab's chunk does not load", async () => {
    history.replaceState(null, "", "/settings/usage");
    stubFetch({ "/api/me/bot": meBot({ role: "admin" }) });
    const onready = vi.fn();
    const m = mount(Settings, { onready });
    await vi.waitFor(() => expect(onready).toHaveBeenCalledTimes(1));
    expect(m.root.querySelector(".bpg [role=alert]")!.textContent).toBe("This screen could not load. Reload to try again.");
    expect(m.root.querySelector(".bpg [role=status]")).toBeNull();
  });
});
