import { beforeEach, describe, expect, it } from "vitest";
import { MAX_FAR, MAX_WAIT_MS, type Placed, QUIET_MS, Resolver, type Timers } from "./resolver";

/** Timers a test advances by hand. */
function manual() {
  let now = 0;
  let queue: { at: number; fn: () => void }[] = [];
  const frames: (() => void)[] = [];
  const t: Timers & { advance(ms: number): void; now(): number } = {
    now: () => now,
    set: (fn, ms) => { const e = { at: now + ms, fn }; queue.push(e); return e; },
    clear: h => { queue = queue.filter(e => e !== h); },
    frame: fn => { frames.push(fn); },
    advance(ms) {
      now += ms;
      for (const e of queue.filter(q => q.at <= now)) { queue = queue.filter(x => x !== e); e.fn(); }
      while (frames.length) frames.shift()!();
    },
  };
  return t;
}
const flush = () => new Promise(r => setTimeout(r, 0)); // lets MutationObserver records arrive
const records = () => new Promise<void>(r => queueMicrotask(r)); // after the observer's own microtask
const thread = (id: string, selector: string, quote: string | null, route?: string) => ({
  id, status: "open", anchor: { kind: "element", selector, quote, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html", ...(route ? { route } : {}) },
}) as never;

let placed: Placed[] = [];
beforeEach(() => {
  document.querySelectorAll("clax-overlay").forEach(n => n.remove());
  document.body.innerHTML = `<div id="app"><main><button id="save">Save</button></main></div>`;
  placed = [];
});

describe("Resolver", () => {
  it("shows only the current route's threads", () => {
    const t = manual();
    const r = new Resolver(document, p => { placed = p; }, t);
    r.set([thread("01J9AAAAAAAAAAAAAAAAAAAAAA", "#save", "Save"), thread("01J9BBBBBBBBBBBBBBBBBBBBBB", "#save", "Save", "?tab=billing")], null);
    t.advance(0);
    expect(placed.map(p => p.id)).toEqual(["01J9AAAAAAAAAAAAAAAAAAAAAA"]);
    r.set(r.threads, "?tab=billing");
    t.advance(0);
    expect(placed.map(p => p.id)).toEqual(["01J9BBBBBBBBBBBBBBBBBBBBBB"]);
    r.stop();
  });

  it("resolves another page's threads whatever the route, after the page's own, with the path they were left at", () => {
    const t = manual();
    const r = new Resolver(document, p => { placed = p; }, t);
    const far = { ...(thread("01J9BBBBBBBBBBBBBBBBBBBBBB", "#save", "Save", "?tab=billing") as object), from: "/users/7" } as never;
    r.set([thread("01J9AAAAAAAAAAAAAAAAAAAAAA", "#save", "Save"), far], null);
    t.advance(0);
    expect(placed.map(p => [p.id, p.n, p.from])).toEqual([["01J9AAAAAAAAAAAAAAAAAAAAAA", 1, undefined], ["01J9BBBBBBBBBBBBBBBBBBBBBB", 2, "/users/7"]]);
    r.stop();
  });

  it("resolves the page's own threads first, and at most MAX_FAR of the other pages'", () => {
    const t = manual();
    const r = new Resolver(document, p => { placed = p; }, t);
    const far = Array.from({ length: 300 }, (_, i) => ({ ...(thread(`01J9F${String(i).padStart(21, "0")}`, "#save", "Save") as object), from: "/x" }) as never);
    r.set([...far, thread("01J9AAAAAAAAAAAAAAAAAAAAAA", "#save", "Save")], null);
    t.advance(0);
    expect(placed).toHaveLength(1 + MAX_FAR);
    expect(placed[0].id).toBe("01J9AAAAAAAAAAAAAAAAAAAAAA");
    r.stop();
  });

  it("re-resolves after a wholesale replacement and detaches what is gone", async () => {
    const t = manual();
    const r = new Resolver(document, p => { placed = p; }, t);
    r.set([thread("01J9AAAAAAAAAAAAAAAAAAAAAA", "div#app > main > button", "Save")], null);
    t.advance(0);
    expect(placed[0].box).not.toBeNull();
    document.querySelector("#app")!.innerHTML = `<main><button id="save">Save changes</button></main>`;
    await flush();
    t.advance(QUIET_MS - 1);
    expect(placed[0].box).not.toBeNull();
    t.advance(1);
    expect(placed[0].box).not.toBeNull();
    document.querySelector("#app")!.innerHTML = `<main><p>No button</p></main>`;
    await flush();
    t.advance(QUIET_MS);
    expect(placed[0].box).toBeNull();
    expect(placed[0].n).toBeNull();
    r.stop();
  });

  it("waits for the DOM to be quiet: each mutation restarts the wait", async () => {
    const t = manual();
    let runs = 0;
    const r = new Resolver(document, () => { runs++; }, t);
    r.set([], null);
    t.advance(0);
    document.body.appendChild(document.createElement("p"));
    await flush();
    t.advance(QUIET_MS - 1);
    document.body.appendChild(document.createElement("p"));
    await flush();
    t.advance(QUIET_MS - 1);
    expect(runs).toBe(1);
    t.advance(1);
    expect(runs).toBe(2);
    r.stop();
  });

  it("resolves a page that never goes quiet at most MAX_WAIT_MS after the first change", async () => {
    const t = manual();
    const at: number[] = [];
    const r = new Resolver(document, () => { at.push(t.now()); }, t);
    r.set([], null);
    t.advance(0);
    const clock = document.createElement("p");
    document.body.appendChild(clock);
    await records();
    for (let i = 0; i < 100; i++) {
      clock.textContent = String(i);
      await records();
      expect(r.busy).toBe(true);
      t.advance(50);
    }
    const runs = at.slice(1);
    expect(runs.length).toBeGreaterThanOrEqual(4);
    expect(runs.length).toBeLessThanOrEqual(5);
    expect(runs[0]).toBeLessThanOrEqual(MAX_WAIT_MS);
    for (let i = 1; i < runs.length; i++) expect(runs[i] - runs[i - 1]).toBeLessThanOrEqual(MAX_WAIT_MS + 50);
    t.advance(QUIET_MS);
    expect(r.busy).toBe(false);
    r.stop();
  });

  it("ignores mutations it is told to", async () => {
    const t = manual();
    const host = document.createElement("clax-overlay");
    document.documentElement.appendChild(host);
    let runs = 0;
    const r = new Resolver(document, () => { runs++; }, t, n => host === n || host.contains(n));
    r.set([], null);
    t.advance(0);
    host.setAttribute("data-x", "1");
    await flush();
    t.advance(QUIET_MS);
    expect(runs).toBe(1);
    r.stop();
  });

  it("by default ignores every clax-overlay element, including one moved or added", async () => {
    const t = manual();
    const a = document.createElement("clax-overlay");
    document.documentElement.appendChild(a);
    let runs = 0;
    const r = new Resolver(document, () => { runs++; }, t);
    r.set([], null);
    t.advance(0);
    a.style.visibility = "hidden";
    document.documentElement.appendChild(document.createElement("clax-overlay"));
    document.documentElement.appendChild(a); // moved last, as comment mode does
    await flush();
    t.advance(QUIET_MS);
    expect(runs).toBe(1);
    document.body.appendChild(document.createElement("p"));
    await flush();
    t.advance(QUIET_MS);
    expect(runs).toBe(2);
    r.stop();
  });

  it("numbers the found threads in order and measures without resolving again", () => {
    const t = manual();
    const r = new Resolver(document, p => { placed = p; }, t);
    r.set([
      thread("01J9AAAAAAAAAAAAAAAAAAAAAA", "#gone", "Nowhere to be found"),
      thread("01J9BBBBBBBBBBBBBBBBBBBBBB", "#save", "Save"),
      { ...(thread("01J9CCCCCCCCCCCCCCCCCCCCCC", "#save", "Save") as object), status: "resolved" } as never,
    ], null);
    t.advance(0);
    expect(placed.map(p => [p.id, p.n])).toEqual([["01J9AAAAAAAAAAAAAAAAAAAAAA", null], ["01J9BBBBBBBBBBBBBBBBBBBBBB", 1]]);
    document.querySelector("#save")!.remove();
    expect(r.measure().map(p => p.n)).toEqual([null, 1]);
    r.stop();
  });

  it("stops listening when stopped", async () => {
    const t = manual();
    let runs = 0;
    const r = new Resolver(document, () => { runs++; }, t);
    r.set([], null);
    t.advance(0);
    r.stop();
    document.body.appendChild(document.createElement("p"));
    await flush();
    t.advance(QUIET_MS);
    expect(runs).toBe(1);
  });
});
