import { afterEach, expect, it, vi } from "vitest";
import type { Thread } from "./threads";

// A line's time is relative on the day that is today and a clock time on an
// earlier one, decided by the day's date: the day's label is display text.
afterEach(() => {
  vi.doUnmock("./view/timeline-model");
  vi.resetModules();
});

it("words a line's time by its day's date, whatever the day's label says", async () => {
  vi.resetModules();
  vi.doMock("./view/timeline-model", async importOriginal => {
    const m = await importOriginal<typeof import("./view/timeline-model")>();
    return { ...m, days: (...a: Parameters<typeof m.days>) => m.days(...a).map(d => ({ ...d, label: d.label === "Today" ? "Heute" : d.label })) };
  });
  // The component, its runtime and the mount helper all come from the registry the mock is in.
  const { mount } = await import("./test/svelte");
  const { timeline } = await import("./view/timeline-model");
  const { default: Timeline } = await import("./ui/Timeline.svelte");
  const now = new Date(2026, 10, 13, 12, 0);
  const made = new Date(2026, 10, 13, 9, 0).toISOString();
  const t: Thread = {
    id: "tm1", artifact_id: "7q3k9mzx2b4t", version_n: 1, status: "open", sent_to_agent: false, has_clip: false, clip_url: null, created_at: made,
    resolved_at: null, resolved_by: null, feedback_state: null,
    anchor: { kind: "element", selector: "body > h2", quote: "Goals", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" },
    comments: [{ id: "tm1.1", thread_id: "tm1", author_kind: "viewer", author_name: "Mia", via_harness: null, body: "Shorter", created_at: made }],
  };
  const tl = timeline({ threads: [t], versions: [], working: [], claimed: [] });
  const view = mount(Timeline, { tl, threads: [t], now, selected: null, numbers: new Map(), detached: new Set<string>(), side: "right" as const,
    expanded: [], onExpand: () => {}, card: (() => {}) as never });
  expect(view.root.querySelector(".tl-day")!.textContent).toBe("Heute");
  expect(view.root.querySelector(".tl-time")!.textContent).toBe("3 h ago");
  view.unmount();
});
