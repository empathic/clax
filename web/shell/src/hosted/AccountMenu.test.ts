import { afterEach, describe, expect, it, vi } from "vitest";
import { flush, mount } from "../test/svelte";
import { settle, stubFetch } from "../people/fixtures";
import AccountMenu from "./AccountMenu.svelte";

afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); });

const items = (root: HTMLElement) => [...root.querySelectorAll<HTMLElement>("[role=menuitem]")];
const key = (el: Element, k: string) => { const e = new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true }); flush(() => { el.dispatchEvent(e); }); return e; };

describe("AccountMenu", () => {
  it("names who the browser is seen as, and opens on Your account and Sign out with the keys of the header's menus", async () => {
    const m = mount(AccountMenu, { name: "Alex Kesling" });
    const btn = m.root.querySelector<HTMLButtonElement>("button.acct-btn")!;
    expect(btn.textContent?.replace(/\s+/g, " ").trim()).toBe("seen as Alex Kesling");
    expect(btn.getAttribute("aria-haspopup")).toBe("menu");
    expect(btn.getAttribute("aria-expanded")).toBe("false");
    expect(m.root.querySelector("[role=menu]")).toBeNull();

    btn.click();
    await settle();
    expect(btn.getAttribute("aria-expanded")).toBe("true");
    const [account, out] = items(m.root);
    expect(items(m.root).map(i => i.textContent)).toEqual(["Your account", "Sign out"]);
    expect(account.getAttribute("href")).toBe("/people");
    expect(document.activeElement).toBe(account);
    key(account, "ArrowDown");
    expect(document.activeElement).toBe(out);
    key(out, "ArrowDown");
    expect(document.activeElement).toBe(account);
    key(account, "End");
    expect(document.activeElement).toBe(out);
    expect(key(out, "f").defaultPrevented, "a printable key is not the menu's").toBe(false);
    key(out, "Escape");
    expect(m.root.querySelector("[role=menu]")).toBeNull();
    expect(document.activeElement).toBe(btn);
  });

  it("offers a guest Sign out alone", async () => {
    const m = mount(AccountMenu, { name: "Sam", guest: true });
    m.root.querySelector<HTMLButtonElement>("button.acct-btn")!.click();
    await settle();
    expect(items(m.root).map(i => i.textContent)).toEqual(["Sign out"]);
  });

  it("signs out, forgetting the reporter first, and lands on the sign-in page", async () => {
    const calls = stubFetch({ "POST /api/hosted/signout": {} });
    const go = vi.fn();
    const m = mount(AccountMenu, { name: "Alex", go });
    m.root.querySelector<HTMLButtonElement>("button.acct-btn")!.click();
    await settle();
    items(m.root).find(i => i.textContent === "Sign out")!.click();
    await vi.waitFor(() => { expect(go).toHaveBeenCalledWith("/signin"); });
    const signout = calls.findIndex(c => c.method === "POST" && c.url === "/api/hosted/signout");
    expect(signout).toBeGreaterThanOrEqual(0);
    const config = calls.findIndex(c => c.url === "/_app/feedback/config");
    expect(config, "the reporter's config is read before the session ends").toBeGreaterThanOrEqual(0);
    expect(config).toBeLessThan(signout);
  });

  it("shows a refused sign-out in the menu and stays on the page", async () => {
    stubFetch({ "POST /api/hosted/signout": [403, { error: { code: "forbidden", message: "the public origin did not send this" } }] });
    const go = vi.fn();
    const m = mount(AccountMenu, { name: "Alex", go });
    m.root.querySelector<HTMLButtonElement>("button.acct-btn")!.click();
    await settle();
    items(m.root).find(i => i.textContent === "Sign out")!.click();
    await vi.waitFor(() => { expect(m.root.querySelector("[role=alert]")).not.toBeNull(); });
    expect(m.root.querySelector("[role=alert]")!.textContent).toBe("The public origin did not send this.");
    expect(go).not.toHaveBeenCalled();
  });
});
