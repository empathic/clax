import { afterEach, describe, expect, it, vi } from "vitest";
import PeoplePanel from "./ui/PeoplePanel.svelte";
import { flush, mount } from "./test/svelte";

const resolvedThread = { id: "t9", status: "resolved", comments: [] };
function view(askName: string | null, status = "resolved") {
  return {
    data: { artifact: { participants: { people: [], agents: [] } }, versions: [] }, agents: [], presence: [], working: [], attention: null,
    threads: [{ ...resolvedThread, status }], me: { public_id: "u_1", display_name: null, created_at: "x" }, shareWhere: false, commenting: false, askName,
  } as never;
}

describe("PeoplePanel asking for a name to reopen", () => {
  afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); });

  it("asks in the menu, not as a failure, with focus in the name field; closed with no name, it forgets the reopen and focus returns to the thread's card", () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ viewer: { public_id: "u_1", display_name: null, created_at: "x" } }))));
    const card = document.createElement("article");
    card.className = "thread-card";
    card.dataset.thread = "t9";
    card.innerHTML = `<button class="card-head">«Our team»</button>`;
    document.body.append(card);
    const ctl = { numbers: () => new Map(), forgetReopen: vi.fn(), setNotice: vi.fn(), setMe: vi.fn(), setShareWhere: vi.fn() };
    const onClose = vi.fn();
    const v = mount(PeoplePanel, { ctl: ctl as never, s: view("t9"), onClose });
    expect(v.root.querySelector(".ask")!.textContent).toBe("Add your name to reopen the thread: people see it beside what you do.");
    expect(v.root.querySelector(".ask")!.getAttribute("role")).toBe("status");
    expect(document.activeElement).toBe(v.root.querySelector("input.viewer-name"));
    v.root.querySelector<HTMLElement>(".people")!.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(ctl.forgetReopen).toHaveBeenCalledOnce();
    expect(onClose).toHaveBeenCalledOnce();
    expect(document.activeElement).toBe(card.querySelector(".card-head"));
    v.unmount();
  });

  it("closes itself once the thread is open again, and opened from the roster asks nothing", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ viewer: { public_id: "u_1", display_name: "Mia", created_at: "x" } }))));
    const ctl = { numbers: () => new Map(), forgetReopen: vi.fn(), setNotice: vi.fn(), setMe: vi.fn(), setShareWhere: vi.fn() };
    const onClose = vi.fn();
    const v = mount(PeoplePanel, { ctl: ctl as never, s: view("t9"), onClose });
    expect(onClose).not.toHaveBeenCalled();
    v.update({ ctl: ctl as never, s: view("t9", "open"), onClose });
    flush();
    expect(onClose).toHaveBeenCalledOnce();
    expect(ctl.forgetReopen).not.toHaveBeenCalled();
    v.unmount();
    const plain = mount(PeoplePanel, { ctl: ctl as never, s: view(null), onClose });
    expect(plain.root.querySelector(".ask")).toBeNull();
    expect(document.activeElement).toBe(plain.root.querySelector(".people"));
    plain.unmount();
  });
});

describe("PeoplePanel's agents", () => {
  afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); document.body.replaceChildren(); });
  // A few turns of the event loop: the stubbed fetch's Response body is read
  // through more microtasks on some Node versions (22) than on others (26).
  const settle = async () => { for (let i = 0; i < 3; i++) await new Promise(r => setTimeout(r, 0)); flush(); };
  const ALEX = { public_id: "u_1", name: "Alex", owner: true };
  const SAM = { public_id: "u_sam", name: "Sam", owner: false };

  it("names each agent, with its person when it is another's, and its state and block time as the roster has them", async () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(new Date("2026-10-09T10:30:00.000Z"));
    const roster = { agents: [
      { handle: "a_cl", name: "clax-claude", person: null, live: true, state: "waiting", since: "2026-10-09T10:18:00.000Z", blocked: { since: "2026-10-09T10:18:00.000Z", questions: 1, approvals: 0 } },
      { handle: "a_sam", name: "sam-web", person: "u_sam", live: true, state: "waiting", since: "2026-10-09T10:20:00.000Z", blocked: { since: "2026-10-09T10:20:00.000Z", questions: 0, approvals: 1 } },
      // Idle, with a routing request for the owner: it does not wait, and says what waits on you.
      { handle: "a_mp", name: "quarterly-fix-chart", person: null, live: true, state: "idle", since: "2026-10-09T09:00:00.000Z", blocked: { since: "2026-10-09T09:34:00.000Z", questions: 0, approvals: 1 } },
    ] };
    const calls: string[] = [];
    vi.stubGlobal("fetch", vi.fn(async (url: string) => { calls.push(url); return new Response(JSON.stringify(url === "/api/agents" ? roster : { viewer: { public_id: "u_1", display_name: "Alex", created_at: "x" } })); }));
    const s = view(null) as unknown as Record<string, unknown>;
    const agents = [
      { handle: "a_cl", harness: "claude", live: true, name: "clax-claude", person: ALEX },
      { handle: "a_sam", harness: "codex", live: true, name: "sam-web", person: SAM },
      { handle: "a_mp", harness: "claude", live: true, name: "quarterly-fix-chart", person: ALEX },
    ];
    const ctl = { numbers: () => new Map(), forgetReopen: vi.fn(), setNotice: vi.fn(), setMe: vi.fn(), setShareWhere: vi.fn() };
    const v = mount(PeoplePanel, { ctl: ctl as never, s: { ...s, agents, data: { artifact: { participants: { people: [], agents } }, versions: [] } } as never, onClose: vi.fn() });
    await settle();
    expect(calls).toContain("/api/agents");
    const rows = [...v.root.querySelectorAll(".prow.agent")];
    expect(rows.map(r => r.querySelector("b")!.textContent)).toEqual(["clax-claude", "sam-web · Sam", "quarterly-fix-chart"]);
    expect(rows[0].querySelector("p")!.textContent).toBe("Waiting on you 12 min (1 question)");
    // The owner sees Sam's agent's block in full: it waits on Sam, not on the owner.
    expect(rows[1].querySelector("p")!.textContent).toBe("Waiting on Sam 10 min (1 approval)");
    expect(rows[2].querySelector("p")!.textContent).toBe("1 approval for you · 56 min");
    v.unmount();
  });

  it("lists an agent whose session ended under Earlier, as Ended, never as Idle", async () => {
    vi.stubGlobal("fetch", vi.fn(async (url: string) => new Response(JSON.stringify(url === "/api/agents" ? { agents: [] } : { viewer: { public_id: "u_1", display_name: "Alex", created_at: "x" } }))));
    const s = view(null) as unknown as Record<string, unknown>;
    const agents = [
      { handle: "a_cl", harness: "claude", live: true, name: "clax-claude", person: ALEX },
      { handle: "a_sam", harness: "claude", live: false, name: "sam-helper", person: SAM },
    ];
    const versions = [{ n: 2, agent: "a_sam", addresses: ["t1"] }];
    const ctl = { numbers: () => new Map([["t1", 1]]), forgetReopen: vi.fn(), setNotice: vi.fn(), setMe: vi.fn(), setShareWhere: vi.fn() };
    const v = mount(PeoplePanel, { ctl: ctl as never, s: { ...s, agents, data: { artifact: { participants: { people: [], agents } }, versions } } as never, onClose: vi.fn() });
    await settle();
    expect([...v.root.querySelectorAll(".people h3")].map(h => h.textContent)).toEqual(["People · 0", "Agents · 1", "Earlier · 1"]);
    const rows = [...v.root.querySelectorAll(".prow.agent")];
    expect(rows.map(r => r.querySelector("b")!.textContent)).toEqual(["clax-claude", "sam-helper · Sam"]);
    expect(rows[0].querySelector("p")!.textContent).toBe("Idle.");
    expect(rows[1].querySelector("p")!.textContent).toBe("Ended. Addressed #1 in v2.");
    expect(rows[1].classList.contains("ended")).toBe(true);
    v.unmount();
  });
});

describe("PeoplePanel's agents that comments do not wake", () => {
  afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); });
  const settle = async () => { for (let i = 0; i < 5; i++) await Promise.resolve(); flush(); };

  it("says under an idle live agent that no comment wakes it until it next acts, and nothing under one that is woken", async () => {
    vi.stubGlobal("fetch", vi.fn(async (url: string) => new Response(JSON.stringify(url === "/api/agents" ? { agents: [] } : { viewer: { public_id: "u_1", display_name: "Alex", created_at: "x" } }))));
    const ALEX = { public_id: "u_1", name: "Alex", owner: true };
    const agents = [
      { handle: "a_web", harness: "claude", live: true, name: "web", person: ALEX, wakes: false },
      { handle: "a_api", harness: "pi", live: true, name: "api", person: ALEX, wakes: true },
      { handle: "a_old", harness: "claude", live: false, name: "old", person: ALEX, wakes: false },
      { handle: "a_chan", harness: "claude", live: true, name: "chan", person: ALEX, wakes: "unsure" },
    ];
    const s = view(null) as unknown as Record<string, unknown>;
    const ctl = { numbers: () => new Map(), forgetReopen: vi.fn(), setNotice: vi.fn(), setMe: vi.fn(), setShareWhere: vi.fn() };
    const v = mount(PeoplePanel, { ctl: ctl as never, s: { ...s, agents, data: { artifact: { participants: { people: [], agents } }, versions: [] } } as never, onClose: vi.fn() });
    await settle();
    // The ended agent (`a_old`) is listed last, under Earlier.
    const rows = [...v.root.querySelectorAll(".prow.agent")];
    expect(rows.map(r => r.querySelector("b")?.textContent)).toEqual(["web", "api", "chan", "old"]);
    expect(rows.map(r => r.querySelector(".nowake")?.textContent ?? null)).toEqual(["Won't be woken by your comments until it next acts.", null, "May not be woken by your comments: Claude Code does not confirm its channel.", null]);
    v.unmount();
  });
});

describe("PeoplePanel's raise for the artifact's creator", () => {
  afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); });
  const settle = async () => { for (let i = 0; i < 12; i++) await new Promise(r => setTimeout(r, 0)); flush(); };
  const SAM = { public_id: "u_sam", name: "Sam", owner: false };
  const MIA = { public_id: "u_mia", name: "Mia", owner: false };
  const HOLDS = [
    { type: "clax-artifacts", actions: ["read", "update", "delete"], locations: ["notes"] },
    { type: "clax-comments", actions: ["read", "write", "reply", "resolve", "send", "watch"], locations: ["notes"] },
    { type: "clax-db", actions: ["read", "write"], locations: ["notes"], level: "admin" },
  ];
  const agents = [
    { handle: "h_mia", harness: "codex", live: true, name: "notes-fix", person: MIA },
    { handle: "h_local", harness: "claude", live: true, name: "clax-claude", person: { public_id: "u_1", name: "Alex", owner: true } },
  ];
  type Call = { url: string; method: string; body: unknown };
  function open(raises: unknown) {
    const calls: Call[] = [];
    vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
      calls.push({ url, method: init?.method ?? "GET", body: init?.body ? JSON.parse(String(init.body)) : undefined });
      if (url.startsWith("/api/people/me/raises")) return raises === null ? new Response("{}", { status: 403 }) : new Response(JSON.stringify(raises));
      if (url === "/api/agents") return new Response(JSON.stringify({ agents: [] }));
      return new Response(JSON.stringify({ agent: {} }));
    }));
    const s = view(null) as unknown as Record<string, unknown>;
    const ctl = { numbers: () => new Map(), forgetReopen: vi.fn(), setNotice: vi.fn(), setMe: vi.fn(), setShareWhere: vi.fn() };
    const v = mount(PeoplePanel, { ctl: ctl as never, s: { ...s, me: { public_id: SAM.public_id, display_name: "Sam", created_at: "x" }, agents, data: { artifact: { id: "notes", participants: { people: [], agents } }, versions: [] } } as never, onClose: vi.fn() });
    return { v, calls };
  }
  const row = (v: { root: Element }, name: string) => [...v.root.querySelectorAll(".prow.agent")].find(r => r.querySelector("b")!.textContent!.startsWith(name))!;
  const box = (el: Element, label: string) => [...el.querySelectorAll("label")].find(l => l.textContent!.trim() === label)!.querySelector("input")!;

  it("offers the creator the actions they hold here, on the agents a raise reaches, and sets their own raise", async () => {
    const { v, calls } = open({ creator: true, holds: HOLDS, raisable: ["h_mia"], raises: [
      { agent: { handle: "h_mia", name: "notes-fix", person: "u_mia", live: true }, access: [{ type: "clax-artifacts", actions: ["update"], locations: ["other"] }], version: "01V1" }] });
    await settle();
    expect(calls.map(c => c.url)).toContain("/api/people/me/raises?artifact=notes");
    const mia = row(v, "notes-fix");
    // The raise's chunk loads on first use.
    await vi.waitFor(() => { flush(); expect(mia.querySelector(".raise-here")).not.toBeNull(); });
    const here = mia.querySelector(".raise-here")!;
    // This machine's agent acts with the token: no raise is offered for it.
    expect(row(v, "clax-claude").querySelector(".raise-here")).toBeNull();
    expect(here.querySelector("summary")!.textContent).toBe("Raise on this artifact");
    const labels = [...here.querySelectorAll("label")].map(l => l.textContent!.trim());
    // Creating and asking are not at an artifact: no creator raises them.
    expect(labels).not.toContain("Publish new artifacts");
    expect(labels).not.toContain("Ask questions");
    expect(labels).toContain("Publish new versions");
    const publish = box(here, "Publish new versions");
    expect(publish.checked).toBe(false);
    publish.checked = true;
    flush(() => { publish.dispatchEvent(new Event("change", { bubbles: true })); });
    await settle();
    const put = calls.find(c => c.method === "PUT")!;
    expect(put.url).toBe("/api/agents/h_mia/elevate");
    // The raise elsewhere is kept; this artifact's is added.
    // It names the raise it was built on, so it never brings back one removed since.
    expect(put.body).toEqual({ access: [{ type: "clax-artifacts", actions: ["update"], locations: ["other"] }, { type: "clax-artifacts", actions: ["update"], locations: ["notes"] }], if_version: "01V1" });
    const reads = () => calls.filter(c => c.url.startsWith("/api/people/me/raises")).length;
    expect(reads()).toBeGreaterThanOrEqual(2);
    // Back on the page, the raises are read again.
    const before = reads();
    window.dispatchEvent(new Event("focus"));
    await settle();
    expect(reads()).toBe(before + 1);
    v.unmount();
  });

  it("removes the creator's raise when its last action here is turned off", async () => {
    const { v, calls } = open({ creator: true, holds: HOLDS, raisable: ["h_mia"], raises: [
      { agent: { handle: "h_mia", name: "notes-fix", person: "u_mia", live: true }, access: [{ type: "clax-artifacts", actions: ["update"], locations: ["notes"] }] }] });
    await settle();
    const here = row(v, "notes-fix").querySelector(".raise-here")!;
    expect(here.querySelector("summary")!.textContent).toBe("Raise on this artifact (raised)");
    const publish = box(here, "Publish new versions");
    expect(publish.checked).toBe(true);
    publish.checked = false;
    flush(() => { publish.dispatchEvent(new Event("change", { bubbles: true })); });
    await settle();
    expect(calls.find(c => c.method === "DELETE")!.url).toBe("/api/agents/h_mia/elevate");
    v.unmount();
  });

  it("offers nothing to a viewer who did not create the artifact, or has no People", async () => {
    for (const raises of [{ creator: false, holds: null, raisable: [], raises: [] }, null]) {
      const { v } = open(raises);
      await settle();
      expect(v.root.querySelector(".raise-here")).toBeNull();
      v.unmount();
    }
  });
});

describe("PeoplePanel's raise when someone else changed it", () => {
  afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); });
  const settle = async () => { for (let i = 0; i < 12; i++) await new Promise(r => setTimeout(r, 0)); flush(); };

  it("is refused when the raise it was built on was removed since, and then shows the raise as it stands", async () => {
    const MIA = { public_id: "u_mia", name: "Mia", owner: false };
    const agents = [{ handle: "h_mia", harness: "codex", live: true, name: "notes-fix", person: MIA }];
    const HOLDS = [{ type: "clax-artifacts", actions: ["read", "update", "delete"], locations: ["notes"] }];
    let removed = false;
    const calls: { url: string; method: string; body: unknown }[] = [];
    vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
      calls.push({ url, method: init?.method ?? "GET", body: init?.body ? JSON.parse(String(init.body)) : undefined });
      if (url.startsWith("/api/people/me/raises")) {
        const raises = removed ? [] : [{ agent: { handle: "h_mia", name: "notes-fix", person: "u_mia", live: true }, access: [{ type: "clax-artifacts", actions: ["update", "delete"], locations: ["notes"] }], version: "01V1" }];
        return new Response(JSON.stringify({ creator: true, holds: HOLDS, raisable: ["h_mia"], raises }));
      }
      if (url === "/api/agents") return new Response(JSON.stringify({ agents: [] }));
      if (init?.method === "PUT") return new Response(JSON.stringify({ error: { code: "conflict", message: "changed" }, raise: null }), { status: 409 });
      return new Response("{}");
    }));
    const s = view(null) as unknown as Record<string, unknown>;
    const ctl = { numbers: () => new Map(), forgetReopen: vi.fn(), setNotice: vi.fn(), setMe: vi.fn(), setShareWhere: vi.fn() };
    const v = mount(PeoplePanel, { ctl: ctl as never, s: { ...s, agents, data: { artifact: { id: "notes", participants: { people: [], agents } }, versions: [] } } as never, onClose: vi.fn() });
    await settle();
    const row = v.root.querySelector(".prow.agent")!;
    await vi.waitFor(() => { flush(); expect(row.querySelector(".raise-here")).not.toBeNull(); });
    // Mia removed it meanwhile; this panel still shows it, and unchecks Delete.
    removed = true;
    const del = [...row.querySelectorAll(".raise-here label")].find(l => l.textContent!.trim() === "Delete artifacts")!.querySelector("input")!;
    expect(del.checked).toBe(true);
    del.checked = false;
    flush(() => { del.dispatchEvent(new Event("change", { bubbles: true })); });
    await settle();
    const put = calls.find(c => c.method === "PUT")!;
    expect((put.body as { if_version: string }).if_version).toBe("01V1");
    expect(row.querySelector(".rerr")!.textContent).toContain("This raise changed since you opened it");
    // Read again: nothing is raised, as it stands.
    const boxes = [...row.querySelectorAll<HTMLInputElement>(".raise-here input")];
    expect(boxes.every(b => !b.checked)).toBe(true);
    expect(row.querySelector(".raise-here summary")!.textContent).toBe("Raise on this artifact");
    v.unmount();
  });
});
