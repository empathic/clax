import { afterEach, describe, expect, it, vi } from "vitest";
import { mount } from "./test/svelte";
import { settle, stubFetch } from "./people/fixtures";
import GalleryLinks from "./ui/GalleryLinks.svelte";

afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); });

const links = (o: Partial<{ current: string; inbox: boolean; unread: number; queues: boolean; feedback: boolean; member: boolean }> = {}) =>
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
