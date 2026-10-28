import { afterEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
import ApprovalDialog from "./ApprovalDialog.svelte";

const pending = { version: 2, capabilities: ["net"], hosts: ["a.clax.invalid"], declaration_sha256: "a".repeat(64) };
const open: { unmount(): void }[] = [];
afterEach(() => { for (const m of open.splice(0)) m.unmount(); document.body.replaceChildren(); });

function show(over: Partial<{ busy: boolean; owner: boolean }> = {}) {
  const outside = document.createElement("button");
  outside.textContent = "outside";
  document.body.append(outside);
  outside.focus();
  const calls = { approve: vi.fn(), notNow: vi.fn(), close: vi.fn() };
  const target = document.createElement("div");
  document.body.append(target);
  const c = mount(ApprovalDialog, { target, props: { pending, title: "Forecast", busy: false, owner: true, ...over, onApprove: calls.approve, onNotNow: calls.notNow, onClose: calls.close } });
  open.push({ unmount: () => unmount(c) });
  flushSync();
  const dialog = document.querySelector<HTMLElement>("[role=dialog]")!;
  const button = (name: string) => [...dialog.querySelectorAll("button")].find(b => b.textContent === name)!;
  return { outside, dialog, button, calls, unmount: () => { unmount(c); open.pop(); flushSync(); } };
}
const key = (el: Element, k: string, shiftKey = false) => { const e = new KeyboardEvent("keydown", { key: k, shiftKey, bubbles: true, cancelable: true }); el.dispatchEvent(e); return e; };

describe("ApprovalDialog", () => {
  it("is titled with the artifact, lists what is asked, and starts on Not now", () => {
    const d = show();
    expect(d.dialog.querySelector("h2")!.textContent).toBe("Let “Forecast” use these?");
    expect(d.dialog.textContent).toContain("the internet (a.clax.invalid)");
    expect(document.activeElement).toBe(d.button("Not now"));
  });

  it("keeps Tab among its buttons and makes the rest of the document inert", () => {
    const d = show();
    expect(d.outside.closest("[inert]")).not.toBeNull();
    const notNow = d.button("Not now"), approve = d.button("Approve");
    // Tab moves on within the dialog; from the last button it wraps to the first, and Shift+Tab from the first to the last.
    expect(key(notNow, "Tab").defaultPrevented).toBe(false);
    approve.focus();
    expect(key(approve, "Tab").defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(notNow);
    expect(key(notNow, "Tab", true).defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(approve);
  });

  it("closes on Escape and gives focus back, undoing the inert", () => {
    const d = show();
    key(d.dialog, "Escape");
    expect(d.calls.close).toHaveBeenCalledTimes(1);
    d.unmount();
    expect(d.outside.closest("[inert]")).toBeNull();
    expect(document.activeElement).toBe(d.outside);
  });

  it("Not now closes and hides the bar; Approve closes and approves", () => {
    const a = show();
    a.button("Not now").click();
    expect([a.calls.close.mock.calls.length, a.calls.notNow.mock.calls.length, a.calls.approve.mock.calls.length]).toEqual([1, 1, 0]);
    a.unmount();
    const b = show();
    b.button("Approve").click();
    expect([b.calls.close.mock.calls.length, b.calls.approve.mock.calls.length]).toEqual([1, 1]);
  });
});
