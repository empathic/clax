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

  it("the more menu shows open raw as plain text for a deleted artifact", () => {
    const m = mount(MoreMenu, { rawHref: null, canCopy: false, onCopy: vi.fn() });
    flush(() => (m.root.querySelector("button.icon") as HTMLButtonElement).click());
    expect(m.root.querySelector("[role=menu] a")).toBeNull();
    expect(m.root.querySelector("[role=menu]")!.textContent).toContain("Open raw");
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

  it("phone tabs switch between the page and the threads", () => {
    const onPage = vi.fn();
    const onThreads = vi.fn();
    const m = mount(PhoneTabs, { panel: false, open: 3, onPage, onThreads });
    const [page, threads] = Array.from(m.root.querySelectorAll("button"));
    expect(page.getAttribute("aria-pressed")).toBe("true");
    expect(threads.textContent).toContain("3");
    flush(() => threads.click());
    expect(onThreads).toHaveBeenCalled();
    m.unmount();
  });
});
