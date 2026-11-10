import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { dispatchTrusted } from "../../../bridge/test/trusted";
import { settle, stubFetch } from "../people/fixtures";
import { flush, mount } from "../test/svelte";
import type { Ctx } from "./screens";

// The browser's context is given, not asked: each test names who it is.
let who: Ctx = { hosted: false, owner: true, member: true, admin: false, bot: false };
vi.mock("./context", () => ({ context: () => Promise.resolve(who) }));
const { default: Settings } = await import("./Settings.svelte");
const { default: Palette } = await import("./Palette.svelte");
const { GO, go, isPaletteKey } = await import("./palette");

const click = (el: Element) => flush(() => { dispatchTrusted(el, new MouseEvent("click", { bubbles: true, cancelable: true, button: 0 })); });
const type = (el: HTMLInputElement, text: string) => flush(() => { el.value = text; el.dispatchEvent(new Event("input", { bubbles: true })); });
const press = (el: Element, key: string) => flush(() => { el.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true })); });
const options = (root: ParentNode) => [...root.querySelectorAll("[role=option]")].map(o => o.querySelector(".t")!.textContent);

beforeEach(() => { who = { hosted: false, owner: true, member: true, admin: false, bot: false }; history.replaceState(null, "", "/settings"); });
afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); });

describe("Settings", () => {
  it("is an overview of the screens this browser sees, by group, each a link with a line", async () => {
    stubFetch({});
    const m = mount(Settings, {});
    await settle();
    expect(m.root.querySelector("h2")!.textContent).toBe("Settings");
    expect([...m.root.querySelectorAll(".stg-over h3")].map(h => h.textContent)).toEqual(["You", "People", "System"]);
    const cards = [...m.root.querySelectorAll<HTMLAnchorElement>(".stg-cards a")];
    expect(cards.map(a => a.getAttribute("href"))).toEqual(["/settings/agents", "/settings/confirmations", "/settings/people", "/settings/config", "/settings/remotes", "/settings/audit"]);
    expect(cards[2].textContent).toContain("Who is connected to this Clax");
    expect(document.title).toBe("Settings · Clax");
  });

  it("searches every screen from the overview, and Enter opens what is marked, in place", async () => {
    stubFetch({ "GET /api/remotes/mutes": { mutes: [{ origin: "https://clax.example" }] } });
    const m = mount(Settings, {});
    await settle();
    const field = m.root.querySelector<HTMLInputElement>("[role=combobox]")!;
    type(field, "muted");
    expect(options(m.root)).toEqual(["Muted instances"]);
    // The overview's list gives way to what was found.
    expect(m.root.querySelector(".stg-cards")).toBeNull();
    expect(field.getAttribute("aria-activedescendant")).toBe(m.root.querySelector("[role=option]")!.id);
    press(field, "Enter");
    expect(location.pathname + location.hash).toBe("/settings/remotes#sec-remotes");
    await vi.waitFor(() => expect(m.root.textContent).toContain("https://clax.example"));
    // The screen shows beside the navigation, which marks it; the heading the result named has the focus.
    expect(m.root.querySelector(".stg-nav [aria-current=page]")!.textContent).toBe("Remotes");
    expect(document.activeElement?.id).toBe("sec-remotes");
  });

  it("says when nothing matches", async () => {
    stubFetch({});
    const m = mount(Settings, {});
    await settle();
    type(m.root.querySelector<HTMLInputElement>("[role=combobox]")!, "zebra");
    expect(options(m.root)).toEqual([]);
    expect(m.root.querySelector("[role=status]")!.textContent).toBe("Nothing in Settings matches.");
  });

  it("moves between screens in place by the navigation, and back by the browser's history", async () => {
    history.replaceState(null, "", "/settings/remotes");
    stubFetch({ "GET /api/remotes/mutes": { mutes: [] } });
    const m = mount(Settings, {});
    await vi.waitFor(() => expect(m.root.querySelector(".bpg h2")?.textContent).toBe("Remotes"));
    click(m.root.querySelector(".stg-nav a[href='/settings/confirmations']")!);
    expect(location.pathname).toBe("/settings/confirmations");
    await vi.waitFor(() => expect(m.root.querySelector(".bpg h2")?.textContent).toBe("Confirmations"));
    history.replaceState(null, "", "/settings/remotes");
    flush(() => { dispatchEvent(new PopStateEvent("popstate")); });
    await vi.waitFor(() => expect(m.root.querySelector(".bpg h2")?.textContent).toBe("Remotes"));
    // Settings' name leads back to the overview.
    click(m.root.querySelector(".stg-home")!);
    expect(location.pathname).toBe("/settings");
    expect(m.root.querySelector(".stg-over")).not.toBeNull();
  });

  it("shows the overview, saying so, for a screen this browser may not open", async () => {
    who = { hosted: true, owner: false, member: true, admin: false, bot: false };
    history.replaceState(null, "", "/settings/provider");
    stubFetch({});
    const m = mount(Settings, {});
    await settle();
    expect(m.root.querySelector(".stg-over")!.textContent).toContain("not one this browser can open");
    expect([...m.root.querySelectorAll(".stg-cards a")].map(a => a.getAttribute("href"))).toEqual(["/settings/account", "/settings/agents", "/settings/sharing", "/settings/audit"]);
  });

  it("shows the screen named instead of one that is not this browser's, at that screen's address", async () => {
    who = { hosted: false, owner: false, member: true, admin: false, bot: false };
    history.replaceState(null, "", "/settings/people?x=1");
    stubFetch({
      "GET /api/people/me": { person: { public_id: "p1", display_name: "Sam", owner: false, rights: [] }, grants: [], agents: [] },
      "GET /api/approvals": { approvals: [] },
      "GET /api/approvals/requests": { requests: [], open: 0, banner: null },
    });
    const m = mount(Settings, {});
    await vi.waitFor(() => expect(m.root.querySelector(".ppl h2")?.textContent).toBe("Agents & machines"));
    expect(location.pathname + location.search).toBe("/settings/agents?x=1");
  });

  it("tells a browser that is no member's that Settings are not its", async () => {
    who = { hosted: false, owner: false, member: false, admin: false, bot: false };
    history.replaceState(null, "", "/settings/agents");
    stubFetch({});
    const m = mount(Settings, {});
    await settle();
    expect(m.root.querySelector(".stg-note")!.textContent).toBe("Settings are for the owner of this Clax and the people they connect.");
    expect(m.root.querySelector(".stg-cards")).toBeNull();
  });

  it("ends its navigation and its overview with Sign out on a hosted Clax, which ends the session", async () => {
    who = { hosted: true, owner: false, member: true, admin: false, bot: false };
    const calls = stubFetch({ "POST /api/hosted/signout": {} });
    const m = mount(Settings, {});
    await settle();
    const out = [...m.root.querySelectorAll<HTMLButtonElement>(".stg-over button")].find(b => b.textContent === "Sign out")!;
    click(out);
    await vi.waitFor(() => expect(calls.some(c => c.method === "POST" && c.url === "/api/hosted/signout")).toBe(true));
  });

  it("says why when signing out is refused, and has no Sign out on a Clax on this machine", async () => {
    who = { hosted: true, owner: false, member: true, admin: false, bot: false };
    stubFetch({ "POST /api/hosted/signout": [403, { error: { code: "forbidden", message: "the public origin did not send this" } }] });
    const m = mount(Settings, {});
    await settle();
    click([...m.root.querySelectorAll<HTMLButtonElement>(".stg-over button")].find(b => b.textContent === "Sign out")!);
    await vi.waitFor(() => expect(m.root.querySelector(".stg-out [role=alert]")?.textContent).toBe("The public origin did not send this."));
    m.unmount();
    who = { hosted: false, owner: true, member: true, admin: false, bot: false };
    const local = mount(Settings, {});
    await settle();
    expect(local.root.querySelector(".stg-out")).toBeNull();
  });

  it("takes an address the search dialog chose, in place", async () => {
    stubFetch({ "GET /api/remotes/mutes": { mutes: [] } });
    const m = mount(Settings, {});
    await settle();
    flush(() => go("/settings/remotes"));
    expect(location.pathname).toBe("/settings/remotes");
    await vi.waitFor(() => expect(m.root.querySelector(".bpg h2")?.textContent).toBe("Remotes"));
  });
});

describe("Settings' search dialog", () => {
  it("lists every screen at once, moves by the arrow keys, and opens the one marked", () => {
    const went: string[] = [];
    let closed = 0;
    const m = mount(Palette, { ctx: who, onGo: (h: string) => { went.push(h); }, onClose: () => { closed += 1; } });
    const field = m.root.querySelector<HTMLInputElement>("[role=combobox]")!;
    expect(document.activeElement).toBe(field);
    expect(m.root.querySelector("[role=dialog]")!.getAttribute("aria-modal")).toBe("true");
    expect(options(m.root)).toEqual(["Agents & machines", "Confirmations", "People", "Configuration", "Remotes", "Audit log", "Queues", "Tags"]);
    press(field, "ArrowDown");
    press(field, "ArrowDown");
    expect(m.root.querySelector("[aria-selected=true] .t")!.textContent).toBe("People");
    press(field, "End");
    press(field, "ArrowUp");
    expect(m.root.querySelector("[aria-selected=true] .t")!.textContent).toBe("Queues");
    press(field, "Home");
    press(field, "Enter");
    expect(went).toEqual(["/settings/agents"]);
    expect(closed).toBe(1);
  });

  it("closes on Escape and on a click outside it", () => {
    let closed = 0;
    const m = mount(Palette, { ctx: who, onGo: () => {}, onClose: () => { closed += 1; } });
    press(m.root.querySelector("[role=combobox]")!, "Escape");
    click(m.root.querySelector(".spal-backdrop")!);
    expect(closed).toBe(2);
  });

  it("opens by Cmd+K or Ctrl+K alone", () => {
    const key = (o: KeyboardEventInit) => isPaletteKey(new KeyboardEvent("keydown", o));
    expect(key({ key: "k", metaKey: true })).toBe(true);
    expect(key({ key: "K", ctrlKey: true })).toBe(true);
    expect(key({ key: "k" })).toBe(false);
    expect(key({ key: "k", metaKey: true, shiftKey: true })).toBe(false);
    expect(key({ key: "j", metaKey: true })).toBe(false);
  });

  it("loads the address when Settings is not the page shown", () => {
    const assign = vi.fn();
    vi.stubGlobal("location", { ...location, assign });
    go("/settings/people");
    expect(assign).toHaveBeenCalledWith("/settings/people");
    // With Settings listening, the event is taken and nothing loads.
    const take = (e: Event) => e.preventDefault();
    addEventListener(GO, take);
    go("/settings/audit");
    removeEventListener(GO, take);
    expect(assign).toHaveBeenCalledTimes(1);
  });
});
