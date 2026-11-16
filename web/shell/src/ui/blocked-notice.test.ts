import { afterEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
import BlockedNotice from "./BlockedNotice.svelte";

const mounted: object[] = [];
afterEach(() => { for (const m of mounted.splice(0)) unmount(m); document.body.replaceChildren(); });
function show(props: { origins: string[]; admin: boolean; own?: boolean; permissions?: boolean }) {
  const onPermissions = vi.fn(), onReload = vi.fn(), onDismiss = vi.fn();
  const target = document.createElement("div");
  document.body.append(target);
  mounted.push(mount(BlockedNotice, { target, props: { origins: props.origins, admin: props.admin, own: props.own ?? false, onPermissions: props.permissions === false ? null : onPermissions, onReload, onDismiss } }));
  flushSync();
  const buttons = () => [...target.querySelectorAll("button")].map(b => b.textContent);
  return { target, buttons, onPermissions, onReload, onDismiss };
}

describe("BlockedNotice", () => {
  it("names the sites to an administrator, and its button opens the Permissions panel and changes nothing", () => {
    const n = show({ origins: ["https://cdn.example.com", "wss://relay.example"], admin: true });
    expect(n.target.textContent).toContain("This clax tried to reach 2 other sites, and Clax blocked it.");
    expect(n.target.querySelector("[data-testid=blocked-sites]")!.textContent).toBe("https://cdn.example.com, wss://relay.example");
    expect(n.buttons()).toEqual(["Permissions…", "Dismiss"]);
    n.target.querySelector<HTMLButtonElement>("button")!.click();
    expect(n.onPermissions).toHaveBeenCalledOnce();
    expect(n.onReload).not.toHaveBeenCalled();
  });
  it("gives anyone else one line with no button but Dismiss", () => {
    const n = show({ origins: ["https://cdn.example.com"], admin: false });
    expect(n.target.textContent).toContain("This page tried to load from other sites, and Clax blocked it.");
    expect(n.target.textContent).not.toContain("cdn.example.com");
    expect(n.buttons()).toEqual(["Dismiss"]);
    n.target.querySelector<HTMLButtonElement>("button")!.click();
    expect(n.onDismiss).toHaveBeenCalledOnce();
  });
  it("renders what the page sent as text", () => {
    const n = show({ origins: ["<img src=x onerror=alert(1)>"], admin: true });
    expect(n.target.querySelector("img")).toBeNull();
    expect(n.target.querySelector("[data-testid=blocked-sites]")!.textContent).toBe("<img src=x onerror=alert(1)>");
  });
  it("offers an administrator Reload once the clax sets its own policy, and anyone else nothing", () => {
    const n = show({ origins: ["https://cdn.example.com"], admin: true, own: true });
    expect(n.buttons()).toEqual(["Reload", "Dismiss"]);
    n.target.querySelector<HTMLButtonElement>("button")!.click();
    expect(n.onReload).toHaveBeenCalledOnce();
    expect(show({ origins: ["https://cdn.example.com"], admin: false, own: true }).target.querySelector("[data-blocked-notice]")).toBeNull();
  });
  it("offers no Permissions… where the panel cannot open", () => {
    expect(show({ origins: ["https://cdn.example.com"], admin: true, permissions: false }).buttons()).toEqual(["Dismiss"]);
  });
});
