import { readFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { DEFAULT, MAX_DRAWERS, cleanDrawer, keepDrawer, keptDrawer, measureDrawer, skeletonBlocks } from "./drawer-shape";
import { keepShape, keptShape } from "./gallery-shape";
import { SHAPE_KEY } from "./kept-shape";

const DSK = /<script id="clax-dsk">([\s\S]*?)<\/script>/.exec(readFileSync(join(__dirname, "../../artifact.html"), "utf8"))![1];
const ID = "7q3k9mzx2b4t";

/** A storage over `init`, that throws on every call with `broken`. */
function storage(init: Record<string, string> = {}, broken = false): Storage {
  const m = new Map(Object.entries(init));
  const fail = () => { if (broken) throw new Error("storage off"); };
  return {
    get length() { fail(); return m.size; },
    key: i => { fail(); return [...m.keys()][i] ?? null; },
    getItem: k => { fail(); return m.get(k) ?? null; },
    setItem: (k, v) => { fail(); m.set(k, String(v)); },
    removeItem: k => { fail(); m.delete(k); },
    clear: () => { fail(); m.clear(); },
  };
}

afterEach(() => { document.body.replaceChildren(); document.head.replaceChildren(); localStorage.clear(); history.replaceState(null, "", "/"); });

describe("the threads' kept shape", () => {
  it("keeps an artifact's blocks beside the gallery's shapes, each kind keeping the other's", () => {
    const s = storage();
    keepShape("/", { w: 1280, links: 0, ban: 0, sum: 0, tb: 0 }, s);
    keepDrawer(ID, { b: [[53, 34], [99.4, 22]] }, s);
    expect(keptDrawer(ID, s)).toEqual({ b: [[53, 34], [99, 22]] });
    keepShape("/tags", { w: 1280, links: 0, ban: 0, sum: 0, tb: 0 }, s);
    expect(keptDrawer(ID, s)).toEqual({ b: [[53, 34], [99, 22]] });
    expect(keptShape("/", s)?.w).toBe(1280);
    expect(keptDrawer("not-an-id", s)).toBeNull();
  });

  it("keeps the newest artifacts' shapes alone", () => {
    const s = storage();
    for (let i = 0; i <= MAX_DRAWERS; i++) keepDrawer(i.toString(36).padStart(12, "0"), { b: [[1, 1]] }, s);
    expect(keptDrawer("000000000000", s)).toBeNull();
    expect(keptDrawer(MAX_DRAWERS.toString(36).padStart(12, "0"), s)).not.toBeNull();
    expect(Object.keys(JSON.parse(s.getItem(SHAPE_KEY)!))).toHaveLength(MAX_DRAWERS);
  });

  it("drops what is not a block, and keeps nothing where storage throws", () => {
    expect(cleanDrawer({ b: [[1, 2], [-1, 3], [1, Number.NaN], "x", [4, 5, 6]] })).toEqual({ b: [[1, 2]] });
    expect(cleanDrawer({ b: [] })).toBeNull();
    const s = storage({}, true);
    expect(() => keepDrawer(ID, { b: [[1, 2]] }, s)).not.toThrow();
    expect(keptDrawer(ID, s)).toBeNull();
  });

  it("measures the list's blocks from the drawer's top, within the window", () => {
    document.body.innerHTML = `<div class="drawer"><aside class="sidebar"><div class="new-comment"></div><h2 class="gh"></h2><article class="thread-card"></article><article class="thread-card"></article></aside></div>`;
    const tops = [100, 153, 199, 231, 2000];
    const els = [document.querySelector(".drawer")!, ...document.querySelectorAll(".new-comment, .gh, .thread-card")];
    els.forEach((e, i) => { e.getBoundingClientRect = () => ({ top: tops[i], height: i ? 30 : 800 }) as DOMRect; });
    expect(measureDrawer(els[0], 900)).toEqual({ b: [[53, 30], [99, 30], [131, 30]] });
  });
});

describe("the threads' skeleton", () => {
  /** The blocks the inline clax-dsk script draws for the page at `path`, with `boot` and `kept`. */
  const drawn = (path: string, boot: unknown, kept?: unknown) => {
    history.replaceState(null, "", path);
    if (kept) localStorage.setItem(SHAPE_KEY, JSON.stringify({ [`/a/${ID}`]: kept }));
    document.head.innerHTML = boot ? `<script type="application/json" id="clax-boot">${JSON.stringify(boot)}</script>` : "";
    document.body.innerHTML = `<div id="app"><div class="page"><div class="viewer"></div></div></div>`;
    new Function(DSK)();
    return [...document.querySelectorAll<HTMLElement>(".viewer > .dsk > i")].map(i => [parseFloat(i.style.top), parseFloat(i.style.height)]);
  };
  const open = (n: number) => Array.from({ length: n }, (_, i) => ({ status: i % 2 ? "resolved" : "open" }));

  it("draws, before the first paint, the blocks skeletonBlocks says: kept, else the block's open threads, else two cards", () => {
    expect(drawn(`/a/${ID}`, null)).toEqual(skeletonBlocks(null, null));
    expect(drawn(`/a/${ID}/v/2`, { v: 1, threads: open(5) })).toEqual(skeletonBlocks(null, 3));
    expect(drawn(`/a/${ID}`, { v: 1, threads: open(20) })).toEqual(skeletonBlocks(null, 10));
    expect(skeletonBlocks(null, 10)).toHaveLength(2 + DEFAULT.most);
    expect(drawn(`/a/${ID}`, { v: 1, threads: open(2) }, { b: [[60, 40], [110, 300]] })).toEqual([[60, 40], [110, 300]]);
    expect(drawn("/", null)).toEqual([]);
    expect(document.querySelector(".dsk")).toBeNull();
  });
});
