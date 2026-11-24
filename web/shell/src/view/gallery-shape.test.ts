import { readFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import type { Artifact, AttentionSummary } from "../api";
import { meBot, week } from "../bot/fixtures";
import { banner } from "../bot/model";
import { groups } from "./attention-model";
import type { GalleryBoot } from "./gallery-boot";
import { SKELETON_CARDS } from "./gallery-frame";
import { SHAPE_KEY, type Shape, cleanShape, forgetShapes, keepShape, keptShape, measureShape } from "./gallery-shape";

/** A Storage over a map, or one that throws on every call. */
function storage(init: Record<string, string> = {}, throws = false): Storage {
  const m = new Map(Object.entries(init));
  const no = () => { throw new Error("denied"); };
  return {
    get length() { return throws ? no() : m.size; },
    key: i => (throws ? no() : [...m.keys()][i] ?? null),
    getItem: k => (throws ? no() : m.get(k) ?? null),
    setItem: (k, v) => { if (throws) no(); m.set(k, String(v)); },
    removeItem: k => { if (throws) no(); m.delete(k); },
    clear: () => { if (throws) no(); m.clear(); },
  };
}

const SHAPE: Shape = { w: 1280, links: 212.5, ban: 0, sum: 431.25, tb: 44, need: 2, rest: 5, tile: 1, empty: false };

describe("the kept shapes", () => {
  it("keeps one shape per page, finite and non-negative numbers only, and forgets them all", () => {
    const s = storage();
    keepShape("/", SHAPE, s);
    keepShape("/inbox", { w: 390, links: 90, ban: 62, sum: 0, tb: 0 }, s);
    expect(keptShape("/", s)).toEqual(SHAPE);
    expect(keptShape("/inbox", s)).toEqual({ w: 390, links: 90, ban: 62, sum: 0, tb: 0 });
    expect(keptShape("/tags", s)).toBeNull();
    forgetShapes(s);
    expect(s.getItem(SHAPE_KEY)).toBeNull();
    expect(keptShape("/", s)).toBeNull();
  });

  it("drops what is not a number it can use, and a shape with no width", () => {
    expect(cleanShape({ ...SHAPE, links: Number.NaN, sum: -4, tb: "44", empty: "no", extra: 1 })).toEqual({ w: 1280, links: 0, ban: 0, sum: 0, tb: 0, need: 2, rest: 5, tile: 1 });
    expect(cleanShape({ links: 4 })).toBeNull();
    expect(cleanShape(null)).toBeNull();
    const s = storage({ [SHAPE_KEY]: JSON.stringify({ "/": { w: Infinity }, "/x/y": SHAPE, "/tags": SHAPE }) });
    expect(keptShape("/", s)).toBeNull();
    expect(keptShape("/x/y", s)).toBeNull();
    expect(keptShape("/tags", s)).toEqual(SHAPE);
  });

  it("keeps nothing and reads nothing where storage throws, and never throws itself", () => {
    const s = storage({}, true);
    expect(() => keepShape("/", SHAPE, s)).not.toThrow();
    expect(keptShape("/", s)).toBeNull();
    expect(() => forgetShapes(s)).not.toThrow();
    expect(keptShape("/", storage({ [SHAPE_KEY]: "{" }))).toBeNull();
  });

  it("measures the page: the links' width, the banner, summary and tag bar with their margins, and the list's cards", () => {
    document.body.innerHTML = `<header class="gbar"><span class="glinks"></span></header><main class="gal"><div class="bbanner" style="margin-bottom:18px"></div><section class="inbox-sum" hidden></section><div class="tagbar" style="margin:0 0 24px"></div></main>`;
    const rect = (h: number, w = 0) => () => ({ height: h, width: w, x: 0, y: 0, top: 0, left: 0, right: w, bottom: h, toJSON() {} }) as DOMRect;
    document.querySelector(".glinks")!.getBoundingClientRect = rect(32, 180);
    document.querySelector(".bbanner")!.getBoundingClientRect = rect(44);
    document.querySelector(".inbox-sum")!.getBoundingClientRect = rect(0);
    document.querySelector(".tagbar")!.getBoundingClientRect = rect(20);
    const s = measureShape(document, { need: 1, rest: 4, tile: true, empty: false });
    expect(s).toEqual({ w: innerWidth, links: 180, ban: 62, sum: 0, tb: 44, need: 2, nh: 0, rest: 4, tile: 1, empty: false });
    // Without needs, the tile leads the rest.
    expect(measureShape(document, { need: 0, rest: 4, tile: true, empty: false })).toMatchObject({ need: 0, rest: 5 });
    expect(measureShape(document)).not.toHaveProperty("rest");
    document.body.replaceChildren();
  });
});

// The inline `clax-shape` script, run as the page runs it: after the block
// and the `clax-page` script (which marks the page's kind), before the body.
const html = readFileSync(join(__dirname, "../../index.html"), "utf8");
const script = /<script id="clax-shape">([\s\S]*?)<\/script>/.exec(html)![1];

type Run = { path?: string; search?: string; kind?: string; block?: unknown; kept?: Record<string, unknown> | string; nav?: string; width?: number; throws?: boolean };
type Out = { sk: string[]; vars: Record<string, string>; need: number; rest: number };

function run(o: Run = {}): Out {
  const root = document.documentElement;
  root.dataset.page = o.kind ?? "gallery";
  delete root.dataset.sk;
  for (const v of ["--sk-ban", "--sk-sum", "--sk-tb", "--sk-links", "--sk-nh"]) root.style.removeProperty(v);
  document.head.replaceChildren();
  if (o.block !== undefined) {
    const b = document.createElement("script");
    b.id = "clax-boot";
    b.type = "application/json";
    b.textContent = JSON.stringify(o.block);
    document.head.append(b);
  }
  const kept: Record<string, string> = o.kept === undefined ? {} : { [SHAPE_KEY]: typeof o.kept === "string" ? o.kept : JSON.stringify(o.kept) };
  const perf = { getEntriesByType: () => [{ type: o.nav ?? "navigate" }] };
  new Function("document", "location", "localStorage", "performance", "innerWidth", script)(document, { pathname: o.path ?? "/", search: o.search ?? "" }, storage(kept, o.throws), perf, o.width ?? 1280);
  const style = document.head.querySelector("style")?.textContent ?? "";
  const nth = (g: string) => { const m = new RegExp(`\\.sk-${g} \\.sk-c:nth-child\\(n\\+(\\d+)\\)`).exec(style); return m ? Number(m[1]) - 1 : -1; };
  const vars = Object.fromEntries(["--sk-ban", "--sk-sum", "--sk-tb", "--sk-links", "--sk-nh"].map(v => [v, root.style.getPropertyValue(v)]).filter(([, v]) => v));
  return { sk: (root.dataset.sk ?? "").split(" ").filter(Boolean), vars, need: nth("need"), rest: nth("rest") };
}

const art = (id: string, o: Partial<Artifact> = {}): Artifact => ({ id, title: id, description: null, icon: null, pinned: false, current_version: 2, updated_at: "2026-11-01T00:00:00Z", ...o }) as Artifact;
const att = (o: Partial<AttentionSummary> = {}): AttentionSummary => ({ addressed: [], addressed_v: null, new_replies: [], open_in: [], seen: 2, ...o }) as AttentionSummary;

function block(list: Artifact[], attention: Record<string, AttentionSummary>, o: Partial<GalleryBoot> = {}): GalleryBoot {
  return {
    v: 1, kind: "gallery",
    status: { viewer: 200, hosted: 404, feedback: 200, bot: 200, summary: 401, artifacts: 200, attention: 200, questions: 401 },
    viewer: { viewer: { public_id: "v1", display_name: "Bea" }, may_create: false },
    hosted: null, feedback: null, bot: { enabled: false } as never, inbox: false,
    artifacts: { artifacts: list }, attention: { artifacts: attention }, summary: null, questions: null,
    ...o,
  };
}

describe("the inline clax-shape script", () => {
  afterEach(() => {
    document.head.replaceChildren();
    delete document.documentElement.dataset.sk;
    document.documentElement.dataset.page = "";
  });

  it("caps each group at the skeleton's cards", () => {
    expect((/Math\.min\(v\|0,(\d+)\)/.exec(script) ?? [])[1]).toBe(String(SKELETON_CARDS));
  });

  it("groups the block's cards as the gallery does, the New tile leading the first group", () => {
    const list = [
      art("a"), art("b", { current_version: 3 }), art("c", { tags: [{ id: "t1", name: "q4", colour: "green" }] }), art("d"), art("e"),
    ];
    const attention = { a: att({ addressed: ["t"] }), b: att(), c: att({ new_replies: ["r"] }), d: att({ seen: null }) };
    const g = groups(list, attention);
    const out = run({ block: block(list, attention, { viewer: { viewer: null, may_create: true } }) });
    expect(out.need).toBe(g.needs.length + 1);
    expect(out.rest).toBe(g.rest.length);
    expect(out.sk.sort()).toEqual(["known", "need", "tb"]);
    // No needs: the tile leads the rest; no tags, no tag bar.
    const calm = run({ block: block([art("a"), art("b")], {}, { viewer: { viewer: null, may_create: true } }) });
    expect(calm).toMatchObject({ need: 0, rest: 3 });
    expect(calm.sk).toEqual(["known"]);
  });

  it("shows the empty gallery's place, not cards, for an empty list (with the tile where it shows)", () => {
    expect(run({ block: block([], {}) }).sk.sort()).toEqual(["empty", "known"]);
    expect(run({ block: block([], {}, { viewer: { viewer: null, may_create: true } }) }).sk.sort()).toEqual(["empty", "known", "tile"]);
  });

  it("holds the summary's place for a browser with an inbox that has something unread or an open question", () => {
    const inbox = (unread: number, open: number) => block([art("a")], {}, {
      inbox: true, status: { ...block([], {}).status, summary: 200, questions: 200 },
      summary: { unread, questions: [], latest: [] } as never, questions: { questions: Array(open).fill({}), open },
    });
    expect(run({ block: inbox(3, 0) }).sk).toContain("sum");
    expect(run({ block: inbox(0, 1) }).sk).toContain("sum");
    expect(run({ block: inbox(0, 0) }).sk).not.toContain("sum");
    // The block not saying, the kept shape does.
    expect(run({ block: { ...inbox(3, 0), status: { ...inbox(3, 0).status, summary: undefined } }, kept: { "/": { ...SHAPE, sum: 0 } } }).sk).not.toContain("sum");
    expect(run({ block: { ...inbox(3, 0), inbox: null, status: { ...inbox(3, 0).status, summary: undefined } }, kept: { "/": SHAPE } }).sk).toContain("sum");
  });

  it("holds the usage banner's place as the banner decides it, on every page", () => {
    for (const w of [week(), week({ spent_milli: 1_600_000 }), week({ spent_milli: 2_100_000 }), week({ warn: true }), week({ weekly_milli: 0, spent_milli: 1 }), week({ weekly_milli: 0, spent_milli: 0 })]) {
      for (const kind of ["gallery", "page"]) {
        const me = meBot({ week: w });
        expect(run({ kind, path: kind === "page" ? "/tags" : "/", block: block([art("a")], {}, { bot: me as never }) }).sk.includes("ban"), JSON.stringify(w)).toBe(banner(me) !== null);
      }
    }
    expect(run({ block: block([art("a")], {}, { bot: meBot({ enabled: false, week: week({ spent_milli: 2_100_000 }) }) as never }) }).sk).not.toContain("ban");
  });

  it("uses the shape kept for the page where there is no block, its heights only at the width it was measured", () => {
    const kept = { "/": SHAPE, "/settings": { w: 1280, links: 120, ban: 62, sum: 0, tb: 0 } };
    expect(run({ kept })).toEqual({ sk: ["sum", "tb", "known", "need"], vars: { "--sk-sum": "431.25px", "--sk-tb": "44px", "--sk-links": "212.5px" }, need: 2, rest: 5 });
    // The needs group's height, for as many cards as it had.
    expect(run({ kept: { "/": { ...SHAPE, nh: 310.5 } } }).vars["--sk-nh"]).toBe("310.5px");
    expect(run({ kept: { "/": { ...SHAPE, nh: 310.5 } }, block: block([art("a")], { a: att({ addressed: ["t"] }) }) }).vars["--sk-nh"]).toBeUndefined();
    expect(run({ kept, width: 390 })).toEqual({ sk: ["sum", "tb", "known", "need"], vars: {}, need: 2, rest: 5 });
    expect(run({ kept, path: "/settings/people", kind: "page" })).toMatchObject({ sk: ["ban"], vars: { "--sk-ban": "62px", "--sk-links": "120px" } });
    expect(run({ kept: { "/": { ...SHAPE, empty: true, tile: 1, need: 0, rest: 1 } } }).sk).toEqual(["sum", "tb", "known", "empty", "tile"]);
  });

  it("reads nothing of the block on a load through history, using the kept shape instead", () => {
    const b = block([], {}, { viewer: { viewer: null, may_create: true } });
    expect(run({ block: b, nav: "back_forward", kept: { "/": SHAPE } })).toMatchObject({ need: 2, rest: 5 });
    expect(run({ block: b, nav: "back_forward" }).sk).toEqual([]);
  });

  it("falls back to a neutral skeleton for nothing usable: no block, no shape, a shape it cannot read, storage that throws", () => {
    const neutral = { sk: [], vars: {}, need: 0, rest: 6 };
    expect(run()).toEqual(neutral);
    expect(run({ kept: "{" })).toEqual(neutral);
    expect(run({ throws: true, kept: { "/": SHAPE } })).toEqual(neutral);
    expect(run({ kept: { "/": { w: 1280, links: "9px", ban: -3, sum: Number.NaN, tb: null, need: -1, rest: "x" } } })).toEqual(neutral);
    expect(run({ block: { v: 2, status: {} } })).toEqual(neutral);
  });

  it("shows one list under a tag filter, and does nothing on the sign-in pages", () => {
    expect(run({ kept: { "/": SHAPE }, search: "?tag=q4" })).toMatchObject({ sk: ["sum", "tb", "known", "flat"], need: 0, rest: 5 });
    const root = document.documentElement;
    run({ kind: "auth", kept: { "/": SHAPE } });
    expect(root.dataset.sk).toBeUndefined();
  });
});
