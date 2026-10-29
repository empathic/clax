import { render as renderComponent } from "@testing-library/svelte";
import { flushSync, tick } from "svelte";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { Anchor } from "../../bridge/src/protocol";
import type { RenderResult } from "../../bridge/src/clip";
import type { NewReport, Reopening, Verification } from "../../sdk/src/reporter";
import type { Badge, DiagLine, Report, ReportPage } from "../../sdk/src/types";
import { mount, type Mounted } from "../../shell/src/test/svelte";
import type { FlattenDeps, Surface } from "./annotate";
import type { CaptureDeps } from "./capture";
import { PAGE_ANCHOR, POLL_MS, type ReporterApi, Session, type Timers } from "./model";
import Overlay from "./Overlay.svelte";
import type { Picked, Picker } from "./picker";

const anchor: Anchor = { kind: "element", selector: "#total", quote: "Total: $12", prefix: "Items: 3", suffix: null, html_hash: "h:4f2a", rect: { x: 10, y: 20, w: 100, h: 20, scrollX: 0, scrollY: 40, viewportW: 1024 }, custom_name: null, file: "index.html", label: "Total: $12", name: "Total" };
/** An element pick at `anchor` whose clip `render` draws, leaving nothing out. */
const picked = (render: () => Promise<ArrayBuffer>): Picked =>
  ({ anchor, mode: "element", rect: () => ({ x: 0, y: 0, w: 100, h: 20 }), render: () => render().then(png => ({ png, omitted: [], at: { x: 0, y: 0, w: 100, h: 20 }, scale: 1 })) });
const when = new Date("2026-10-29T07:12:03.120Z");
const lines: DiagLine[] = [{ at: "2026-10-29T07:12:00Z", level: "error", kind: "error", text: "TypeError: total is undefined" }];

function report(id: string, bodies: string[], over: Partial<Report> = {}): Report {
  return {
    id, queue_id: "q1", artifact: { id: "a1", external_id: "e", title: "Shop", url: "https://shop.example/", metadata: null, metadata_version: 0, revision: null },
    anchor, status: "open", state: "open", reporter_id: "u1", metadata: null, metadata_version: 0, has_clip: false, claim: null, created_at: "",
    comments: bodies.map((body, i) => ({ id: `${id}c${i}`, author_kind: i ? "agent" : "reporter", author_name: i ? "Triage agent" : "u1", body, created_at: "" })),
    ...over,
  };
}
const proposed = (id: string) => report(id, ["The end date is wrong", "Fixed the end date in 0.6.1"], {
  state: "fix_proposed",
  fixes: [{ n: 1, summary: "Fixed the end date in 0.6.1", url: "https://github.com/o/leave/pull/12", verify_url: "https://leave.example/req/1", proposer_name: "Triage agent", created_at: "", outcome: "pending" }],
});

function manualTimers() {
  const due: { fn: () => void; ms: number; id: number }[] = [];
  let next = 0;
  return {
    set: (fn: () => void, ms: number) => { const id = ++next; due.push({ fn, ms, id }); return id; },
    clear: (h: unknown) => { const i = due.findIndex(d => d.id === h); if (i >= 0) due.splice(i, 1); },
    /** Runs the timers due after `ms`. */
    fire(ms: number) { for (const d of due.filter(x => x.ms === ms)) { due.splice(due.indexOf(d), 1); d.fn(); } },
    pending: () => due.map(d => d.ms),
  } satisfies Timers & Record<string, unknown>;
}
const settle = async () => { for (let i = 0; i < 8; i++) await new Promise(r => setTimeout(r, 0)); flushSync(); };

/** Flattening as a canvas would, without one: the marks' count is the PNG. */
let drawn: number[];
const flatten: FlattenDeps = {
  decode: async () => ({ width: 100, height: 20, image: {} as CanvasImageSource }),
  surface: () => {
    let marks = 0;
    const ctx = new Proxy({}, { get: (_, k) => (k === "measureText" ? () => ({ width: 10 }) : k === "drawImage" ? () => {} : typeof k === "string" && /Rect|stroke|fillText/.test(k) ? () => { marks++; } : () => {}), set: () => true }) as Surface["ctx"];
    return { ctx, png: async () => { drawn.push(marks); return new Blob([new Uint8Array([7, marks])], { type: "image/png" }); } };
  },
};

let view: Mounted<Record<string, unknown>>;
let modes: boolean[];
let areas: boolean[];
let done: number;
let onPick: (p: Picked) => void;
let onCancel: () => void;
let filed: NewReport[];
let verified: [string, Verification | undefined][];
let reopened: [string, Reopening][];
let seen: string[];
let badges: Badge[];
let pages: ReportPage[];
let listed: number;
let comments: [string, string][];
let timers: ReturnType<typeof manualTimers>;
let host: HTMLElement;
let screens: number;
let contexts: number;
let client: ReporterApi;
const urls = URL as unknown as { createObjectURL?: unknown; revokeObjectURL?: unknown };
const had = { create: urls.createObjectURL, revoke: urls.revokeObjectURL };

const pickerOf = () => (p: (x: Picked) => void, c: () => void): Picker => {
  onPick = p;
  onCancel = c;
  return { set: on => modes.push(on), areasOnly: on => areas.push(on), done: () => { done++; }, destroy: () => {} };
};
const capture: CaptureDeps = {
  supported: false,
  renderViewport: async (): Promise<RenderResult> => { screens++; return { png: new Uint8Array([137, 80, 78, 71]).buffer, omitted: [], at: { x: 0, y: 0, w: 800, h: 600 }, scale: 1 }; },
};
const session = () => new Session({ context: () => { contexts++; return { release: { version: "0.6.0", commit: "f7e862ab0" }, flags: { claxbot: true }, app: { view: "cart" } }; }, metadata: () => ({ build: "7" }), recorder: { lines: () => lines, count: () => 1, stop: () => {} }, now: () => when }, window);
const props = (over: Record<string, unknown> = {}) => ({ client, doc: document, host, picker: pickerOf(), externalId: "shop-cart", session: session(), capture, flatten, timers, ...over });

beforeEach(() => {
  urls.createObjectURL = () => "blob:clip";
  urls.revokeObjectURL = () => {};
  modes = [];
  areas = [];
  done = 0;
  filed = [];
  verified = [];
  reopened = [];
  seen = [];
  badges = [];
  pages = [];
  listed = 0;
  comments = [];
  drawn = [];
  screens = 0;
  contexts = 0;
  timers = manualTimers();
  client = {
    fileReport: async r => { filed.push(r); return report("t9", [r.body]); },
    myReports: async () => { listed++; return pages.shift() ?? { reports: [], next: null }; },
    comment: async (id, body) => { comments.push([id, body]); return report(id, ["first", body]); },
    verify: async (id, v) => { verified.push([id, v]); return report(id, ["The end date is wrong"], { state: "verified", status: "resolved" }); },
    reopen: async (id, r) => { reopened.push([id, r]); return report(id, ["The end date is wrong", r.body], { state: "open" }); },
    badge: async () => badges.shift() ?? { waiting: 0, report_ids: [] },
    seen: async id => { seen.push(id); },
  };
  host = document.createElement("div");
  document.body.append(host);
  view = mount(Overlay as never, props(), host);
});
afterEach(() => {
  view.unmount();
  host.remove();
  urls.createObjectURL = had.create;
  urls.revokeObjectURL = had.revoke;
});

const q = <E extends HTMLElement = HTMLElement>(s: string) => view.root.querySelector<E>(s);
/** A button's name as a screen reader hears it: its text without what is aria-hidden. */
const nameOf = (b: Element) => { const c = b.cloneNode(true) as Element; c.querySelectorAll('[aria-hidden="true"]').forEach(n => n.remove()); return c.textContent?.trim(); };
const button = (label: string) => [...view.root.querySelectorAll("button")].find(b => nameOf(b) === label);
const typeIn = (sel: string, v: string) => { const t = q<HTMLTextAreaElement>(sel)!; t.value = v; t.dispatchEvent(new Event("input", { bubbles: true })); flushSync(); };
const type = (v: string) => typeIn(".composer textarea", v);
const click = (b: HTMLElement | undefined | null) => { if (!b) throw new Error("no such button"); b.click(); flushSync(); };
const key = (k: string, target: EventTarget = document.body) => { target.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true, composed: true })); flushSync(); };

describe("the overlay", () => {
  it("starts Element mode from the launcher's mode bar, and Cancel or Escape ends it", () => {
    expect(q(".launcher")).not.toBeNull();
    click(button("Report"));
    expect(modes).toEqual([true]);
    expect(q(".modebar")?.textContent).toMatch(/Click what your report is about/);
    expect(q('.modebar button[aria-pressed="true"]')?.textContent).toMatch(/Element/);
    click(button("Cancel"));
    expect(modes).toEqual([true, false]);
    expect(q(".modebar")).toBeNull();
    click(button("Report"));
    onCancel();
    flushSync();
    expect(modes).toEqual([true, false, true, false]);
    expect(q(".modebar")).toBeNull();
  });

  it("switches modes with E, A and S, and leaves with Escape", async () => {
    click(button("Report"));
    key("a");
    expect(areas.at(-1)).toBe(true);
    expect(q('.modebar button[aria-pressed="true"]')?.textContent).toMatch(/Area/);
    expect(q(".modebar")?.textContent).toMatch(/Drag over/);
    key("e");
    expect(areas.at(-1)).toBe(false);
    // Typing in a field of the page is not a mode key.
    const field = document.createElement("input");
    document.body.append(field);
    key("a", field);
    expect(areas.at(-1)).toBe(false);
    field.remove();
    key("Escape");
    expect(q(".modebar")).toBeNull();
    expect(modes.at(-1)).toBe(false);
  });

  it("S takes a screen capture", async () => {
    click(button("Report"));
    key("s");
    await settle();
    expect(screens).toBe(1);
    expect(modes.at(-1)).toBe(false);
    expect(q(".modebar")).toBeNull();
    expect(q(".composer .quote")?.textContent).toBe("What the page shows");
    expect(q<HTMLImageElement>(".composer .annotator img")?.getAttribute("src")).toBe("blob:clip");
    type("The banner covers the total");
    click(button("Send"));
    await settle();
    expect(filed).toHaveLength(1);
    expect(filed[0].anchor).toEqual(PAGE_ANCHOR);
    expect(filed[0].context).toMatchObject({ v: 1, captured_at: when.toISOString(), release: { version: "0.6.0" }, capture: { mode: "screen", method: "dom", omitted: [] } });
    // A Screen capture has no marks: its PNG is sent as taken.
    expect(drawn).toEqual([]);
  });

  it("opens the composer at a pick with the clip and its outline, and sends the report with them", async () => {
    click(button("Report"));
    let rendered: (b: ArrayBuffer) => void = () => {};
    onPick(picked(() => new Promise(r => { rendered = r; })));
    flushSync();
    await settle();
    expect(modes.at(-1)).toBe(false);
    expect(contexts).toBe(1);
    expect(q(".composer .quote")?.textContent).toBe("«Total: $12»");
    expect(q(".composer [role=status]")?.textContent).toMatch(/Taking the screenshot/);
    rendered(new Uint8Array([137, 80, 78, 71]).buffer);
    await settle();
    expect(done).toBe(1);
    const img = q<HTMLImageElement>(".composer .annotator img");
    expect(img?.getAttribute("src")).toBe("blob:clip");
    expect(img?.alt).toBe("Screenshot of the picked part of the page");
    // The pick's outline is drawn as a box, which Undo removes.
    expect(view.root.querySelectorAll(".annotator svg [data-kind=box]")).toHaveLength(1);
    expect(q(".composer textarea")?.getAttribute("aria-label")).toBe("Report");
    expect(button("Send")?.disabled).toBe(true);
    type("The total should include tax");
    click(button("Send"));
    await settle();
    expect(filed).toHaveLength(1);
    expect(filed[0]).toMatchObject({ body: "The total should include tax", anchor, externalId: "shop-cart", metadata: { build: "7" }, url: location.href });
    // The box was drawn into the PNG sent.
    expect(drawn).toEqual([1]);
    expect(filed[0].clip).toBeInstanceOf(Blob);
    expect(q(".composer")).toBeNull();
    expect(q(".filed")?.textContent).toMatch(/Filed/);
    expect(document.activeElement).toBe(button("View in Your reports"));
  });

  it("sends the clip without the outline once it is undone, and no clip when unticked", async () => {
    click(button("Report"));
    onPick(picked(async () => new Uint8Array([1]).buffer));
    await settle();
    click(button("Undo"));
    expect(view.root.querySelectorAll(".annotator svg [data-kind]")).toHaveLength(0);
    type("Plain");
    click(button("Send"));
    await settle();
    expect(drawn).toEqual([]);
    expect(filed[0].clip).toBeInstanceOf(Blob);
    click(button("Report"));
    onPick(picked(async () => new Uint8Array([1]).buffer));
    await settle();
    click(q('input[aria-label="Include screenshot"]'));
    type("No picture");
    click(button("Send"));
    await settle();
    expect("clip" in filed[1]).toBe(false);
  });

  it("sends recent errors only when ticked", async () => {
    click(button("Report"));
    onPick(picked(async () => new Uint8Array([1]).buffer));
    await settle();
    const box = q<HTMLInputElement>('input[aria-label="Include recent errors (1)"]')!;
    expect(box.checked).toBe(false);
    type("Without");
    click(button("Send"));
    await settle();
    expect("diagnostics" in filed[0]).toBe(false);
    click(button("Report"));
    onPick(picked(async () => new Uint8Array([1]).buffer));
    await settle();
    click(q('input[aria-label="Include recent errors (1)"]'));
    type("With");
    click(button("Send"));
    await settle();
    expect(filed[1].diagnostics).toEqual(lines);
  });

  it("preview shows the context JSON", async () => {
    click(button("Report"));
    onPick(picked(async () => new Uint8Array([1]).buffer));
    await settle();
    const details = q<HTMLDetailsElement>(".composer details.sent")!;
    expect(details.querySelector("summary")?.textContent).toBe("What's sent");
    const json = details.querySelector("pre.context")!.textContent!;
    expect(JSON.parse(json)).toMatchObject({ v: 1, captured_at: when.toISOString(), release: { version: "0.6.0", commit: "f7e862ab0" }, flags: { claxbot: true }, app: { view: "cart" }, capture: { mode: "element", method: "dom" } });
    expect(details.textContent).toMatch(/Recent errors are not sent/);
    click(q('input[aria-label="Include recent errors (1)"]'));
    expect(details.querySelector(".diag")?.textContent).toMatch(/TypeError: total is undefined/);
  });

  it("offers the exact screenshot, taken with the overlay stepped aside, and keeps the page render when it is refused", async () => {
    view.unmount();
    const seenOpacity: string[] = [];
    let answer: "ok" | "denied" = "ok";
    const exact: CaptureDeps = {
      ...capture,
      supported: true,
      painted: async () => {},
      display: async () => {
        seenOpacity.push(host.style.getPropertyValue("opacity"));
        return answer === "ok" ? { ok: true, png: new Uint8Array([9, 9]).buffer, thisTab: true } : { ok: false, reason: "denied", message: "no" };
      },
    };
    view = mount(Overlay as never, props({ capture: exact }), host);
    click(button("Report"));
    onPick(picked(async () => new Uint8Array([1]).buffer));
    await settle();
    expect(view.root.querySelectorAll(".annotator svg [data-kind=box]")).toHaveLength(1);
    click(button("Use exact screenshot"));
    await settle();
    // The overlay's host is transparent while the browser captures, and shown again after.
    expect(seenOpacity).toEqual(["0"]);
    expect(host.style.getPropertyValue("opacity")).toBe("");
    // The reporter's marks are moved onto the exact screenshot, and the composer says so.
    expect(view.root.querySelectorAll(".annotator svg [data-kind=box]")).toHaveLength(1);
    expect(q(".composer .marks")?.textContent).toMatch(/Your marks were moved onto the new screenshot/);
    expect(button("Use exact screenshot")).toBeUndefined();
    type("Exact");
    click(button("Send"));
    await settle();
    expect(filed[0].context?.capture).toEqual({ mode: "element", method: "display" });
    // Refused: the page render stays, and the composer says why.
    answer = "denied";
    click(button("Report"));
    onPick(picked(async () => new Uint8Array([1]).buffer));
    await settle();
    click(button("Use exact screenshot"));
    await settle();
    expect(q(".composer .warn")?.textContent).toMatch(/You cancelled screen sharing/);
    expect(view.root.querySelectorAll(".annotator svg [data-kind=box]")).toHaveLength(1);
  });

  it("draws every kind of mark from the keyboard, announced, and redacts the pick in one step", async () => {
    click(button("Report"));
    onPick(picked(async () => new Uint8Array([1]).buffer));
    await settle();
    const stage = q(".annotator .stage")!;
    expect(stage.tabIndex).toBe(0);
    stage.focus();
    const said = async () => { await tick(); flushSync(); return q(".annotator [aria-live]")?.textContent; };
    const marks = () => [...view.root.querySelectorAll(".annotator svg [data-kind]")].map(n => n.getAttribute("data-kind"));
    for (const [tool, kind] of [["Box", "box"], ["Arrow", "arrow"], ["Redact", "redact"]] as const) {
      click(button(tool));
      stage.focus();
      key("Enter", stage);
      expect(await said()).toMatch(/first corner set/);
      key("ArrowRight", stage);
      key("ArrowDown", stage);
      key("Enter", stage);
      expect(await said()).toMatch(/added\./);
      expect(marks().at(-1)).toBe(kind);
    }
    // Escape drops a first corner, and leaves the draft be.
    key("Enter", stage);
    key("Escape", stage);
    expect(await said()).toBe("Mark cancelled.");
    expect(q(".composer")).not.toBeNull();
    click(button("Text"));
    stage.focus();
    key("Enter", stage);
    const input = q<HTMLInputElement>('.annotator input[aria-label="Text to add"]')!;
    input.value = "this one";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    flushSync();
    expect(marks().at(-1)).toBe("text");
    expect(await said()).toBe("Text added.");
    click(button("Redact the pick"));
    const last = view.root.querySelectorAll(".annotator svg [data-kind=redact]");
    const r = last[last.length - 1];
    expect(["x", "y", "width", "height"].map(a => r.getAttribute(a))).toEqual(["0", "0", "100", "20"]);
    type("Marked");
    click(button("Send"));
    await settle();
    expect(drawn).toEqual([7]);
  });

  it("starts the crosshair where the stage shows the image, keeps it in view, and says a repeated message again", async () => {
    click(button("Report"));
    onPick(picked(async () => new Uint8Array([1]).buffer));
    await settle();
    const stage = q(".annotator .stage")!;
    const svg = q(".annotator svg")!;
    // A drawing 800 px tall, scrolled 300 px down a 240 px stage (the image is 100 x 20 px).
    const rect = (top: number, height: number) => () => ({ top, bottom: top + height, left: 0, right: 354, width: 354, height, x: 0, y: top, toJSON() {} }) as DOMRect;
    stage.getBoundingClientRect = rect(0, 240);
    svg.getBoundingClientRect = rect(-300, 800);
    stage.focus();
    key("ArrowDown", stage);
    const cross = () => Number(q(".annotator .crosshair")?.getAttribute("data-y"));
    // The middle of the stage is 420 px down the drawing: 10.5 of its 20 px; then one step down.
    expect(cross()).toBe(11 + 3);
    expect(stage.scrollTop).toBe(32);
    const texts: string[] = [];
    const live = q(".annotator [aria-live]")!;
    new MutationObserver(() => texts.push(live.textContent ?? "")).observe(live, { childList: true, characterData: true, subtree: true });
    for (let i = 0; i < 2; i++) {
      key("Enter", stage);
      await settle();
      key("Escape", stage);
      await settle();
    }
    expect(texts.filter(t => t === "Mark cancelled.")).toHaveLength(2);
  });

  it("shows the page the report points at in What's sent, from what is sent", async () => {
    document.title = "Cart";
    click(button("Report"));
    onPick(picked(async () => new Uint8Array([1]).buffer));
    await settle();
    const page = q(".composer details.sent dl.page")!;
    const rows = Object.fromEntries([...page.querySelectorAll("dt")].map(dt => [dt.textContent, dt.nextElementSibling?.textContent]));
    expect(rows).toMatchObject({ Title: "Cart", Address: location.href, Artifact: "shop-cart", Kind: "element", Selector: "#total", Text: "Total: $12" });
    // And the whole anchor, every field as sent.
    expect(JSON.parse(q(".composer details.sent pre.anchor")!.textContent!)).toEqual(anchor);
  });

  it("asks before a new report drops a Still broken note, and a reopen's capture can be retaken", async () => {
    pages.push({ reports: [proposed("t1")], next: null });
    click(button("Your reports"));
    await settle();
    click(q(".panel .row"));
    click(button("Still broken"));
    await settle();
    click(button("Element"));
    onPick(picked(async () => new Uint8Array([5]).buffer));
    await settle();
    typeIn(".composer textarea", "Still wrong");
    click(button("Retake the screenshot"));
    expect(q(".modebar")).not.toBeNull();
    key("Escape");
    expect(q(".modebar")).toBeNull();
    await settle();
    expect(document.activeElement).toBe(q(".composer textarea"));
    click(button("Report"));
    expect(q('[role="alertdialog"]')?.textContent).toMatch(/Still broken/);
    click(button("Keep writing"));
    expect(q('[role="alertdialog"]')).toBeNull();
    expect(q<HTMLTextAreaElement>('.composer[aria-label="Still broken"] textarea')?.value).toBe("Still wrong");
    click(button("Report"));
    click(button("Drop it"));
    expect(q('.composer[aria-label="Still broken"]')).toBeNull();
    expect(q(".modebar")).not.toBeNull();
    expect(modes.at(-1)).toBe(true);
  });

  it("gives focus back to Report when the mode bar closes", () => {
    click(button("Report"));
    key("Escape");
    expect(document.activeElement).toBe(q(".report-btn"));
  });

  it("announces what the keyboard outlines in the mode bar", async () => {
    view.unmount();
    let hover: ((what: string | null) => void) | undefined;
    const withHover = (p: (x: Picked) => void, c: () => void, h?: (what: string | null) => void): Picker => { hover = h; return pickerOf()(p, c); };
    view = mount(Overlay as never, props({ picker: withHover }), host);
    click(button("Report"));
    hover?.("h2: Total");
    flushSync();
    expect(q(".modebar [aria-live]")?.textContent).toBe("Outlined h2: Total.");
  });

  it("filed event fires with the report", async () => {
    view.unmount();
    const heard: Report[] = [];
    view = mount(Overlay as never, props({ onFiled: (r: Report) => heard.push(r) }), host);
    click(button("Report"));
    onPick(picked(async () => new Uint8Array([1]).buffer));
    await settle();
    type("Wrong");
    click(button("Send"));
    await settle();
    expect(heard.map(r => r.id)).toEqual(["t9"]);
    pages.push({ reports: [report("t9", ["Wrong"])], next: null });
    click(button("View in Your reports"));
    await settle();
    expect(q(".panel")).not.toBeNull();
    expect(q('.panel li.open')?.textContent).toMatch(/Wrong/);
  });

  it("launcher false mounts no launcher and report() opens the mode bar", async () => {
    view.unmount();
    const r = renderComponent(Overlay as never, { props: props({ launcher: false }) as never, target: host });
    flushSync();
    expect(host.querySelector(".launcher")).toBeNull();
    expect(host.querySelector(".report-btn")).toBeNull();
    const api = r.component as unknown as { report(o?: { mode?: string }): void; showReports(): void };
    api.report();
    flushSync();
    expect(host.querySelector(".modebar")).not.toBeNull();
    expect(modes).toEqual([true]);
    api.report({ mode: "area" });
    flushSync();
    expect(areas.at(-1)).toBe(true);
    api.showReports();
    await settle();
    expect(host.querySelector(".panel")).not.toBeNull();
    expect(host.querySelector(".modebar")).toBeNull();
    r.unmount();
    view = mount(Overlay as never, props(), host);
  });

  it("a reporter who may not file has no Report button, and report() does nothing", () => {
    view.unmount();
    view = mount(Overlay as never, props({ canFile: false }), host);
    expect(button("Report")).toBeUndefined();
    expect(button("Your reports")).toBeDefined();
  });

  it("files the page's URL with its query and without its fragment", async () => {
    const was = location.href;
    history.replaceState(null, "", "/leave?team=7#req-2");
    try {
      click(button("Report"));
      onPick(picked(async () => new ArrayBuffer(0)));
      await settle();
      type("Wrong date");
      click(button("Send"));
      await settle();
      expect(filed[0].url).toBe(`${location.origin}/leave?team=7`);
    } finally {
      history.replaceState(null, "", was);
    }
  });

  it("Your reports opened over a draft hides the composer and keeps the draft for when it closes", async () => {
    click(button("Report"));
    onPick(picked(async () => new Uint8Array([1]).buffer));
    await settle();
    type("Half written");
    click(button("Your reports"));
    await settle();
    expect(q(".composer")).toBeNull();
    expect(q(".panel")).not.toBeNull();
    click(q('[aria-label="Close your reports"]'));
    expect(q(".panel")).toBeNull();
    expect(q<HTMLTextAreaElement>(".composer textarea")?.value).toBe("Half written");
    expect(q<HTMLImageElement>(".composer .annotator img")?.getAttribute("src")).toBe("blob:clip");
    type("Half written, now done");
    click(button("Send"));
    await settle();
    expect(filed.map(r => r.body)).toEqual(["Half written, now done"]);
  });

  it("stacks the panel above the launcher", async () => {
    click(button("Your reports"));
    await settle();
    const dock = [...q(".dock")!.children].map(c => c.className.split(" ")[0]);
    expect(dock).toEqual(["panel", "launcher"]);
  });

  it("sends without a clip when the clip cannot be drawn", async () => {
    click(button("Report"));
    onPick(picked(() => Promise.reject(new Error("a cross-origin image"))));
    await settle();
    expect(q(".composer [role=status]")?.textContent).toMatch(/No screenshot: a cross-origin image/);
    type("Broken");
    click(button("Send"));
    await settle();
    expect(filed).toHaveLength(1);
    expect("clip" in filed[0]).toBe(false);
  });

  it("waits for a clip still being taken before it sends", async () => {
    click(button("Report"));
    let rendered: (b: ArrayBuffer) => void = () => {};
    onPick(picked(() => new Promise(r => { rendered = r; })));
    await settle();
    type("Soon");
    click(button("Send"));
    await settle();
    expect(filed).toHaveLength(0);
    expect(button("Sending…")).toBeDefined();
    rendered(new Uint8Array([1]).buffer);
    await settle();
    expect(filed).toHaveLength(1);
    expect(filed[0].clip).toBeInstanceOf(Blob);
  });

  it("keeps the text when a send fails, and says why", async () => {
    client.fileReport = async () => { throw Object.assign(new Error("x"), { code: "network" }); };
    click(button("Report"));
    onPick(picked(async () => new ArrayBuffer(0)));
    await settle();
    type("Keep me");
    click(button("Send"));
    await settle();
    expect(q("[role=alert]")?.textContent).toMatch(/Not sent: Clax could not be reached/);
    expect(q<HTMLTextAreaElement>(".composer textarea")?.value).toBe("Keep me");
    expect(button("Send")?.disabled).toBe(false);
  });

  it("Cancel drops the draft", async () => {
    click(button("Report"));
    onPick(picked(async () => new ArrayBuffer(0)));
    await settle();
    click(button("Cancel"));
    expect(q(".composer")).toBeNull();
    expect(document.activeElement).toBe(q(".report-btn"));
  });

  it("Escape in the composer drops the draft and gives focus back to Report", async () => {
    click(button("Report"));
    onPick(picked(async () => new ArrayBuffer(0)));
    await settle();
    const t = q<HTMLTextAreaElement>(".composer textarea")!;
    expect(document.activeElement).toBe(t);
    t.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    flushSync();
    expect(q(".composer")).toBeNull();
    expect(document.activeElement).toBe(q(".report-btn"));
  });

  it("polls Your reports every POLL_MS only while it is open, and shows replies", async () => {
    pages.push({ reports: [report("t1", ["Typo in header", "Fixed, thanks"])], next: null });
    expect(listed).toBe(0);
    click(button("Your reports"));
    await settle();
    expect(listed).toBe(1);
    expect(q(".panel")?.textContent).toMatch(/Typo in header/);
    expect(q(".panel")?.textContent).toMatch(/1 reply/);
    click(q(".panel .row"));
    expect(q(".panel .thread")?.textContent).toMatch(/Fixed, thanks/);
    expect(q(".panel .thread")?.textContent).toMatch(/Triage agent/);
    expect(timers.pending()).toContain(POLL_MS);
    pages.push({ reports: [report("t1", ["Typo in header", "Fixed, thanks", "Deployed"])], next: null });
    timers.fire(POLL_MS);
    await settle();
    expect(listed).toBe(2);
    expect(q(".panel")?.textContent).toMatch(/2 replies/);
    click(q('[aria-label="Close your reports"]'));
    expect(q(".panel")).toBeNull();
    expect(timers.pending()).not.toContain(POLL_MS);
    timers.fire(POLL_MS);
    await settle();
    expect(listed).toBe(2);
  });

  it("marks a report read when it is opened", async () => {
    pages.push({ reports: [report("t1", ["Typo"])], next: null });
    click(button("Your reports"));
    await settle();
    click(q(".panel .row"));
    await settle();
    expect(seen).toEqual(["t1"]);
  });

  it("replies on a report from Your reports", async () => {
    pages.push({ reports: [report("t1", ["Typo"])], next: null });
    click(button("Your reports"));
    await settle();
    click(q(".panel .row"));
    const t = q<HTMLTextAreaElement>(".panel textarea")!;
    expect(t.getAttribute("aria-label")).toBe("Reply");
    t.value = "Still there";
    t.dispatchEvent(new Event("input", { bubbles: true }));
    flushSync();
    click(button("Reply"));
    await settle();
    expect(comments).toEqual([["t1", "Still there"]]);
    expect(q(".panel .thread")?.textContent).toMatch(/Still there/);
  });

  it("fix_proposed report shows It's fixed and Still broken; It's fixed calls verify", async () => {
    pages.push({ reports: [proposed("t1")], next: null });
    click(button("Your reports"));
    await settle();
    expect(q(".panel .state")?.textContent).toBe("Fix ready for you to check");
    click(q(".panel .row"));
    const fix = q(".panel .fix.waiting")!;
    expect(fix.textContent).toMatch(/Fixed the end date in 0\.6\.1/);
    expect(fix.querySelector<HTMLAnchorElement>('a[data-link="change"]')?.href).toBe("https://github.com/o/leave/pull/12");
    expect(fix.querySelector<HTMLAnchorElement>('a[data-link="verify"]')?.href).toBe("https://leave.example/req/1");
    expect(button("Still broken")).toBeDefined();
    const before = contexts;
    click(button("It's fixed"));
    await settle();
    expect(verified.map(([id]) => id)).toEqual(["t1"]);
    expect(verified[0][1]?.context).toMatchObject({ v: 1, release: { version: "0.6.0" } });
    expect(verified[0][1]?.context && "capture" in verified[0][1].context).toBe(false);
    expect(contexts).toBe(before + 1);
    expect(q(".panel .state")?.textContent).toBe("Fixed: you confirmed it");
    expect(button("It's fixed")).toBeUndefined();
  });

  it("Still broken requires a note and sends reopen with the new clip", async () => {
    pages.push({ reports: [proposed("t1")], next: null });
    click(button("Your reports"));
    await settle();
    click(q(".panel .row"));
    click(button("Still broken"));
    await settle();
    // The panel gives way to a note and the mode bar, with no mode on yet.
    expect(q(".panel")).toBeNull();
    const composer = q('.composer[aria-label="Still broken"]')!;
    expect(composer.textContent).toMatch(/The end date is wrong/);
    expect(q(".modebar")).not.toBeNull();
    expect(q('.modebar button[aria-pressed="true"]')).toBeNull();
    expect(modes).toEqual([]);
    expect(button("Send")?.disabled).toBe(true);
    click(button("Element"));
    expect(modes).toEqual([true]);
    onPick(picked(async () => new Uint8Array([5]).buffer));
    await settle();
    expect(q(".modebar")).toBeNull();
    expect(q<HTMLImageElement>(".composer .annotator img")?.getAttribute("src")).toBe("blob:clip");
    typeIn(".composer textarea", "Still says 3 to 7 on the summary");
    click(button("Send"));
    await settle();
    expect(reopened).toHaveLength(1);
    const [id, r] = reopened[0];
    expect(id).toBe("t1");
    expect(r.body).toBe("Still says 3 to 7 on the summary");
    expect(r.clip).toBeInstanceOf(Blob);
    expect(drawn).toEqual([1]);
    expect(r.context).toMatchObject({ v: 1, capture: { mode: "element", method: "dom" } });
    expect(q(".composer")).toBeNull();
    expect(q(".panel")).not.toBeNull();
  });

  it("pulses the launcher once when the badge rises, shows its count, and opens on the waiting report", async () => {
    view.unmount();
    const heard: number[] = [];
    badges.push({ waiting: 0, report_ids: [] }, { waiting: 1, report_ids: ["t1"] });
    view = mount(Overlay as never, props({ onBadge: (n: number) => heard.push(n) }), host);
    await settle();
    expect(heard).toEqual([0]);
    expect(q(".reports-btn .count")).toBeNull();
    window.dispatchEvent(new Event("focus"));
    await settle();
    expect(heard).toEqual([0, 1]);
    expect(q(".reports-btn .count")?.textContent).toBe("1");
    expect(q(".launcher.pulse")).not.toBeNull();
    pages.push({ reports: [report("t0", ["Older"]), proposed("t1")], next: null });
    click(button("Your reports 1"));
    await settle();
    expect(q(".panel li.open")?.textContent).toMatch(/The end date is wrong/);
    expect(seen).toEqual(["t1"]);
  });

  it("while comment mode is on, presses on the overlay stop before the page's listeners, and others go on", () => {
    click(button("Report"));
    const heard: string[] = [];
    const page = (e: Event) => { heard.push((e.target as Element).tagName); };
    document.addEventListener("mousedown", page, true);
    const outside = document.createElement("p");
    document.body.append(outside);
    try {
      q(".modebar button")!.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, composed: true }));
      outside.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, composed: true }));
      click(button("Cancel"));
      expect(heard).toEqual(["P"]);
      click(button("Report"));
      expect(modes.at(-1)).toBe(true);
      click(button("Picking…"));
      q(".launcher button")!.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, composed: true }));
      expect(heard).toEqual(["P", "BUTTON"]);
    } finally {
      document.removeEventListener("mousedown", page, true);
      outside.remove();
    }
  });
});
