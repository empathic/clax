import { afterEach, describe, expect, it, vi } from "vitest";
import type { Participants } from "./api";
import { flush, mount } from "./test/svelte";
import SendButton from "./ui/SendButton.svelte";
import { sendGroups } from "./view/send-menu";

type A = Participants["agents"][number];
const ALEX = { public_id: "u_alex", name: "Alex", owner: true };
const SAM = { public_id: "u_sam", name: "Sam", owner: false };
const MIA = { public_id: "u_mia", name: "Mia", owner: false };
const agent = (handle: string, name: string, person: typeof ALEX, live = true): A => ({ handle, harness: "claude", live, name, person });
// Sam's agent comes first in the daemon's order (most recently active): the menu still leads with yours.
const AGENTS = [agent("a_sam", "sam-web", SAM), agent("a_cl", "clax-claude", ALEX), agent("a_mp", "clax-multi-person", ALEX), agent("a_mia", "mia-docs", MIA), agent("a_gone", "old", SAM, false)];
const settle = async () => { for (let i = 0; i < 5; i++) await new Promise(r => setTimeout(r, 0)); flush(); };
// The menu loads on first open: loaded here first, it opens a few microtasks after the press.
await import("./ui/SendMenu.svelte");
const names = new Map(AGENTS.map(a => [a.handle, a.name!]));

afterEach(() => { document.body.replaceChildren(); });

describe("the Send menu", () => {
  it("groups live agents as Your agents, then each other person's, saying who approves first", () => {
    expect(sendGroups(AGENTS, "u_alex", ["u_sam"])).toEqual([
      { label: "Your agents", mine: true, agents: [{ handle: "a_cl", name: "clax-claude", first: null }, { handle: "a_mp", name: "clax-multi-person", first: null }] },
      { label: "Sam's agents", mine: false, agents: [{ handle: "a_sam", name: "sam-web", first: "Sam approves first" }] },
      { label: "Mia's agents", mine: false, agents: [{ handle: "a_mia", name: "mia-docs", first: null }] },
    ]);
    // The owner (`null` in `approves_first`) asks themself first: "You approve first".
    expect(sendGroups(AGENTS, "u_alex", [null])[0].agents[0].first).toBe("You approve first");
    // A connected person's browser: Sam's agents are theirs; the owner's are named.
    expect(sendGroups(AGENTS, "u_sam", [null]).map(g => [g.label, g.agents[0].first])).toEqual([["Your agents", null], ["Alex's agents", "Alex approves first"], ["Mia's agents", null]]);
    // A viewer who has no agents: every group is someone's.
    expect(sendGroups(AGENTS, "u_lan", []).map(g => g.label)).toEqual(["Sam's agents", "Alex's agents", "Mia's agents"]);
  });

  it("lists Your agents, then Sam's agents, by name, in the caret's menu", async () => {
    const onChoose = vi.fn();
    const m = mount(SendButton, { label: "Send to clax-claude", agents: AGENTS, names, target: "a_cl", me: "u_alex", firsts: ["u_sam"], onSend: vi.fn(), onChoose });
    flush(() => m.root.querySelector<HTMLButtonElement>('[aria-label="Choose the agent"]')!.click());
    await settle();
    const menu = m.root.querySelector('[role="menu"]')!;
    expect([...menu.querySelectorAll('[role="group"]')].map(g => g.getAttribute("aria-label"))).toEqual(["Your agents", "Sam's agents", "Mia's agents"]);
    expect([...menu.querySelectorAll(".send-group-h")].map(h => h.textContent)).toEqual(["Your agents", "Sam's agents", "Mia's agents"]);
    const items = [...menu.querySelectorAll<HTMLButtonElement>('[role="menuitemradio"]')];
    expect(items.map(i => [i.querySelector(".send-name")!.textContent, i.getAttribute("aria-checked")])).toEqual([["clax-claude", "true"], ["clax-multi-person", "false"], ["sam-web", "false"], ["mia-docs", "false"]]);
    expect(items[2].querySelector(".send-first")!.textContent).toBe("Sam approves first");
    expect(items[0].querySelector(".send-first")).toBeNull();
    flush(() => items[2].click());
    expect(onChoose).toHaveBeenCalledWith("a_sam");
    expect(m.root.querySelector('[role="menu"]')).toBeNull();
  });

  it("offers the caret for one live agent when it is another person's", () => {
    const m = mount(SendButton, { label: "Send to sam-web", agents: [AGENTS[0]], names, target: "a_sam", me: "u_alex", firsts: [], onSend: vi.fn(), onChoose: vi.fn() });
    expect(m.root.querySelector('[aria-label="Choose the agent"]')).not.toBeNull();
  });
});
