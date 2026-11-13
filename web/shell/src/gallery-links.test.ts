import { afterEach, describe, expect, it, vi } from "vitest";
import { mount } from "./test/svelte";
import { settle, stubFetch } from "./people/fixtures";
import GalleryLinks from "./ui/GalleryLinks.svelte";
import type { MeBot } from "./bot/api";
import { meBot, week } from "./bot/fixtures";

afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); });

const links = (o: Partial<{ current: string; inbox: boolean; unread: number; queues: boolean; feedback: boolean; member: boolean; bot: MeBot | null }> = {}) =>
  ({ inbox: false, unread: 0, queues: false, feedback: false, member: false, ...o });

describe("the gallery header's links", () => {
  it("are in a fixed order, each as its answers decide, asking nothing", () => {
    const calls = stubFetch({});
    const m = mount(GalleryLinks, links({ inbox: true, unread: 3, queues: true, feedback: true, member: true }));
    expect([...m.root.querySelector(".glinks")!.children].map(e => e.className)).toEqual(["inbox-link", "queues-link", "fb-slot", "settings-link"]);
    expect(m.root.querySelector(".inbox-link")!.getAttribute("aria-label")).toBe("Inbox, 3 unread");
    expect(m.root.querySelector(".inbox-link .count")!.textContent).toBe("3");
    expect(calls).toEqual([]);
    m.update(links());
    expect(m.root.querySelector(".glinks")!.children).toHaveLength(0);
  });

  it("is a gear linking to Settings, marked current there, and opens Settings' search on the Settings key", async () => {
    stubFetch({});
    const m = mount(GalleryLinks, links({ current: "settings", member: true, queues: true }));
    const gear = m.root.querySelector<HTMLAnchorElement>("a.settings-link")!;
    expect(gear.getAttribute("href")).toBe("/settings");
    expect(gear.getAttribute("aria-label")).toBe("Settings");
    expect(gear.getAttribute("aria-current")).toBe("page");
    expect(m.root.querySelector("a.queues-link")!.hasAttribute("aria-current")).toBe(false);
    const key = new KeyboardEvent("keydown", { key: "k", metaKey: true, cancelable: true });
    dispatchEvent(key);
    expect(key.defaultPrevented).toBe(true);
    await vi.waitFor(() => expect(document.querySelector("[role=dialog][aria-label='Search settings']")).not.toBeNull());
  });

  it("leaves the Settings key alone for a browser that is no member's", async () => {
    stubFetch({});
    mount(GalleryLinks, links());
    await settle();
    const key = new KeyboardEvent("keydown", { key: "k", metaKey: true, cancelable: true });
    dispatchEvent(key);
    expect(key.defaultPrevented).toBe(false);
  });
});

describe("ClaxBot's meter in the header's links", () => {
  it("leads the links from the header's answer, in words and as a share, its name the whole sentence", () => {
    const calls = stubFetch({});
    const m = mount(GalleryLinks, links({ member: true, bot: meBot({ week: week({ spent_milli: 2_000_000 }) }) }));
    expect([...m.root.querySelector(".glinks")!.children].map(e => e.className)).toEqual(["bot-meter out", "settings-link"]);
    const meter = m.root.querySelector<HTMLAnchorElement>(".bot-meter")!;
    expect(meter.getAttribute("href")).toBe("/settings/claxbot");
    expect(meter.getAttribute("aria-label")).toBe("ClaxBot: 2,000 of 2,000 credits used this week");
    expect(meter.querySelector(".long")!.textContent).toBe("ClaxBot · 2,000 of 2,000 credits");
    expect(meter.querySelector(".short")!.textContent).toBe("100%");
    expect(meter.querySelector<HTMLElement>(".ring")!.style.getPropertyValue("--used")).toBe("100%");
    expect(calls).toEqual([]);
  });

  it("says 'your key' on the person's own key, warns from 80 %, and shows nothing while ClaxBot is off", () => {
    stubFetch({});
    const own = mount(GalleryLinks, links({ bot: meBot({ key: { own: { set: true, last4: "4f2c", set_at: null, last_ok_at: null }, use_own: true }, week: week({ spent_milli: 3_120 }) }) }));
    const meter = own.root.querySelector<HTMLAnchorElement>(".bot-meter")!;
    expect(meter.querySelector(".long")!.textContent).toBe("ClaxBot · your key · 3.12 of 2,000 credits");
    expect(meter.getAttribute("aria-label")).toBe("ClaxBot on your key: 3.12 of 2,000 credits used this week, for run fees, script compute, storage and web calls");
    expect(meter.title).toContain("pays for your tokens");
    expect(meter.className).toBe("bot-meter");
    own.unmount();
    const warn = mount(GalleryLinks, links({ bot: meBot({ week: week({ spent_milli: 1_700_000 }) }) }));
    expect(warn.root.querySelector(".bot-meter")!.className).toBe("bot-meter warn");
    expect(warn.root.querySelector(".bot-meter .short")!.textContent).toBe("85%");
    warn.unmount();
    const off = mount(GalleryLinks, links({ bot: meBot({ enabled: false }) }));
    expect(off.root.querySelector(".bot-meter")).toBeNull();
  });
});
