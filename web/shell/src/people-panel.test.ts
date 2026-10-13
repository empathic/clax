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
  const settle = async () => { for (let i = 0; i < 5; i++) await Promise.resolve(); flush(); };
  const ALEX = { public_id: "u_1", name: "Alex", owner: true };
  const SAM = { public_id: "u_sam", name: "Sam", owner: false };

  it("names each agent, with its person when it is another's, and its state and block time as the roster has them", async () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(new Date("2026-10-09T10:30:00.000Z"));
    const roster = { agents: [
      { handle: "a_cl", name: "clax-claude", person: null, live: true, state: "waiting", since: "2026-10-09T10:18:00.000Z", blocked: { since: "2026-10-09T10:18:00.000Z", questions: 1, approvals: 0 } },
      { handle: "a_sam", name: "sam-web", person: "u_sam", live: true, state: "waiting", since: "2026-10-09T10:20:00.000Z", blocked: { since: "2026-10-09T10:20:00.000Z", questions: 0, approvals: 1 } },
    ] };
    const calls: string[] = [];
    vi.stubGlobal("fetch", vi.fn(async (url: string) => { calls.push(url); return new Response(JSON.stringify(url === "/api/agents" ? roster : { viewer: { public_id: "u_1", display_name: "Alex", created_at: "x" } })); }));
    const s = view(null) as unknown as Record<string, unknown>;
    const agents = [
      { handle: "a_cl", harness: "claude", live: true, name: "clax-claude", person: ALEX },
      { handle: "a_sam", harness: "codex", live: true, name: "sam-web", person: SAM },
    ];
    const ctl = { numbers: () => new Map(), forgetReopen: vi.fn(), setNotice: vi.fn(), setMe: vi.fn(), setShareWhere: vi.fn() };
    const v = mount(PeoplePanel, { ctl: ctl as never, s: { ...s, agents, data: { artifact: { participants: { people: [], agents } }, versions: [] } } as never, onClose: vi.fn() });
    await settle();
    expect(calls).toContain("/api/agents");
    const rows = [...v.root.querySelectorAll(".prow.agent")];
    expect(rows.map(r => r.querySelector("b")!.textContent)).toEqual(["clax-claude", "sam-web · Sam"]);
    expect(rows[0].querySelector("p")!.textContent).toBe("Waiting on you 12 min (1 question)");
    // The owner sees Sam's agent's block in full: it waits on Sam, not on the owner.
    expect(rows[1].querySelector("p")!.textContent).toBe("Waiting on Sam 10 min (1 approval)");
    v.unmount();
  });
});
