import { afterEach, describe, expect, it } from "vitest";
import { htmlHash, fingerprint } from "../src/anchor";
import { keepOnTop, topModal } from "../src/top-layer";

// jsdom has no top layer: a dialog here is modal while it is open with
// data-modal, as `showModal()` makes it in a browser.
function dialog(): HTMLDialogElement {
  const d = document.createElement("dialog");
  const own = Element.prototype.matches;
  d.matches = (s: string) => (s === ":modal" ? d.hasAttribute("open") && d.hasAttribute("data-modal") : own.call(d, s));
  d.innerHTML = `<p>Send feedback</p>`;
  document.body.append(d);
  return d;
}
const open = (d: HTMLDialogElement) => { d.setAttribute("data-modal", ""); d.setAttribute("open", ""); };
const settle = () => new Promise(r => setTimeout(r, 0));

afterEach(() => { document.body.replaceChildren(); for (const h of document.querySelectorAll("clax-overlay")) h.remove(); });

describe("keepOnTop", () => {
  it("moves the host into the page's topmost modal dialog while it is open, and back to <html> after", async () => {
    const host = document.createElement("clax-overlay");
    host.setAttribute("popover", "manual");
    const top = keepOnTop(document, host);
    top.place();
    expect(host.parentNode).toBe(document.documentElement);
    const a = dialog(), b = dialog();
    open(a);
    await settle();
    expect(host.parentNode).toBe(a);
    // Two open: the last in document order.
    open(b);
    await settle();
    expect([host.parentNode, top.parent()]).toEqual([b, b]);
    b.removeAttribute("open");
    await settle();
    expect(host.parentNode).toBe(a);
    a.removeAttribute("open");
    await settle();
    expect(host.parentNode).toBe(document.documentElement);
    expect(document.documentElement.lastElementChild).toBe(host);
    top.stop();
  });

  it("puts the host back when the page removes the dialog it was in, and does nothing once stopped", async () => {
    const host = document.createElement("clax-overlay");
    const top = keepOnTop(document, host);
    top.place();
    const d = dialog();
    open(d);
    await settle();
    expect(host.parentNode).toBe(d);
    d.remove();
    await settle();
    expect(host.parentNode).toBe(document.documentElement);
    top.stop();
    const e = dialog();
    open(e);
    await settle();
    expect(host.parentNode).toBe(document.documentElement);
  });

  it("names no modal for a dialog shown without showModal", () => {
    const d = dialog();
    d.setAttribute("open", "");
    expect(topModal(document)).toBeNull();
  });
});

describe("anchors of a dialog the host sits in", () => {
  it("leave the host out of the hash and the fingerprint", () => {
    const d = dialog();
    const before = [htmlHash(d), fingerprint(d)];
    d.append(document.createElement("clax-overlay"));
    expect([htmlHash(d), fingerprint(d)]).toEqual(before);
  });
});
