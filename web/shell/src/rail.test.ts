import { afterEach, describe, expect, it, vi } from "vitest";
import { writable } from "svelte/store";
import { flush, mount } from "./test/svelte";
import type { Thread } from "./threads";
import Rail from "./ui/Rail.svelte";
import SidebarIsland from "./ui/SidebarIsland.svelte";
import { railShows } from "./ui/sidebar-chunk";
import type { Line, Timeline } from "./view/timeline-model";

afterEach(() => {
  vi.unstubAllGlobals();
  delete document.documentElement.dataset.drawer;
  document.body.replaceChildren();
});

// A timeline of `pending` threads, `waiting` of them on a person, and `lines` (oldest first).
const ids = (n: number, p: string) => Array.from({ length: n }, (_, i) => `${p}${i}`);
const tl = (pending: number, waiting: number, lines: Line[] = []): Timeline => ({ lines, pending: ids(pending, "p"), waiting: ids(waiting, "p"), stateOf: new Map() });
// Versions 1..n published a minute apart on 13 November 2026 (a Friday), from 10:00 local time; `day` moves the first `older` of them to the day before.
const versions = (n: number, older = 0): Line[] => Array.from({ length: n }, (_, i) => ({
  key: `v${i + 1}`, at: new Date(2026, 10, i < older ? 12 : 13, 10, i).toISOString(), kind: "version", who: "claude", agent: true, mine: false, v: i + 1,
  ids: [], here: [], pending: 2, waiting: 1,
}));

function rail(t: Timeline, more: Partial<{ side: "left" | "right"; working: boolean; height: number }> = {}) {
  const onOpen = vi.fn();
  const m = mount(Rail, { tl: t, side: "right" as "left" | "right", working: false, height: 400, onOpen, ...more });
  const button = m.root.querySelector<HTMLButtonElement>("button.rail")!;
  const svg = m.root.querySelector("svg")!;
  // The band's labels, positioned over it: their texts, and the vertical centres they are placed at.
  const label = (cls: string) => Array.from(m.root.querySelectorAll<HTMLElement>(`.rail-l.${cls}`));
  const labels = (cls: string) => label(cls).map(e => e.textContent);
  const ys = (cls: string) => label(cls).map(e => parseFloat(e.style.top));
  return { m, onOpen, button, svg, labels, ys };
}
// How many marks a tally path draws, and where its first starts.
const count = (p: Element | null) => (p?.getAttribute("d") ?? "").match(/M/g)?.length ?? 0;
const firstX = (p: Element | null) => Number(/^M([\d.]+)/.exec(p?.getAttribute("d") ?? "")?.[1]);

describe("railShows", () => {
  const s = { panel: false, threads: [{}], deleted: false };
  it("is true for a closed panel docked left or right, wider than a phone, outside inspecting, with a thread", () => {
    expect(railShows(s, "right", true, false)).toBe(true);
    expect(railShows(s, "left", true, false)).toBe(true);
    // No layout stored is docked right.
    expect(railShows(s, "", true, false)).toBe(true);
    expect(railShows(s, undefined, true, false)).toBe(true);
  });
  it("is false with the panel open, no threads, a deleted artifact, a bottom or floating drawer, at phone width, or inspecting", () => {
    expect(railShows({ ...s, panel: true }, "right", true, false)).toBe(false);
    expect(railShows({ ...s, threads: [] }, "right", true, false)).toBe(false);
    expect(railShows({ ...s, deleted: true }, "right", true, false)).toBe(false);
    expect(railShows(s, "bottom", true, false)).toBe(false);
    expect(railShows(s, "float", true, false)).toBe(false);
    expect(railShows(s, "right", false, false)).toBe(false);
    expect(railShows(s, "right", true, true)).toBe(false);
  });
});

describe("Rail", () => {
  it("is one button that names the counts and shows the threads", () => {
    const { m, button, onOpen } = rail(tl(3, 2));
    expect(m.root.querySelectorAll("button")).toHaveLength(1);
    expect(button.getAttribute("aria-label")).toBe("Threads: 3 pending, 2 need you. Show threads");
    expect(button.dataset.shellControl).toBe("threads");
    // A native button: Enter and Space activate it as a click.
    expect(button.type).toBe("button");
    expect(button.hasAttribute("tabindex")).toBe(false);
    flush(() => button.click());
    expect(onOpen).toHaveBeenCalledTimes(1);
    expect(Array.from(m.root.querySelectorAll(".rail-n"), e => e.textContent)).toEqual(["3", "2"]);
    expect(Array.from(m.root.querySelectorAll(".rail-cap"), e => e.textContent)).toEqual(["pending", "need you"]);
    m.unmount();
  });

  it("says 1 needs you, and leaves the clause out when nothing waits on a person", () => {
    const one = rail(tl(3, 1));
    expect(one.button.getAttribute("aria-label")).toBe("Threads: 3 pending, 1 needs you. Show threads");
    expect(Array.from(one.m.root.querySelectorAll(".rail-cap"), e => e.textContent)).toEqual(["pending", "needs you"]);
    one.m.unmount();
    const none = rail(tl(4, 0));
    expect(none.button.getAttribute("aria-label")).toBe("Threads: 4 pending. Show threads");
    expect(Array.from(none.m.root.querySelectorAll(".rail-n"), e => e.textContent)).toEqual(["4"]);
    none.m.unmount();
  });

  it("tallies the pending threads up to the band's cap, the waiting ones in their share and at least one", () => {
    const marks = (t: Timeline) => {
      const r = rail(t);
      const out = [count(r.svg.querySelector("path.rail-mark:not(.waits)")), count(r.svg.querySelector("path.rail-mark.waits"))];
      r.m.unmount();
      return out;
    };
    // [the agents' marks, the waiting ones]
    expect(marks(tl(3, 1))).toEqual([2, 1]);
    expect(marks(tl(4, 0))).toEqual([4, 0]);
    expect(marks(tl(9, 3))).toEqual([4, 2]);
    expect(marks(tl(20, 1))).toEqual([5, 1]);
    expect(marks(tl(0, 0))).toEqual([0, 0]);
    // Docked right the outer edge is the right one: the waiting marks lie inward of the agents', as the band's waiting area does.
    const r = rail(tl(3, 1));
    expect(firstX(r.svg.querySelector("path.rail-mark.waits"))).toBeLessThan(firstX(r.svg.querySelector("path.rail-mark:not(.waits)")));
    r.m.unmount();
  });

  it("shows a dot while an agent works", () => {
    const idle = rail(tl(2, 0));
    expect(idle.svg.querySelector(".rail-dot.on")).toBeNull();
    idle.m.unmount();
    const busy = rail(tl(2, 0), { working: true });
    expect(busy.svg.querySelector(".rail-dot.on")).not.toBeNull();
    busy.m.unmount();
  });

  it("draws the band newest first, a step per line, with its labels, all hidden from assistive technology", () => {
    const { m, svg, labels, ys } = rail(tl(2, 1, versions(3)));
    expect(m.root.querySelector(".rail-plot")!.getAttribute("aria-hidden")).toBe("true");
    expect(svg.closest(".rail-plot")).not.toBeNull();
    expect(labels("v")).toEqual(["v3", "v2", "v1"]);
    // One step (18 px) between lines.
    expect(ys("v")).toEqual([32, 50, 68]);
    expect(labels("cnt")).toEqual(["2"]);
    expect(svg.querySelector(".rail-band-a")!.getAttribute("d")).not.toBe("");
    expect(svg.querySelector(".rail-band-w")!.getAttribute("d")).not.toBe("");
    m.unmount();
  });

  it("breaks the band where the day changes, with the earlier day's short name", () => {
    const { m, svg, labels, ys } = rail(tl(2, 1, versions(4, 2)));
    expect(labels("gap")).toEqual(["Thu"]);
    expect(count(svg.querySelector(".rail-gap"))).toBe(1);
    // The break adds 20 px to the step.
    expect(ys("v")).toEqual([32, 50, 88, 106]);
    m.unmount();
  });

  it("draws no line below its height", () => {
    const { m, svg, labels, ys } = rail(tl(2, 1, versions(10)), { height: 100 });
    expect(labels("v")).toEqual(["v10", "v9", "v8", "v7"]);
    for (const y of [...ys("v"), ...ys("cnt"), ...ys("gap")]) expect(y).toBeLessThanOrEqual(100);
    expect(count(svg.querySelector(".rail-tick"))).toBe(4);
    m.unmount();
  });

  it("puts the band, the tally and their labels on the left edge when docked left", () => {
    const { m, svg } = rail(tl(3, 1, versions(1)), { side: "left" });
    const agents = svg.querySelector("path.rail-mark:not(.waits)"), waits = svg.querySelector("path.rail-mark.waits");
    expect(firstX(agents)).toBeLessThan(5);
    expect(firstX(agents)).toBeLessThan(firstX(waits));
    expect(m.root.querySelector(".rail-labels")!.classList.contains("start")).toBe(true);
    m.unmount();
  });
});

describe("the sidebar island with the panel closed", () => {
  const at = "2026-11-13T10:00:00.000Z";
  const thread: Thread = {
    id: "tr1", artifact_id: "7q3k9mzx2b4t", version_n: 1, status: "open", sent_to_agent: false, has_clip: false, clip_url: null, created_at: at,
    resolved_at: null, resolved_by: null, feedback_state: null,
    anchor: { kind: "element", selector: "body > h2", quote: "Goals", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" },
    comments: [{ id: "tr1.1", thread_id: "tr1", author_kind: "viewer", author_name: "Mia", via_harness: null, body: "Shorter", created_at: at }],
  };
  function island(threads: Thread[] = [thread]) {
    vi.stubGlobal("matchMedia", (q: string) => ({ matches: q === "(min-width: 701px)", media: q, addEventListener() {}, removeEventListener() {} }));
    const state = writable({ error: null, origin: "", data: { versions: [], artifact: {} }, panel: false, deleted: false, threads, working: [], me: null });
    const ctl = { state, togglePanel: vi.fn(), setNotice: vi.fn(), openCount: () => threads.length };
    return { m: mount(SidebarIsland, { ctl: ctl as never }), ctl };
  }

  it("holds the strip's box until its chunk loads, then shows the strip, which opens the threads", async () => {
    const { m, ctl } = island();
    expect(m.root.querySelector('aside.rail[aria-busy="true"]')).not.toBeNull();
    expect(m.root.querySelector("button.rail")).toBeNull();
    const button = await vi.waitFor(() => { flush(); const b = m.root.querySelector<HTMLButtonElement>("button.rail"); expect(b).not.toBeNull(); return b!; });
    expect(m.root.querySelector("aside.rail[aria-busy]")).toBeNull();
    expect(button.closest("aside.rail")).not.toBeNull();
    expect(button.getAttribute("aria-label")).toBe("Threads: 1 pending, 1 needs you. Show threads");
    flush(() => button.click());
    expect(ctl.togglePanel).toHaveBeenCalledTimes(1);
    m.unmount();
  });

  it("shows no strip docked at the bottom, or with no threads", () => {
    document.documentElement.dataset.drawer = "bottom";
    const bottom = island();
    expect(bottom.m.root.querySelector(".rail")).toBeNull();
    bottom.m.unmount();
    delete document.documentElement.dataset.drawer;
    const empty = island([]);
    expect(empty.m.root.querySelector(".rail")).toBeNull();
    empty.m.unmount();
  });
});
