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

  // Re-inserting the host reloads the composer's iframe and loses the pick
  // ("Clax stopped listening to this comment"): Clax's own pages toggle
  // `open` on <details> and non-modal dialogs all the time.
  const reinsertions = (host: Element) => {
    const seen: Node[] = [];
    const mo = new MutationObserver(rs => { for (const r of rs) for (const n of r.removedNodes) if (n === host) seen.push(n); });
    mo.observe(document, { subtree: true, childList: true });
    return { count: async () => { await settle(); mo.disconnect(); return seen.length; } };
  };

  it("never re-inserts the host when the page toggles a <details> or a non-modal <dialog> while a composer is open", async () => {
    const host = document.createElement("clax-overlay");
    const frame = document.createElement("iframe");
    host.append(frame);
    let placed = 0;
    const top = keepOnTop(document, host, () => { placed++; }, () => true);
    top.place();
    expect(placed).toBe(1);
    // The page adds a node after the host: it stays where it is.
    const after = document.createElement("div");
    document.documentElement.append(after);
    const r = reinsertions(host);
    const details = document.createElement("details");
    details.innerHTML = "<summary>Versions</summary><p>v5</p>";
    document.body.append(details);
    details.setAttribute("open", "");
    await settle();
    details.removeAttribute("open");
    const panel = document.createElement("dialog");
    document.body.append(panel);
    panel.setAttribute("open", "");
    await settle();
    panel.removeAttribute("open");
    expect(await r.count()).toBe(0);
    expect([placed, host.parentNode, frame.parentNode, top.held()]).toEqual([1, document.documentElement, host, false]);
    top.stop();
    after.remove();
  });

  it("holds the host out of a modal while a composer is open without moveBefore, and moves it once the composer closes", async () => {
    const host = document.createElement("clax-overlay");
    let composing = true;
    const top = keepOnTop(document, host, () => {}, () => composing);
    top.place();
    const r = reinsertions(host);
    const d = dialog();
    // A browser without moveBefore, whatever this jsdom has.
    (d as unknown as { moveBefore?: unknown }).moveBefore = undefined;
    open(d);
    await settle();
    expect(await r.count()).toBe(0);
    expect([host.parentNode, top.held()]).toEqual([document.documentElement, true]);
    composing = false;
    top.place();
    expect([host.parentNode, top.held()]).toEqual([d, false]);
    top.stop();
  });

  it("moves the host with moveBefore, keeping its frame, even while a composer is open", async () => {
    const host = document.createElement("clax-overlay");
    const top = keepOnTop(document, host, () => {}, () => true);
    top.place();
    const d = dialog();
    const moves: Node[] = [];
    (d as unknown as { moveBefore: (n: Node, c: Node | null) => void }).moveBefore = (n, c) => { moves.push(n); d.insertBefore(n, c); };
    open(d);
    await settle();
    expect([moves, host.parentNode, top.held()]).toEqual([[host], d, false]);
    top.stop();
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
