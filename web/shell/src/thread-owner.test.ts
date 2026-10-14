import { afterEach, describe, expect, it, vi } from "vitest";
import { item } from "./q/fixtures";
import { flush, mount } from "./test/svelte";
import type { Thread, Viewer } from "./threads";
import ShellSidebar from "./ui/ShellSidebar.svelte";
import Sidebar from "./ui/Sidebar.svelte";
import { waitingLabel } from "./waiting";

const TID = "01J9T0000000000000000000AA";
const base = { artifact_id: "7q3k9mzx2b4t", version_n: 1, has_clip: false, clip_url: null, created_at: "2026-10-09T10:00:00.000Z", resolved_at: null, resolved_by: null, feedback_state: null };
const anchor = { kind: "element" as const, selector: "body > h2", quote: "Goals", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" };
const comment = { id: "c1", thread_id: TID, author_kind: "viewer" as const, author_name: "Mia", via_harness: null, body: "Add a Q4 column", created_at: base.created_at };
const withheld = { thread_id: TID, state: "withheld" as const, tier: null, since: base.created_at, resends: 0, exhausted: false, approver: "Sam" };
const thread = (o: Partial<Thread> = {}): Thread => ({ ...base, id: TID, anchor, status: "open", sent_to_agent: true, comments: [comment], ...o });
const OWNER: Viewer = { public_id: "u_alex", display_name: "Alex", created_at: base.created_at, owner: true };
const SAM: Viewer = { public_id: "u_sam", display_name: "Sam", created_at: base.created_at, person: true };
const LAN: Viewer = { public_id: "u_lucy", display_name: "Lucy", created_at: base.created_at };
const HOSTILE = `<img src=x onerror="window.pwned=1">`;
// What the sidebar loads on demand, loaded here first: it then lands within a few tasks.
await import("./people/ApprovalCard.svelte");
await import("./ui/thread-approvals");
await import("./ui/ThreadApproval.svelte");
const settle = async () => { for (let i = 0; i < 8; i++) await new Promise(r => setTimeout(r, 0)); flush(); };

function render(threads: Thread[], more: Record<string, unknown> = {}) {
  return mount(Sidebar, { versions: [], shown: 1, agent: "claude", threads, resolved: {}, now: new Date("2026-10-09T10:05:00.000Z"), selected: null,
    onSelect: vi.fn(), onSend: vi.fn(), onResolve: vi.fn(), onReply: vi.fn(), ...more });
}
/** The artifact view's sidebar, over a controller that holds `threads`. */
function shell(threads: Thread[], me: Viewer, more: Record<string, unknown> = {}, ctl: Record<string, unknown> = {}) {
  const s = { threads, resolved: {}, selected: null, file: "index.html", me, data: { versions: [], artifact: { owner_harness: "claude" } }, working: [], commenting: false,
    agents: [], attention: null, decided: null, selection: { ids: [], anchor: null }, batchNote: "", batchBusy: false, sendTo: null, ...more };
  return mount(ShellSidebar, { ctl: { holds: () => true, shown: () => 1, ...ctl } as never, s: s as never });
}
const chip = (root: Element) => root.querySelector<HTMLElement>(".thread-card .owner-chip");

afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); });

describe("a thread's owner and delivery state", () => {
  it("shows its owner agent's name only, with its person on hover, when the agent is yours", () => {
    const m = render([thread({ owner: { name: "clax-claude", person: null, person_name: "Alex", live: true } })], { me: OWNER });
    expect(chip(m.root)!.textContent).toBe("clax-claude");
    expect(chip(m.root)!.title).toBe("clax-claude, your agent");
  });

  it("shows another person's agent with its person", () => {
    const m = render([thread({ owner: { name: "sam-web", person: "u_sam", person_name: "Sam", live: true } })], { me: OWNER });
    expect(chip(m.root)!.textContent).toBe("sam-web · Sam");
    // The hover names the agent whole too: at phone width the chip clips it.
    expect(chip(m.root)!.title).toBe("sam-web, Sam's agent");
    // Sam's own browser: his agent, by name only.
    m.unmount();
    const s = render([thread({ owner: { name: "sam-web", person: "u_sam", person_name: "Sam", live: true } })], { me: SAM });
    expect(chip(s.root)!.textContent).toBe("sam-web");
  });

  it("shows no chip for a thread nobody owns, or an ended owner, and keeps names as text", () => {
    const m = render([thread(), thread({ id: "t2", owner: { name: "gone", person: null, person_name: "Alex", live: false } })], { me: OWNER });
    expect(m.root.querySelector(".owner-chip")).toBeNull();
    m.unmount();
    const h = render([thread({ owner: { name: HOSTILE, person: "u_x", person_name: HOSTILE, live: true } })], { me: OWNER });
    expect(h.root.querySelector("img")).toBeNull();
    expect(chip(h.root)!.textContent).toBe(`${HOSTILE} · ${HOSTILE}`);
  });

  it("reads withheld as waiting for the approver's approval", () => {
    expect(waitingLabel(withheld, new Date())).toBe("waiting for Sam's approval");
    const m = render([thread({ feedback_state: withheld })], { me: OWNER });
    expect(m.root.querySelector(".thread-card .waiting")!.textContent).toBe("waiting for Sam's approval");
  });

  it("puts the approver's approval card above a thread waiting for their approval", async () => {
    const calls: string[] = [];
    vi.stubGlobal("fetch", vi.fn(async (url: string) => {
      calls.push(url);
      return new Response(JSON.stringify({ items: [item("approval")], next_cursor: null, unread: 1 }));
    }));
    const m = shell([thread({ feedback_state: withheld }), thread({ id: "t2" })], SAM);
    await settle();
    expect(calls).toEqual(["/api/inbox?kind=approval&artifact=7q3k9mzx2b4t&limit=200"]);
    const card = m.root.querySelector<HTMLElement>(`.thread-card[data-thread="${TID}"]`)!;
    const above = card.previousElementSibling!;
    expect(above.querySelector(".acard")!.getAttribute("data-approval")).toBe("01J9R0000000000000000000AA");
    expect(above.querySelector(".acard .held")!.textContent).toContain("The goals table needs a Q4 column.");
    expect(card.querySelector(".waiting")!.textContent).toBe("waiting for your approval");
    // Only one card, above its own thread.
    expect(m.root.querySelectorAll(".acard")).toHaveLength(1);
  });

  it("puts the card above a thread whose newest comment went through while an earlier one waits", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ items: [item("approval")], next_cursor: null, unread: 1 }))));
    const sent = { ...withheld, state: "sent" as const, tier: "piggyback" as const, approver: undefined };
    const plusOne = { ...comment, id: "c2", author_name: "Alex", body: "+1" };
    const m = shell([thread({ feedback_state: sent, comments: [comment, plusOne] })], SAM);
    await settle();
    const card = m.root.querySelector<HTMLElement>(`.thread-card[data-thread="${TID}"]`)!;
    expect(card.previousElementSibling!.querySelector(".acard")).not.toBeNull();
    expect(card.querySelector(".waiting")!.textContent).toBe("waiting for your approval");
  });

  it("lets the person on another person's agent's chip stay whole, the agent's name giving way", () => {
    const m = render([thread({ owner: { name: "clax-multi-person-agent-strip-ab", person: "u_sam", person_name: "Sam", live: true } })], { me: OWNER });
    expect(chip(m.root)!.querySelector(".on")!.textContent).toBe("clax-multi-person-agent-strip-ab");
    expect(chip(m.root)!.querySelector(".op")!.textContent).toBe(" · Sam");
  });

  it("asks for no approval card where the host shows none (the extension's panel), for a thread never sent, or in a browser with no inbox", async () => {
    const f = vi.fn(async () => new Response("{}"));
    vi.stubGlobal("fetch", f);
    render([thread({ feedback_state: withheld })], { me: OWNER });
    shell([thread({ sent_to_agent: false })], OWNER);
    shell([thread({ feedback_state: withheld })], LAN);
    await settle();
    expect(f).not.toHaveBeenCalled();
  });
});

describe("the Send target (spec 2026-10-08 §10, §11)", () => {
  const ALEX_P = { public_id: "u_alex", name: "Alex", owner: true };
  const SAM_P = { public_id: "u_sam", name: "Sam", owner: false };
  const sams = { handle: "a_sam", harness: "claude", live: true, name: "sam-notes", person: SAM_P };
  const mine = { handle: "a_cx", harness: "codex", live: true, name: "codex", person: ALEX_P };
  const unsent = thread({ sent_to_agent: false });
  const button = (root: Element) => root.querySelector<HTMLButtonElement>(".thread-card .send > button.primary")!;

  it("names another person's agent with its person on the button and the unsent bar", () => {
    const m = render([unsent], { me: OWNER, agents: [sams, mine], sendTo: "a_sam", onSendUnsent: vi.fn() });
    expect(button(m.root).textContent).toBe("Send to sam-notes (Sam's)");
    expect(m.root.querySelector(".send-unsent")!.textContent).toBe("Send 1 unsent to sam-notes (Sam's)");
    m.unmount();
    const own = render([unsent], { me: OWNER, agents: [sams, mine], sendTo: "a_cx", onSendUnsent: vi.fn() });
    expect(button(own.root).textContent).toBe("Send to codex");
  });

  it("with only other people's agents live and no target, Send opens the menu and nothing sends in one press", async () => {
    await import("./ui/SendMenu.svelte");
    vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ approves_first: ["u_sam"] }))));
    const onSend = vi.fn();
    const m = render([unsent], { me: OWNER, agents: [sams], sendTo: null, onSend, onSendUnsent: vi.fn() });
    expect(button(m.root).textContent).toBe("Send to …");
    // The unsent bar stays, and opens the menu too.
    const bar = m.root.querySelector<HTMLButtonElement>(".send-unsent .send > button.primary")!;
    expect(bar.textContent).toBe("Send 1 unsent to …");
    expect(bar.getAttribute("aria-haspopup")).toBe("menu");
    flush(() => button(m.root).click());
    await settle();
    expect(onSend).not.toHaveBeenCalled();
    const entry = m.root.querySelector('[role="menu"] [role="menuitemradio"]')!;
    expect(entry.textContent).toBe("sam-notes Sam's agent Sam approves first");
  });

  it("in the artifact view, a Send with none chosen goes to this viewer's own agent, chosen as it sends", () => {
    const calls: string[] = [];
    const ctl = { chooseTarget: (h: string) => calls.push(`choose ${h}`), sendThread: (t: Thread) => calls.push(`send ${t.id}`) };
    const m = shell([unsent], OWNER, { agents: [sams, mine] }, ctl);
    expect(button(m.root).textContent).toBe("Send to codex");
    flush(() => button(m.root).click());
    expect(calls).toEqual(["choose a_cx", `send ${TID}`]);
  });

  it("never asks who approves first in a browser that may not learn it (a LAN viewer)", async () => {
    await import("./ui/SendMenu.svelte");
    const f = vi.fn(async () => new Response("{}"));
    vi.stubGlobal("fetch", f);
    const m = render([unsent], { me: LAN, agents: [sams, mine], sendTo: null });
    flush(() => button(m.root).click());
    await settle();
    expect(m.root.querySelector('[role="menu"]')).not.toBeNull();
    expect(f).not.toHaveBeenCalled();
  });
});
