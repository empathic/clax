import { afterEach, describe, expect, it, vi } from "vitest";
import { dispatchTrusted } from "../../../bridge/test/trusted";
import type { StripAgent } from "../api";
import { flush, mount } from "../test/svelte";
import AgentStrip from "./AgentStrip.svelte";
import { stripOrder, waitedFor } from "./strip";

const click = (el: Element) => flush(() => { dispatchTrusted(el, new MouseEvent("click", { bubbles: true, cancelable: true, detail: 1 })); });
// The fake clock: every label reads this time.
const now = new Date("2026-10-09T10:30:00.000Z");
const HOSTILE = `<img src=x onerror="window.pwned=1">`;
const agent = (o: Partial<StripAgent>): StripAgent => ({ handle: "a_1", name: "web", harness: "codex", live: true, unread: 0, blocked: null, ...o });
// In the daemon's order, which this test does not trust: blocked-longest first is the strip's own rule.
const AGENTS = [
  agent({ handle: "a_web", name: "web", unread: 3 }),
  agent({ handle: "a_new", name: "clax-multi-person", blocked: { since: "2026-10-09T10:28:00.000Z", questions: 0, approvals: 1 } }),
  agent({ handle: "a_old", name: "clax-claude", harness: "claude", unread: 1, blocked: { since: "2026-10-09T10:18:00.000Z", questions: 1, approvals: 0 } }),
  agent({ handle: "a_end", name: "pi", harness: "pi", live: false, unread: 1 }),
];
const chips = (root: Element) => [...root.querySelectorAll<HTMLButtonElement>(".astrip .achip")];

afterEach(() => { document.body.replaceChildren(); });

describe("AgentStrip", () => {
  it("orders the agents blocked-longest first, then the rest as the daemon listed them", () => {
    expect(stripOrder(AGENTS).map(a => a.handle)).toEqual(["a_old", "a_new", "a_web", "a_end"]);
  });

  it("says how long each agent has waited on you, from the clock it is given", () => {
    const m = mount(AgentStrip, { agents: AGENTS, now, selected: null, onPick: vi.fn() });
    expect(chips(m.root).map(c => c.textContent!.replace(/\s+/g, " ").trim())).toEqual([
      "clax-claude · waiting on you 12 min (1 question)",
      "clax-multi-person · 1 approval for you · 2 min",
      "web · 3 unread",
      "pi · ended · 1 unread",
    ]);
    m.update({ agents: AGENTS, now: new Date("2026-10-09T11:35:00.000Z"), selected: null, onPick: vi.fn() });
    expect(chips(m.root)[0].textContent).toContain("waiting on you 1 h 17 min");
  });

  it("names another person's agent with its person, and its own person's agent alone", () => {
    const m = mount(AgentStrip, { agents: [agent({ handle: "a_sam", name: "sam-notes", unread: 2, person: "u_sam", person_name: "Sam", own: false }), agent({ handle: "a_me", name: "web", unread: 1, person: null, person_name: "Alex", own: true })], now, selected: null, onPick: vi.fn() });
    expect(chips(m.root).map(c => c.textContent!.replace(/\s+/g, " ").trim())).toEqual(["sam-notes · Sam · 2 unread", "web · 1 unread"]);
  });

  it("formats a wait in minutes, then hours", () => {
    expect(waitedFor("2026-10-09T10:29:40.000Z", now)).toBe("under a minute");
    expect(waitedFor("2026-10-09T10:18:00.000Z", now)).toBe("12 min");
    expect(waitedFor("2026-10-09T08:30:00.000Z", now)).toBe("2 h");
    expect(waitedFor("2026-10-09T10:31:00.000Z", now)).toBe("under a minute");
  });

  it("filters the inbox by the chip's agent, and a second press clears it", () => {
    const onPick = vi.fn();
    const m = mount(AgentStrip, { agents: AGENTS, now, selected: null as string | null, onPick });
    click(chips(m.root)[0]);
    expect(onPick).toHaveBeenLastCalledWith("a_old");
    m.update({ agents: AGENTS, now, selected: "a_old", onPick });
    expect(chips(m.root).map(c => c.getAttribute("aria-pressed"))).toEqual(["true", "false", "false", "false"]);
    click(chips(m.root)[0]);
    expect(onPick).toHaveBeenLastCalledWith(null);
  });

  it("shows nothing without agents, and an unnamed agent by its harness", () => {
    const m = mount(AgentStrip, { agents: [] as StripAgent[], now, selected: null, onPick: vi.fn() });
    expect(m.root.querySelector(".astrip")).toBeNull();
    m.update({ agents: [agent({ name: null, harness: "codex", unread: 1 })], now, selected: null, onPick: vi.fn() });
    expect(chips(m.root)[0].textContent).toContain("codex · 1 unread");
  });

  it("keeps a hostile name as text", () => {
    const m = mount(AgentStrip, { agents: [agent({ name: HOSTILE, unread: 1 })], now, selected: null, onPick: vi.fn() });
    expect(m.root.querySelector("img")).toBeNull();
    expect(chips(m.root)[0].textContent).toContain(HOSTILE);
  });

  it("keeps a chip that clears the filter when the filtered agent has left the strip", () => {
    const onPick = vi.fn();
    const m = mount(AgentStrip, { agents: [] as StripAgent[], now, selected: "a_gone" as string | null, onPick });
    const c = chips(m.root);
    expect(c.map(x => [x.textContent!.replace(/\s+/g, " ").trim(), x.getAttribute("aria-pressed")])).toEqual([["This agent · clear the filter", "true"]]);
    click(c[0]);
    expect(onPick).toHaveBeenLastCalledWith(null);
  });
});
