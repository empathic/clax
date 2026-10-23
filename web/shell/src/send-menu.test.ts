import { afterEach, describe, expect, it, vi } from "vitest";
import type { Participants } from "./api";
import { flush, mount } from "./test/svelte";
import SendButton from "./ui/SendButton.svelte";
import { ownTarget, sendGroups } from "./view/send-menu";

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

afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); });
/** The daemon answering who approves first; keeps every URL asked. */
function stubFirsts(firsts: (string | null)[] | number) {
  const asked: string[] = [];
  vi.stubGlobal("fetch", vi.fn(async (url: string) => { asked.push(url); return typeof firsts === "number" ? new Response("{}", { status: firsts }) : new Response(JSON.stringify({ approves_first: firsts })); }));
  return asked;
}

describe("the Send menu", () => {
  it("groups live agents as Your agents, then each other person's, saying who approves first", () => {
    expect(sendGroups(AGENTS, "u_alex", ["u_sam"])).toEqual([
      { label: "Your agents", mine: true, agents: [{ handle: "a_cl", name: "clax-claude", person: null, first: null }, { handle: "a_mp", name: "clax-multi-person", person: null, first: null }] },
      { label: "Sam's agents", mine: false, agents: [{ handle: "a_sam", name: "sam-web", person: "Sam's agent", first: "Sam approves first" }] },
      { label: "Mia's agents", mine: false, agents: [{ handle: "a_mia", name: "mia-docs", person: "Mia's agent", first: null }] },
    ]);
    // The owner (`null` in `approves_first`) asks themself first: "You approve first".
    expect(sendGroups(AGENTS, "u_alex", [null])[0].agents[0].first).toBe("You approve first");
    // A connected person's browser: Sam's agents are theirs; the owner's are named.
    expect(sendGroups(AGENTS, "u_sam", [null]).map(g => [g.label, g.agents[0].first])).toEqual([["Your agents", null], ["Alex's agents", "Alex approves first"], ["Mia's agents", null]]);
    // A viewer who has no agents (a LAN viewer): every group is someone's, the owner's first.
    expect(sendGroups(AGENTS, "u_lan", []).map(g => g.label)).toEqual(["Alex's agents", "Sam's agents", "Mia's agents"]);
    // A daemon from before people names no person: its agents are this viewer's, as the one-press Send takes them.
    expect(sendGroups([{ handle: "a_old", harness: "claude", live: true }], "u_alex")).toEqual([{ label: "Your agents", mine: true, agents: [{ handle: "a_old", name: "claude", person: null, first: null }] }]);
  });

  it("names another Clax's agent with whose Clax it relays from", () => {
    const relayed = [{ handle: "a_rel", harness: "claude", live: true, name: "claude", person: { public_id: "u_alex", name: "Alex", owner: true }, via: "Alex's Clax" }];
    expect(sendGroups(relayed, "u_sam")[0].agents[0].name).toBe("claude · via Alex's Clax");
  });

  it("lists Your agents, then Sam's agents, by name, in the caret's menu", async () => {
    const onChoose = vi.fn();
    const asked = stubFirsts(["u_sam"]);
    const m = mount(SendButton, { label: "Send to clax-claude", agents: AGENTS, names, target: "a_cl", me: "u_alex", aid: "7q3k9mzx2b4t", threads: ["t1", "t2"], onSend: vi.fn(), onChoose });
    flush(() => m.root.querySelector<HTMLButtonElement>('[aria-label="Choose the agent"]')!.click());
    await settle();
    const menu = m.root.querySelector('[role="menu"]')!;
    expect([...menu.querySelectorAll('[role="group"]')].map(g => g.getAttribute("aria-label"))).toEqual(["Your agents", "Sam's agents", "Mia's agents"]);
    expect([...menu.querySelectorAll(".send-group-h")].map(h => h.textContent)).toEqual(["Your agents", "Sam's agents", "Mia's agents"]);
    const items = [...menu.querySelectorAll<HTMLButtonElement>('[role="menuitemradio"]')];
    expect(items.map(i => [i.querySelector(".send-name")!.textContent, i.getAttribute("aria-checked")])).toEqual([["clax-claude", "true"], ["clax-multi-person", "false"], ["sam-web", "false"], ["mia-docs", "false"]]);
    // Asked of the daemon as the menu opens, for the threads the Send sends.
    expect(asked).toEqual(["/api/artifacts/7q3k9mzx2b4t/approves-first?threads=t1,t2"]);
    expect(items[2].querySelector(".send-person")!.textContent).toBe("Sam's agent");
    expect(items[2].querySelector(".send-first")!.textContent).toBe("Sam approves first");
    expect(items[0].querySelector(".send-first")).toBeNull();
    expect(items[0].querySelector(".send-person")).toBeNull();
    flush(() => items[2].click());
    expect(onChoose).toHaveBeenCalledWith("a_sam");
    expect(m.root.querySelector('[role="menu"]')).toBeNull();
  });

  it("takes the keyboard: focus on the checked agent, arrows, Home and End, Escape back to the caret", async () => {
    stubFirsts([]);
    const onChoose = vi.fn();
    const m = mount(SendButton, { label: "Send to clax-claude", agents: AGENTS, names, target: "a_mp", me: "u_alex", onSend: vi.fn(), onChoose });
    const caret = m.root.querySelector<HTMLButtonElement>('[aria-label="Choose the agent"]')!;
    caret.focus();
    flush(() => caret.click());
    await settle();
    const name = () => (document.activeElement as HTMLElement).querySelector(".send-name")?.textContent;
    expect(name()).toBe("clax-multi-person");
    const press = (key: string, on: Element = document.activeElement!) => flush(() => on.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true })));
    press("ArrowDown");
    expect(name()).toBe("sam-web");
    press("End");
    expect(name()).toBe("mia-docs");
    press("ArrowDown");
    expect(name()).toBe("clax-claude");
    press("ArrowUp");
    expect(name()).toBe("mia-docs");
    press("Home");
    expect(name()).toBe("clax-claude");
    press("Escape");
    expect(m.root.querySelector('[role="menu"]')).toBeNull();
    expect(document.activeElement).toBe(caret);
    // Opened again, Escape with the focus back on the caret closes it too.
    flush(() => caret.click());
    await settle();
    caret.focus();
    press("Escape", caret);
    expect(m.root.querySelector('[role="menu"]')).toBeNull();
    // Choosing gives the focus back to the caret.
    flush(() => caret.click());
    await settle();
    flush(() => (document.activeElement as HTMLButtonElement).click());
    expect(onChoose).toHaveBeenCalledWith("a_mp");
    expect(document.activeElement).toBe(caret);
  });

  it("offers the caret for one live agent when it is another person's", () => {
    const m = mount(SendButton, { label: "Send to sam-web (Sam's)", agents: [AGENTS[0]], names, target: "a_sam", me: "u_alex", onSend: vi.fn(), onChoose: vi.fn() });
    expect(m.root.querySelector('[aria-label="Choose the agent"]')).not.toBeNull();
  });

  it("with no target while only other people's agents are live, the button opens the menu and sends nothing", async () => {
    stubFirsts(403);
    const onSend = vi.fn();
    const m = mount(SendButton, { label: "Send to …", agents: [AGENTS[0], AGENTS[3]], names, target: null, me: "u_alex", aid: "7q3k9mzx2b4t", threads: ["t1"], onSend, onChoose: vi.fn() });
    const button = m.root.querySelector<HTMLButtonElement>(".send > button.primary")!;
    expect(button.getAttribute("aria-haspopup")).toBe("menu");
    flush(() => button.click());
    await settle();
    expect(onSend).not.toHaveBeenCalled();
    const menu = m.root.querySelector('[role="menu"]')!;
    expect([...menu.querySelectorAll(".send-group-h")].map(h => h.textContent)).toEqual(["Sam's agents", "Mia's agents"]);
    // A browser the daemon refuses (403) learns nothing of who approves first.
    expect(menu.querySelector(".send-first")).toBeNull();
  });

  it("leads with ClaxBot in a Clax group of its own, saying whether it can run, and never picks it by default", async () => {
    stubFirsts([]);
    const bot = (live: boolean, line: string): A => ({ handle: "claxbot", harness: "claxbot", live, name: "ClaxBot", builtin: true, line });
    const list = [bot(true, "Runs in Clax · your key · 1,690 credits left"), AGENTS[1]];
    expect(sendGroups(list, "u_alex")[0]).toEqual({ label: "Clax", mine: false, agents: [{ handle: "claxbot", name: "ClaxBot", person: null, first: null, line: "Runs in Clax · your key · 1,690 credits left", off: false }] });
    expect(ownTarget(list, "u_alex")).toBe("a_cl");
    expect(ownTarget([list[0]], "u_alex")).toBeNull();
    const onChoose = vi.fn();
    // With one agent of the viewer's live, ClaxBot listed brings the caret.
    const m = mount(SendButton, { label: "Send to clax-claude", agents: [bot(false, "Weekly credits used · resets Mon Oct 26"), AGENTS[1]], names, target: "a_cl", me: "u_alex", onSend: vi.fn(), onChoose });
    flush(() => m.root.querySelector<HTMLButtonElement>('[aria-label="Choose the agent"]')!.click());
    await settle();
    const menu = m.root.querySelector('[role="menu"]')!;
    expect([...menu.querySelectorAll(".send-group-h")].map(h => h.textContent)).toEqual(["Clax", "Your agents"]);
    const claxbot = menu.querySelector<HTMLButtonElement>('[role="menuitemradio"]')!;
    expect(claxbot.getAttribute("aria-disabled")).toBe("true");
    expect(claxbot.querySelector(".send-line")!.textContent).toBe("Weekly credits used · resets Mon Oct 26");
    // Not available: choosing it does nothing, and the menu stays.
    flush(() => claxbot.click());
    expect(onChoose).not.toHaveBeenCalled();
    expect(m.root.querySelector('[role="menu"]')).not.toBeNull();
  });

  it("sends at once with no target when no agent is live", () => {
    const onSend = vi.fn();
    const m = mount(SendButton, { label: "Send to claude", agents: [AGENTS[4]], names, target: null, me: "u_alex", onSend, onChoose: vi.fn() });
    flush(() => m.root.querySelector<HTMLButtonElement>(".send > button.primary")!.click());
    expect(onSend).toHaveBeenCalledOnce();
  });
});
