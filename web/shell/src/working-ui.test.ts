import { describe, expect, it, vi } from "vitest";

/** The `gallery` topic as the feed hears it, driven by the test. */
const topic = vi.hoisted(() => ({ on: null as ((e: Record<string, unknown> & { type: string }) => void) | null }));
vi.mock("./working-events", () => ({ subscribeGallery: (on: (e: Record<string, unknown> & { type: string }) => void) => { topic.on = on; return () => { topic.on = null; }; } }));
import { WorkingFeed } from "./ui/working-feed.svelte";
import { mount } from "./test/svelte";
import Roster from "./ui/Roster.svelte";
import WorkingSummary from "./ui/WorkingSummary.svelte";

const agents = [{ handle: "a_1111aaaa", harness: "claude", live: true }, { handle: "a_2222bbbb", harness: "codex", live: true }];
const people = [{ public_id: "u_me", display_name: "alex", seen: null }, { public_id: "u_mia", display_name: "Mia Kovač", seen: null }];

describe("working UI", () => {
  it("the roster puts people left and agents right, the viewer nearest the centre, a working agent solid", () => {
    const working = [{ key: "k", agent: "a_1111aaaa", harness: "claude", message: null, thread_ids: [], started_at: "s", last_heartbeat: "s" }];
    const m = mount(Roster, { people, agents, working, me: "u_me", max: 5 });
    const ppl = Array.from(m.root.querySelectorAll(".ppl .tok")).map(e => e.textContent);
    expect(ppl).toEqual(["AL", "MK"]);
    expect(m.root.querySelector(".ppl .tok.me")!.textContent).toBe("AL");
    expect(Array.from(m.root.querySelectorAll(".agt .tok")).map(e => [e.textContent, e.classList.contains("work")])).toEqual([["cl", true], ["cx", false]]);
    m.unmount();
  });

  it("the roster overflows past max with a +n token", () => {
    const many = Array.from({ length: 7 }, (_, i) => ({ public_id: `u_${i}`, display_name: `P${i} Q`, seen: null }));
    const m = mount(Roster, { people: many, agents: [], working: [], me: null, max: 3, small: true });
    expect(m.root.querySelector(".ppl .tok.more")!.textContent).toBe("+4");
    m.unmount();
  });

  it("the summary is a polite live region whose elapsed time sits outside it", () => {
    const m = mount(WorkingSummary, { s: { line1: "claude working on 2", agent: true, line2: "all yours", elapsed: "0:42" } });
    const live = m.root.querySelector("[role=status]")!;
    expect(live.getAttribute("aria-live")).toBe("polite");
    expect(live.textContent).toBe("claude working on 2all yours");
    expect(m.root.querySelector(".el")!.textContent).toBe("0:42");
    expect(live.contains(m.root.querySelector(".el"))).toBe(false);
    m.unmount();
  });

  it("the gallery's feed keeps an event newer than the list that seeds it", () => {
    const emit = (type: string, data: Record<string, unknown> = {}) => topic.on?.({ ...data, type, topic: "gallery" });
    try {
      const w = { key: "k", agent: "a_1111aaaa", harness: "claude", message: null, thread_ids: [], started_at: "s", last_heartbeat: "s" };
      const art = (id: string, working: unknown[]) => ({ id, title: id, description: null, icon: null, updated_at: "x", current_version: 1, pinned: false, working }) as never;
      const feed = new WorkingFeed();
      feed.seed([art("a", [w]), art("b", [w])]);
      feed.start(() => {});
      const since = feed.events;
      emit("working", { artifact_id: "a", working: [] });
      feed.seed([art("a", [w]), art("b", [])], since);
      expect(feed.byId).toEqual({ a: [], b: [] });
      feed.stop();
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("the gallery's feed passes each event's artifact to onChange at most once a second per artifact, and card deltas to onDelta at once", () => {
    const emit = (type: string, data: Record<string, unknown> = {}) => topic.on?.({ ...data, type, topic: "gallery" });
    vi.useFakeTimers();
    try {
      const onChange = vi.fn();
      const onResync = vi.fn();
      const onDelta = vi.fn();
      const feed = new WorkingFeed();
      feed.start(onResync, onChange, onDelta);
      emit("version", { artifact_id: "a", n: 2, title: "A", at: "t" });
      emit("thread", { artifact_id: "a", thread_id: "t", comments: 1 });
      emit("thread", { artifact_id: "b", thread_id: "u", comments: 1 });
      vi.advanceTimersByTime(0);
      expect(onChange.mock.calls).toEqual([["a"], ["b"]]);
      emit("thread_deleted", { artifact_id: "a", thread_id: "t" });
      emit("artifact_deleted", { artifact_id: "c" });
      vi.advanceTimersByTime(0);
      expect(onChange.mock.calls).toEqual([["a"], ["b"]]);
      expect(onDelta.mock.calls.map(([d]) => [d.type, d.artifact_id])).toEqual([["version", "a"], ["artifact_deleted", "c"]]);
      vi.advanceTimersByTime(999);
      expect(onChange).toHaveBeenCalledTimes(2);
      vi.advanceTimersByTime(1);
      expect(onChange.mock.calls[2]).toEqual(["a"]);
      expect(onResync).not.toHaveBeenCalled();
      emit("resync");
      expect(onResync).toHaveBeenCalledTimes(1);
      emit("thread", { artifact_id: "a", thread_id: "t", comments: 2 });
      feed.stop();
      vi.advanceTimersByTime(2000);
      expect(onChange).toHaveBeenCalledTimes(3);
    } finally {
      vi.useRealTimers();
      vi.unstubAllGlobals();
    }
  });

  it("the gallery's feed seeds one artifact's working list without touching the others", () => {
    const w = { key: "k", agent: "a_1111aaaa", harness: "claude", message: null, thread_ids: [], started_at: "s", last_heartbeat: "s" };
    const art = (id: string, working: unknown[]) => ({ id, title: id, description: null, icon: null, updated_at: "x", current_version: 1, pinned: false, working }) as never;
    const feed = new WorkingFeed();
    feed.seed([art("a", []), art("b", [w])]);
    feed.seedOne("a", art("a", [w]));
    expect(feed.byId).toEqual({ a: [w], b: [w] });
    feed.seedOne("b", null);
    expect(feed.byId).toEqual({ a: [w] });
  });
});
