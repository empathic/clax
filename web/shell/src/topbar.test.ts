import { describe, expect, it, vi } from "vitest";
import { flush, mount } from "./test/svelte";
import MoreMenu from "./ui/MoreMenu.svelte";
import PhoneTabs from "./ui/PhoneTabs.svelte";

describe("Echo top bar parts", () => {
  it("the more menu holds open raw and copy link, and closes on Escape with focus back", () => {
    const onCopy = vi.fn();
    const m = mount(MoreMenu, { rawHref: "/c/x/v/1/", canCopy: true, onCopy });
    const b = m.root.querySelector("button.icon") as HTMLButtonElement;
    expect(b.getAttribute("aria-label")).toBe("Open raw or copy link");
    expect(b.getAttribute("aria-expanded")).toBe("false");
    flush(() => b.click());
    const menu = m.root.querySelector("[role=menu]")!;
    expect(menu.querySelector("a")!.getAttribute("href")).toBe("/c/x/v/1/");
    expect(menu.querySelector("a")!.getAttribute("target")).toBe("_blank");
    flush(() => (menu.querySelector("button") as HTMLButtonElement).click());
    expect(onCopy).toHaveBeenCalled();
    flush(() => b.click());
    flush(() => m.root.querySelector("[role=menu]")!.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })));
    expect(m.root.querySelector("[role=menu]")).toBeNull();
    expect(document.activeElement).toBe(b);
    m.unmount();
  });

  it("the more menu keeps a disabled, focusable open raw for a deleted artifact", async () => {
    const m = mount(MoreMenu, { rawHref: null, canCopy: false, onCopy: vi.fn() });
    flush(() => (m.root.querySelector("button.icon") as HTMLButtonElement).click());
    await new Promise(r => setTimeout(r));
    expect(m.root.querySelector("[role=menu] a")).toBeNull();
    const raw = m.root.querySelector<HTMLButtonElement>("[role=menuitem]")!;
    expect(raw.textContent).toBe("Open raw");
    expect(raw.getAttribute("aria-disabled")).toBe("true");
    expect(document.activeElement).toBe(raw);
    m.unmount();
  });

  it("the more menu's items are out of the Tab order; Open raw closes it with focus back, and Shift+Tab closes it on its button", async () => {
    const m = mount(MoreMenu, { rawHref: "/c/x/v/1/", canCopy: true, onCopy: vi.fn() });
    const b = m.root.querySelector<HTMLButtonElement>("button.icon")!;
    flush(() => b.click());
    await new Promise(r => setTimeout(r));
    const items = Array.from(m.root.querySelectorAll<HTMLElement>("[role=menuitem]"));
    expect(items.map(i => i.getAttribute("tabindex"))).toEqual(["-1", "-1"]);
    // Open raw opens a new tab; the menu closes with focus on its button.
    const raw = items[0] as HTMLAnchorElement;
    raw.addEventListener("click", e => e.preventDefault());
    flush(() => raw.click());
    expect(m.root.querySelector("[role=menu]")).toBeNull();
    expect(document.activeElement).toBe(b);
    flush(() => b.click());
    await new Promise(r => setTimeout(r));
    const tab = new KeyboardEvent("keydown", { key: "Tab", shiftKey: true, bubbles: true, cancelable: true });
    flush(() => document.activeElement!.dispatchEvent(tab));
    expect(tab.defaultPrevented).toBe(true);
    expect(m.root.querySelector("[role=menu]")).toBeNull();
    expect(document.activeElement).toBe(b);
    m.unmount();
  });

  it("C and ? typed in the open more menu do not reach the shell's keys", async () => {
    const m = mount(MoreMenu, { rawHref: "/c/x/v/1/", canCopy: true, onCopy: vi.fn() });
    const outsideKeys = vi.fn();
    window.addEventListener("keydown", outsideKeys);
    flush(() => (m.root.querySelector("button.icon") as HTMLButtonElement).click());
    await new Promise(r => setTimeout(r));
    for (const key of ["c", "C", "?"]) flush(() => document.activeElement!.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true })));
    expect(outsideKeys).not.toHaveBeenCalled();
    expect(m.root.querySelector("[role=menu]")).not.toBeNull();
    window.removeEventListener("keydown", outsideKeys);
    m.unmount();
  });

  it("the more menu moves between its items with the arrow keys, keeps them from the shell, and closes when focus leaves it", async () => {
    const m = mount(MoreMenu, { rawHref: "/c/x/v/1/", canCopy: true, onCopy: vi.fn() });
    const outsideKeys = vi.fn();
    window.addEventListener("keydown", outsideKeys);
    flush(() => (m.root.querySelector("button.icon") as HTMLButtonElement).click());
    // Opening focuses the first item once the menu is drawn.
    await new Promise(r => setTimeout(r));
    const [raw, copy] = Array.from(m.root.querySelectorAll<HTMLElement>("[role=menuitem]"));
    expect(document.activeElement).toBe(raw);
    const press = (key: string) => flush(() => (document.activeElement as HTMLElement).dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true })));
    press("ArrowDown");
    expect(document.activeElement).toBe(copy);
    press("ArrowDown");
    expect(document.activeElement).toBe(raw);
    press("End");
    expect(document.activeElement).toBe(copy);
    press("ArrowUp");
    expect(document.activeElement).toBe(raw);
    expect(outsideKeys).not.toHaveBeenCalled();
    const elsewhere = document.body.appendChild(document.createElement("button"));
    flush(() => elsewhere.focus());
    expect(m.root.querySelector("[role=menu]")).toBeNull();
    window.removeEventListener("keydown", outsideKeys);
    elsewhere.remove();
    m.unmount();
  });

  it("phone tabs are a labelled group that switches between the page and the threads", () => {
    const onPage = vi.fn();
    const onThreads = vi.fn();
    const props = { panel: false, open: 3, onPage, onThreads };
    const m = mount(PhoneTabs, props);
    const group = m.root.querySelector("[role=group]")!;
    expect(group.getAttribute("aria-label")).toBe("Page or threads");
    expect(m.root.querySelector("nav")).toBeNull();
    const [page, threads] = Array.from(group.querySelectorAll("button"));
    expect(page.getAttribute("aria-pressed")).toBe("true");
    expect(threads.getAttribute("aria-pressed")).toBe("false");
    expect(threads.textContent).toContain("3");
    flush(() => threads.click());
    expect(onThreads).toHaveBeenCalledOnce();
    m.update({ ...props, panel: true });
    expect(page.getAttribute("aria-pressed")).toBe("false");
    expect(threads.getAttribute("aria-pressed")).toBe("true");
    flush(() => page.click());
    expect(onPage).toHaveBeenCalledOnce();
    m.unmount();
  });
});
