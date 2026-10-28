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
    // Another Clax's agent says whose Clax it relays from, in place of its person.
    const relayed = mount(AgentStrip, { agents: [agent({ handle: "a_rel", name: "claude", harness: "claude", own: false, person_name: "Ada", via: "Ada's Clax", blocked: { since: "2026-10-09T10:18:00.000Z", questions: 1, approvals: 0 } })], now, selected: null, onPick: vi.fn() });
    expect(chips(relayed.root)[0].textContent!.replace(/\s+/g, " ").trim()).toBe("claude · via Ada's Clax · waiting on you 12 min (1 question)");
    relayed.unmount();
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

  it("at phone width, folds the chips behind one Filter by agent button that says what it holds", () => {
    const onPick = vi.fn();
    const m = mount(AgentStrip, { agents: AGENTS, now, selected: null as string | null, onPick });
    const toggle = m.root.querySelector<HTMLButtonElement>("button.astrip-toggle")!;
    const group = m.root.querySelector(".astrip")!;
    // A real button that names what it opens, before the chips.
    expect(toggle.textContent!.replace(/\s+/g, " ").trim()).toBe("Filter by agent (4 agents · 2 waiting on you)");
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    expect(toggle.getAttribute("aria-controls")).toBe(group.id);
    expect(group.id).not.toBe("");
    expect(toggle.compareDocumentPosition(group) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(group.hasAttribute("data-open")).toBe(false);
    click(toggle);
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
    expect(group.hasAttribute("data-open")).toBe(true);
    // Picking an agent filters and folds the chips again; the button names the agent.
    click(chips(m.root)[2]);
    expect(onPick).toHaveBeenLastCalledWith("a_web");
    expect(group.hasAttribute("data-open")).toBe(false);
    m.update({ agents: AGENTS, now, selected: "a_web", onPick });
    expect(toggle.textContent!.replace(/\s+/g, " ").trim()).toBe("Agent: web (4 agents · 2 waiting on you)");
    // The phone's rules: the button shows and the chips hide until it opens them; elsewhere the button is hidden.
    const css = document.head.textContent!;
    expect(css).toMatch(/\.astrip-toggle\s*\{\s*display:\s*none;?\s*\}/);
    expect(css).toMatch(/@media \(max-width: 700px\)[^@]*\.astrip-toggle\s*\{\s*display:\s*inline-flex/);
    expect(css).toMatch(/@media \(max-width: 700px\)[^@]*\.astrip:not\(\[data-open\]\)\s*\{\s*display:\s*none;?\s*\}/);
  });
});
