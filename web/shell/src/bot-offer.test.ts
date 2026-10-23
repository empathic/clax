import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { dispatchTrusted } from "../../bridge/test/trusted";
import { meBot } from "./bot/fixtures";
import { BOT, botView } from "./bot/send.svelte";
import { flush, mount } from "./test/svelte";
import type { Thread, Viewer } from "./threads";
import ShellSidebar from "./ui/ShellSidebar.svelte";

const AID = "7q3k9mzx2b4t";
const ME: Viewer = { public_id: "u_alex", display_name: "Alex", created_at: "2026-10-09T10:00:00.000Z", owner: true };
const anchor = { kind: "element" as const, selector: "body > h2", quote: "Goals", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" };
const ended = (id: string) => ({ thread_id: id, state: "agent_ended" as const, tier: null, since: "2026-10-09T10:00:00.000Z", resends: 0, exhausted: false });
const thread = (id: string, created_at: string, author = ME.public_id): Thread => ({
  id, artifact_id: AID, version_n: 1, anchor, status: "open", sent_to_agent: true, has_clip: false, clip_url: null, created_at, resolved_at: null, resolved_by: null,
  feedback_state: ended(id),
  comments: [{ id: `c_${id}`, thread_id: id, author_kind: "viewer", author_name: "Alex", author_public_id: author, via_harness: null, body: "@agent use the new layout", created_at }],
});
await import("./bot/BotOffer.svelte");
const settle = async () => { for (let i = 0; i < 8; i++) await new Promise(r => setTimeout(r, 0)); flush(); };
const click = (el: Element) => flush(() => { dispatchTrusted(el, new MouseEvent("click", { bubbles: true, cancelable: true, detail: 1 })); });

/** The sidebar over `threads`, with the daemon answering a run start. */
function shell(threads: Thread[]) {
  const runs: unknown[] = [];
  vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
    if (url === "/api/bot/runs" && init?.method === "POST") { runs.push(JSON.parse(String(init.body))); return new Response(JSON.stringify({ run: { id: `r${runs.length}` } })); }
    return new Response("{}", { status: 404 });
  }));
  const s = { threads, resolved: {}, selected: null, file: "index.html", me: ME, data: { versions: [], artifact: { owner_harness: "claude" } }, working: [], commenting: false,
    agents: [], attention: null, decided: null, selection: { ids: [], anchor: null }, batchNote: "", batchBusy: false, sendTo: null };
  const m = mount(ShellSidebar, { ctl: { id: AID, holds: () => true, shown: () => 1, setNotice: vi.fn() } as never, s: s as never });
  return { m, runs };
}

beforeEach(() => {
  botView.run = null; botView.started = []; botView.claimed = []; botView.chosen = false;
  botView.me = meBot({ estimate_milli: { low: 2000, high: 6000 } });
  botView.agent = { handle: BOT, harness: BOT, name: "ClaxBot", live: true, builtin: true, line: "" };
  try { localStorage.clear(); } catch { /* none */ }
});
afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); botView.agent = null; });

describe("ClaxBot's offer on a thread no agent takes", () => {
  it("asks before sending, with what it usually costs, and sends on Send to ClaxBot", async () => {
    const { m, runs } = shell([thread("t1", "2026-10-09T10:00:00.000Z")]);
    await settle();
    const offer = m.root.querySelector(".bot-offer")!;
    expect(offer.textContent).toContain("No agent is running to take this. ClaxBot can work on it here, using your credits, usually 2–6 credits.");
    // Nothing is sent before the person says so.
    expect(runs).toEqual([]);
    click([...offer.querySelectorAll("button")].find(b => b.textContent === "Send to ClaxBot")!);
    await settle();
    expect(runs).toEqual([{ thread_ids: ["t1"], artifact_id: AID }]);
    expect(m.root.querySelector(".bot-offer")).toBeNull();
    m.unmount();
  });

  it("is not offered where ClaxBot is unavailable to this viewer", async () => {
    botView.agent = { handle: BOT, harness: BOT, name: "ClaxBot", live: false, builtin: true, line: "" };
    const { m } = shell([thread("t1", "2026-10-09T10:00:00.000Z")]);
    await settle();
    expect(m.root.querySelector(".bot-offer")).toBeNull();
    m.unmount();
  });

  it("Always here chooses ClaxBot, then sends this viewer's new such threads without asking, and no one else's", async () => {
    const { m, runs } = shell([thread("t1", "2026-10-09T10:00:00.000Z")]);
    await settle();
    click([...m.root.querySelectorAll(".bot-offer button")].find(b => b.textContent === "Always here")!);
    await settle();
    expect(runs).toEqual([{ thread_ids: ["t1"], artifact_id: AID }]);
    expect([botView.chosen, localStorage.getItem(`clax.sendTo.${AID}`)]).toEqual([true, BOT]);
    m.unmount();
    // A new view of the artifact: threads written since it opened go
    // without asking, a teammate's and older ones keep the offer.
    const later = new Date(Date.now() + 60_000).toISOString();
    const { m: m2, runs: runs2 } = shell([thread("t2", later), thread("t3", later, "u_sam"), thread("t4", "2026-10-09T10:00:00.000Z")]);
    await settle();
    expect(runs2).toEqual([{ thread_ids: ["t2"], artifact_id: AID }]);
    expect(m2.root.querySelectorAll(".bot-offer").length).toBe(2);
    m2.unmount();
  });
});
