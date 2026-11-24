import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { botView } from "./bot/send.svelte";
import { flush, mount } from "./test/svelte";
import type { Thread, Viewer } from "./threads";
import ShellRail from "./ui/ShellRail.svelte";
import ShellSidebar from "./ui/ShellSidebar.svelte";
import { sidebarView } from "./ui/sidebar-view.svelte";
import { timeline } from "./view/timeline-model";

// The timeline, counted: each call is one recompute.
vi.mock("./view/timeline-model", async orig => {
  const m = await orig<typeof import("./view/timeline-model")>();
  return { ...m, timeline: vi.fn(m.timeline) };
});
const calls = () => vi.mocked(timeline).mock.calls.length;

const AID = "7q3k9mzx2b4t";
const ME: Viewer = { public_id: "u_alex", display_name: "Alex", created_at: "2026-11-13T09:00:00.000Z", owner: true };
const anchor = { kind: "element" as const, selector: "body > h2", quote: "Goals", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" };
const thread = (id: string): Thread => ({
  id, artifact_id: AID, version_n: 1, anchor, status: "open", sent_to_agent: false, has_clip: false, clip_url: null, created_at: "2026-11-13T09:10:00.000Z",
  resolved_at: null, resolved_by: null, feedback_state: null,
  comments: [{ id: `c_${id}`, thread_id: id, author_kind: "viewer", author_name: "Alex", author_public_id: ME.public_id, via_harness: null, body: "Shorter", created_at: "2026-11-13T09:10:00.000Z" }],
});
// The controller's state: a new object on every change, its parts kept when they did not change.
const state = (threads: Thread[]) => ({
  threads, resolved: {}, selected: null, file: "index.html", me: ME, working: [], commenting: false, agents: [], attention: null, decided: null,
  selection: { ids: [], anchor: null }, batchNote: "", batchBusy: false, sendTo: null,
  data: { versions: [{ artifact_id: AID, n: 1, label: null, created_at: "2026-11-13T09:00:00.000Z", files: {}, agent_harness: "claude", addresses: [] }], artifact: { owner_harness: "claude" } },
});
const ctl = { id: AID, holds: () => true, shown: () => 1, setNotice: vi.fn(), togglePanel: vi.fn(), hover: vi.fn() } as never;

beforeEach(() => {
  botView.claimed = [];
  vi.stubGlobal("fetch", vi.fn(async () => new Response("{}", { status: 404 })));
  try { localStorage.clear(); } catch { /* none */ }
});
afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); });

describe("the timeline's inputs", () => {
  it("are not recomputed by the strip when the controller's state changes and they do not", () => {
    const s = state([thread("a")]);
    const m = mount(ShellRail, { ctl, s: s as never });
    const n = calls();
    expect(n).toBeGreaterThan(0);
    // A hover, a note typed: a new state object, the same threads, versions, agents at work and viewer.
    m.update({ ctl, s: { ...s, batchNote: "x" } as never });
    m.update({ ctl, s: { ...s, selected: "a" } as never });
    expect(calls()).toBe(n);
    // A new list of threads is a new input.
    m.update({ ctl, s: { ...s, threads: [...s.threads, thread("b")] } as never });
    expect(calls()).toBe(n + 1);
    m.unmount();
  });

  it("are not recomputed by the sidebar's views when the controller's state changes and they do not", async () => {
    sidebarView.set("timeline");
    const s = state([thread("a")]);
    const m = mount(ShellSidebar, { ctl, s: s as never });
    await vi.waitFor(() => expect(m.root.querySelector(".timeline")).not.toBeNull());
    const n = calls();
    m.update({ ctl, s: { ...s, batchNote: "x" } as never });
    m.update({ ctl, s: { ...s, batchBusy: true } as never });
    expect(calls()).toBe(n);
    m.update({ ctl, s: { ...s, threads: [...s.threads, thread("b")] } as never });
    expect(calls()).toBe(n + 1);
    m.unmount();
  });

  it("open and scroll to the selected thread's row when the selection changes, not when the controller's state changes and it does not", async () => {
    sidebarView.set("timeline");
    const scrolled: Element[] = [];
    const own = Element.prototype.scrollIntoView;
    Element.prototype.scrollIntoView = function (this: Element) { scrolled.push(this); };
    try {
      const s = state([thread("a"), thread("b")]);
      const m = mount(ShellSidebar, { ctl, s: s as never });
      await vi.waitFor(() => expect(m.root.querySelector(".timeline")).not.toBeNull());
      const row = (id: string) => m.root.querySelector<HTMLElement>(`.tl-row[data-row="${id}"]`)!;
      m.update({ ctl, s: { ...s, selected: "a" } as never });
      await vi.waitFor(() => expect(scrolled).toEqual([row("a")]));
      // The viewer closes the row, then the pointer enters its thread and leaves: new state objects, the same selection.
      flush(() => row("a").click());
      m.update({ ctl, s: { ...s, selected: "a", hovered: "a" } as never });
      m.update({ ctl, s: { ...s, selected: "a", hovered: null } as never });
      expect(row("a").getAttribute("aria-expanded")).toBe("false");
      // A scroll those changes asked for would come before the next selection's.
      m.update({ ctl, s: { ...s, selected: "b" } as never });
      await vi.waitFor(() => expect(scrolled.at(-1)).toBe(row("b")));
      expect(scrolled).toEqual([row("a"), row("b")]);
      m.unmount();
    } finally {
      Element.prototype.scrollIntoView = own;
    }
  });
});
