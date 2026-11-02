// The overlay with plugins: what each first-party plugin adds, and that the
// core shows nothing a plugin does not contribute.
import { render } from "@testing-library/svelte";
import { flushSync } from "svelte";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { Anchor } from "../../bridge/src/protocol";
import { type ClaxPlugin, definePlugin, PLUGIN_API_VERSION, PluginHost } from "../../sdk/src/plugin";
import type { NewReport } from "../../sdk/src/reporter";
import type { Report } from "../../sdk/src/types";
import { mount, type Mounted } from "../../shell/src/test/svelte";
import type { FlattenDeps, Surface } from "./annotate";
import { type ReporterApi, Session } from "./model";
import Overlay from "./Overlay.svelte";
import type { Picked, Picker } from "./picker";
import {
  annotate, appVersion, browser, classic, codeBlur, codeLine, codeRegions, diagOf, domCapture, elementPicker, EXTENSION_ATTR, EXTENSION_EVENT,
  extensionPresent, extensionPresentation, handToExtension, locateCode, logs, pagePicker, presentations, windowPresentation,
} from "./plugins";

const anchor: Anchor = { kind: "element", selector: "#total", quote: "Total", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" };
const PNG = new Uint8Array([137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 100, 0, 0, 0, 20]).buffer;
const settle = async () => { for (let i = 0; i < 10; i++) await new Promise(r => setTimeout(r, 0)); flushSync(); };
const pickOf = (target?: Element | Range): Picked => ({ anchor, mode: "element", target, rect: () => ({ x: 0, y: 0, w: 100, h: 20 }), render: async () => ({ png: PNG, omitted: [], at: { x: 0, y: 0, w: 100, h: 20 }, scale: 1 }) });

let view: Mounted<Record<string, unknown>>;
let host: HTMLElement;
let filed: NewReport[];
let onPick: (p: Picked) => void;
const urls = URL as unknown as { createObjectURL?: unknown; revokeObjectURL?: unknown };
const had = { create: urls.createObjectURL, revoke: urls.revokeObjectURL };

const client = (): ReporterApi => ({
  fileReport: async r => { filed.push(r); return { id: "r1" } as Report; },
  myReports: async () => ({ reports: [], next: null }),
  comment: async () => ({}) as Report,
  verify: async () => ({}) as Report,
  reopen: async () => ({}) as Report,
  badge: async () => ({ waiting: 0, report_ids: [] }),
  seen: async () => {},
});
const picker = () => (p: (x: Picked) => void): Picker => { onPick = p; return { set: () => {}, areasOnly: () => {}, keyboard: () => {}, done: () => {}, destroy: () => {} }; };
const session = () => new Session({ context: () => ({ release: { version: "host" }, app: { view: "cart" } }), recorder: { lines: () => [], count: () => 0, stop: () => {} }, now: () => new Date("2026-11-02T12:00:00Z") }, window);

/** Flattening without a canvas: records the PNG it was given, and answers a new one. */
let decoded: Blob[];
const flatten: FlattenDeps = {
  decode: async png => { decoded.push(png); return { width: 100, height: 20, image: {} as CanvasImageSource }; },
  surface: () => ({ ctx: new Proxy({}, { get: (_, k) => (k === "measureText" ? () => ({ width: 10 }) : () => {}), set: () => true }) as Surface["ctx"], png: async () => new Blob([new Uint8Array([1])], { type: "image/png" }) }),
};

function start(plugins: ClaxPlugin[], over: Record<string, unknown> = {}): void {
  view = mount(Overlay as never, { client: client(), doc: document, host, picker: picker(), session: session(), capture: { supported: false }, timers: { set: () => 0, clear: () => {} }, flatten, plugins: new PluginHost(plugins), ...over }, host);
}
const q = <E extends Element = HTMLElement>(s: string, root: ParentNode = view.root) => root.querySelector<E>(s);
const button = (label: string, root: ParentNode = view.root) => [...root.querySelectorAll("button")].find(b => b.textContent?.trim() === label) ?? null;
const click = (b: Element | null) => { if (!b) throw new Error("no such button"); (b as HTMLElement).click(); flushSync(); };
const type = (v: string, root: ParentNode = view.root) => { const t = q<HTMLTextAreaElement>(".composer textarea", root)!; t.value = v; t.dispatchEvent(new Event("input", { bubbles: true })); flushSync(); };
const tick = (label: string) => { const c = q<HTMLInputElement>(`input[aria-label="${label}"]`)!; c.click(); flushSync(); };

beforeEach(() => {
  urls.createObjectURL = () => "blob:clip";
  urls.revokeObjectURL = () => {};
  filed = [];
  decoded = [];
  host = document.createElement("div");
  document.body.append(host);
});
afterEach(() => {
  view?.unmount();
  host.remove();
  document.body.innerHTML = "";
  document.documentElement.removeAttribute(EXTENSION_ATTR);
  urls.createObjectURL = had.create;
  urls.revokeObjectURL = had.revoke;
});

describe("the core shows only what plugins contribute", () => {
  it("with no capture plugin, a pick makes a report with no screenshot and no screenshot controls", async () => {
    start([elementPicker(), presentations()]);
    click(button("Report"));
    onPick(pickOf());
    await settle();
    expect(q(".composer")).not.toBeNull();
    expect(q(".annotator")).toBeNull();
    expect(q('input[aria-label="Include screenshot"]')).toBeNull();
    expect(button("Add a screenshot")).toBeNull();
    type("No picture");
    click(button("Send"));
    await settle();
    expect(filed[0].clip).toBeUndefined();
    expect(filed[0].anchor).toEqual(anchor);
  });

  it("with no annotate plugin, the screenshot shows without tools; with one, its tools only", async () => {
    start([domCapture(), elementPicker(), presentations()]);
    click(button("Report"));
    onPick(pickOf());
    await settle();
    expect(q(".composer img.shot")).not.toBeNull();
    expect(q(".annotator")).toBeNull();
    view.unmount();
    start([domCapture(), annotate({ tools: ["highlight", "text"] }), elementPicker(), presentations()]);
    click(button("Report"));
    onPick(pickOf());
    await settle();
    expect([...view.root.querySelectorAll(".annotator .tools button[aria-pressed]")].map(b => b.textContent)).toEqual(["Highlight", "Text"]);
  });

  it("offers the modes the pickers give", () => {
    start([elementPicker(), pagePicker({ label: "This page" }), presentations()]);
    click(button("Report"));
    expect([...view.root.querySelectorAll(".modebar .modes button")].map(b => b.getAttribute("aria-keyshortcuts"))).toEqual(["E", "S"]);
    expect(view.root.querySelector(".modebar .modes button:last-child")?.textContent).toBe("This pageS");
  });

  it("keeps one notices zone, empty until there is a notice", () => {
    start(classic());
    const zones = view.root.querySelectorAll(".notice[role=status]");
    expect(zones).toHaveLength(1);
    expect(zones[0].textContent).toBe("");
  });
});

describe("presentations", () => {
  it("dialog: a modal dialog with its heading and Close · Esc, over a scrim", async () => {
    start(classic(), { presentation: "dialog" });
    click(button("Report"));
    onPick(pickOf());
    await settle();
    const frame = q('.dock[data-layout="dialog"] .frame[role="dialog"]')!;
    expect(frame.getAttribute("aria-modal")).toBe("true");
    expect(q("#cx-composer-title", frame)?.textContent).toBe("Send feedback");
    expect(q(".scrim")).not.toBeNull();
    click(button("Close · Esc"));
    expect(q(".composer")).toBeNull();
  });

  it.each(["drawer", "sheet"])("%s: a labelled region in its own layout, with no modal", async layout => {
    start(classic(), { presentation: layout });
    click(button("Report"));
    onPick(pickOf());
    await settle();
    const frame = q(`.dock[data-layout="${layout}"] .frame[role="region"]`)!;
    expect(frame.getAttribute("aria-labelledby")).toBe("cx-composer-title");
    expect(frame.querySelector(".composer")).not.toBeNull();
    expect(q("[aria-modal]")).toBeNull();
  });

  it("dialog: Tab wraps within it, and Escape anywhere in it closes it", async () => {
    start(classic(), { presentation: "dialog" });
    click(button("Report"));
    onPick(pickOf());
    await settle();
    const frame = q('.frame[role="dialog"]')!;
    const send = button("Send")!;
    const close = button("Close · Esc")!;
    type("x");
    send.focus();
    frame.dispatchEvent(new KeyboardEvent("keydown", { key: "Tab", bubbles: true, cancelable: true }));
    expect(document.activeElement).toBe(close);
    frame.dispatchEvent(new KeyboardEvent("keydown", { key: "Tab", shiftKey: true, bubbles: true, cancelable: true }));
    expect(document.activeElement).toBe(send);
    send.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    flushSync();
    expect(q(".composer")).toBeNull();
  });

  it("note first: the composer at once, nothing captured, and Add a screenshot keeps the note", async () => {
    let renders = 0;
    const renderViewport = async () => { renders++; return { png: PNG, omitted: [], at: { x: 0, y: 0, w: 800, h: 600 }, scale: 1 }; };
    const r = render(Overlay as never, { props: { client: client(), doc: document, host, picker: picker(), session: session(), capture: { supported: false, renderViewport }, flatten, timers: { set: () => 0, clear: () => {} }, plugins: new PluginHost(classic()), presentation: "dialog" } as never, target: host });
    const app = r.component as unknown as { report(o: { mode: string }): void };
    flushSync();
    app.report({ mode: "note" });
    flushSync();
    await settle();
    const root: ParentNode = host;
    expect(root.querySelector(".frame[role=dialog] .composer")).not.toBeNull();
    expect(root.querySelector(".modebar")).toBeNull();
    expect(renders).toBe(0);
    expect(root.querySelector(".composer .quote")?.textContent).toBe("What the page shows");
    type("Started with words", root);
    click(button("Add a screenshot", root));
    expect(root.querySelector(".modebar")).not.toBeNull();
    // Screen: the visible page, into this draft.
    click(root.querySelector('.modebar button[aria-keyshortcuts="S"]'));
    await settle();
    expect(renders).toBe(1);
    expect(root.querySelector<HTMLTextAreaElement>(".composer textarea")?.value).toBe("Started with words");
    expect(root.querySelector(".composer .annotator img")?.getAttribute("src")).toBe("blob:clip");
    click(button("Send", root));
    await settle();
    expect(filed.map(f => [f.body, !!f.clip])).toEqual([["Started with words", true]]);
    r.unmount();
  });

  it("extension: handed to the extension when it is there and takes it; a dialog when it is not", async () => {
    const asked: unknown[] = [];
    start([...classic(), extensionPresentation({ handoff: req => { asked.push(req); return true; } })], { presentation: "extension" });
    click(button("Report"));
    // Not there: the dialog stands in, and the report starts on the page.
    expect(asked).toEqual([]);
    expect(q(".modebar")).not.toBeNull();
    view.unmount();
    document.documentElement.setAttribute(EXTENSION_ATTR, "");
    start([...classic(), extensionPresentation({ handoff: req => { asked.push(req); return true; } })], { presentation: "extension" });
    click(button("Report"));
    expect(asked).toEqual([{ mode: "element", url: expect.any(String), title: document.title }]);
    expect(q(".modebar")).toBeNull();
    expect(q(".composer")).toBeNull();
  });

  it("window: the composer is drawn in its own window, opened in the click, and closes when the report is sent", async () => {
    const frame = document.createElement("iframe");
    document.body.append(frame);
    const w = frame.contentWindow!;
    let opened = 0;
    let closed = 0;
    (w as unknown as { close: () => void }).close = () => { closed++; };
    start([...classic(), windowPresentation()], { presentation: "window", openWindow: () => { opened++; return w; } });
    click(button("Report"));
    expect(opened).toBe(1);
    expect(w.document.title).toBe("Send feedback");
    expect(w.document.querySelector(".waiting")?.textContent).toMatch(/Pick what your report is about/);
    onPick(pickOf());
    await settle();
    // Drawn there, not in the page's dock.
    expect(q(".dock .composer")).toBeNull();
    const there = w.document.querySelector(".cx.window")!;
    expect(q(".composer h2", there)?.textContent).toBe("Send feedback");
    type("From the window", there);
    click(button("Send", there));
    await settle();
    expect(filed.map(r => r.body)).toEqual(["From the window"]);
    expect(closed).toBe(1);
  });

  it("window: a blocked window falls back to a dialog on the page", async () => {
    start([...classic(), windowPresentation()], { presentation: "window", openWindow: () => null });
    click(button("Report"));
    onPick(pickOf());
    await settle();
    expect(q('.dock[data-layout="dialog"] .frame[role="dialog"] .composer')).not.toBeNull();
  });
});

describe("attachments", () => {
  it("each has its box and its line in What's sent, and lands on the report's own fields", async () => {
    const lines = [{ at: "2026-11-02T11:59:00Z", level: "request" as const, text: "GET /api/x → 500 (request r-1)" }, { at: "2026-11-02T11:59:01Z", level: "warn" as const, text: "slow\nsecond line" }];
    start([...classic(), logs({ source: () => lines }), appVersion({ release: { version: "1.4.0", commit: "abcdef1234567" } }), browser()]);
    click(button("Report"));
    onPick(pickOf());
    await settle();
    expect(q<HTMLInputElement>('input[aria-label="The app\'s recent log lines"]')!.checked).toBe(false);
    expect(q<HTMLInputElement>('input[aria-label="The app\'s version"]')!.checked).toBe(true);
    expect(q('pre.row[data-row="logs"]')).toBeNull();
    expect(q('pre.row[data-row="version"]')?.textContent).toBe("1.4.0 (abcdef123)");
    tick("The app's recent log lines");
    tick("Your browser");
    expect(q('pre.row[data-row="logs"]')?.textContent).toBe("error GET /api/x → 500 (request r-1)\nwarn slow");
    type("Broken");
    click(button("Send"));
    await settle();
    expect(q('[role="alert"]')?.textContent ?? "").toBe("");
    const r = filed[0];
    expect(r.diagnostics).toEqual([{ at: "2026-11-02T11:59:00Z", level: "error", kind: "request", text: "GET /api/x → 500 (request r-1)" }, { at: "2026-11-02T11:59:01Z", level: "warn", kind: "console", text: "slow" }]);
    expect(r.context?.release).toEqual({ version: "1.4.0", commit: "abcdef1234567" });
    expect(r.context?.app).toEqual({ view: "cart" });
    expect(r.context?.environment?.user_agent).toBeUndefined();
    expect(r.context?.environment?.time_zone).toBeUndefined();
  });

  it("sends nothing of an attachment that gives no line for What's sent", async () => {
    const sneaky = definePlugin({ name: "sneaky", version: "1", apiVersion: PLUGIN_API_VERSION, capabilities: [], contributes: { attachments: [{ id: "sneaky", label: "Sneaky", default: true, collect: () => ({ data: { app: { secret: 1 } }, row: null }) }] } });
    start([...classic(), sneaky]);
    click(button("Report"));
    onPick(pickOf());
    await settle();
    type("x");
    click(button("Send"));
    await settle();
    expect(filed[0].context?.app).toEqual({ view: "cart" });
  });

  it("a before-send listener can stop the send, with its reason", async () => {
    const veto = definePlugin({ name: "veto", version: "1", apiVersion: PLUGIN_API_VERSION, capabilities: [], contributes: {}, setup: ctx => { ctx.on("before-send", m => m.cancel("the queue is closed for maintenance")); } });
    start([...classic(), veto]);
    click(button("Report"));
    onPick(pickOf());
    await settle();
    type("x");
    click(button("Send"));
    await settle();
    expect(filed).toEqual([]);
    expect(q('[role="alert"]')?.textContent).toBe("Not sent: the queue is closed for maintenance");
  });
});

describe("filters", () => {
  it("code-blur runs on the screenshot before the reporter sees it, says so, and turned off gives back the screenshot taken", async () => {
    document.body.insertAdjacentHTML("beforeend", "<pre id=code>secret()</pre>");
    document.getElementById("code")!.getBoundingClientRect = () => ({ left: 10, top: 2, right: 60, bottom: 12, width: 50, height: 10, x: 10, y: 2, toJSON: () => ({}) }) as DOMRect;
    const blurred = new Blob([new Uint8Array([9])], { type: "image/png" });
    const seen: unknown[] = [];
    start([...classic(), codeBlur({ deps: { blur: async (_png, regions) => { seen.push(regions); return blurred; } } })]);
    click(button("Report"));
    onPick(pickOf());
    await settle();
    expect(seen).toEqual([[{ x: 10, y: 2, w: 50, h: 10 }]]);
    expect(view.root.textContent).toMatch(/Code blurred \(1\)/);
    type("x");
    click(button("Send"));
    await settle();
    // The marks are drawn into the blurred screenshot, never the one taken.
    expect(decoded).toEqual([blurred]);
    // A second report, with the blur turned off: the screenshot as taken.
    click(button("Report"));
    onPick(pickOf());
    await settle();
    tick("Blur code");
    await settle();
    type("y");
    click(button("Send"));
    await settle();
    expect(decoded[1]).not.toBe(blurred);
    expect(view.root.textContent).not.toMatch(/Code blurred/);
  });

  it("a filter that fails leaves the screenshot out, never unfiltered", async () => {
    document.body.insertAdjacentHTML("beforeend", "<pre id=code>secret()</pre>");
    document.getElementById("code")!.getBoundingClientRect = () => ({ left: 0, top: 0, right: 10, bottom: 10, width: 10, height: 10, x: 0, y: 0, toJSON: () => ({}) }) as DOMRect;
    start([...classic(), codeBlur({ deps: { blur: async () => { throw new Error("no canvas"); } } })]);
    click(button("Report"));
    onPick(pickOf());
    await settle();
    expect(view.root.textContent).toMatch(/No screenshot: the Blur code filter could not run \(no canvas\)/);
    type("x");
    click(button("Send"));
    await settle();
    expect(filed[0].clip).toBeUndefined();
  });
});

describe("pickers", () => {
  it("code line: a pick in code records its file and lines as context.app.code", async () => {
    document.body.insertAdjacentHTML("beforeend", `<div data-clax-file="src/a.ts" data-clax-line="12" data-clax-side="RIGHT" data-clax-sha="abc1234"><code id=c>x()</code></div>`);
    start([...classic(), codeLine()]);
    click(button("Report"));
    onPick(pickOf(document.getElementById("c")!));
    await settle();
    type("x");
    click(button("Send"));
    await settle();
    expect(filed[0].context?.app).toEqual({ view: "cart", code: { file: "src/a.ts", start: 12, end: 12, side: "RIGHT", sha: "abc1234" } });
  });
});

describe("the plugins on their own", () => {
  it("codeRegions: the outermost code in the image, in its pixels, rounded outward; none of Clax's own", () => {
    document.body.innerHTML = `<pre id=a><code id=b>x</code></pre><clax-overlay><pre id=c>y</pre></clax-overlay>`;
    const rect = (l: number, t: number, w: number, h: number) => () => ({ left: l, top: t, right: l + w, bottom: t + h, width: w, height: h, x: l, y: t, toJSON: () => ({}) }) as DOMRect;
    document.getElementById("a")!.getBoundingClientRect = rect(10.5, 20.2, 100, 30);
    document.getElementById("c")!.getBoundingClientRect = rect(0, 0, 50, 50);
    expect(codeRegions({ doc: document, at: { x: 0, y: 0, w: 80, h: 40 }, scale: 2 }, "pre, code")).toEqual([{ x: 21, y: 40, w: 139, h: 40 }]);
    expect(codeRegions({ doc: document, at: { x: 0, y: 0, w: 5, h: 5 }, scale: 1 }, "pre")).toEqual([]);
    expect(codeRegions({ doc: document, at: { x: 0, y: 0, w: 5, h: 5 }, scale: 1 }, "[[bad")).toEqual([]);
  });

  it("diagOf: one line, scrubbed by the SDK's rules, request lines as errors of kind request", () => {
    expect(diagOf({ at: "t", level: "info", text: "token ghp_abcdefghijklmnopqrstuvwxyz0123456789\nmore" }).text).not.toMatch(/ghp_/);
    expect(diagOf({ at: "t", level: "request", text: "GET / → 502" })).toEqual({ at: "t", level: "error", kind: "request", text: "GET / → 502" });
    expect(diagOf({ at: "x".repeat(65), level: "debug", text: "a" }).at).not.toHaveLength(65);
  });

  it("locateCode: a range across lines of one file and side; nothing across files", () => {
    document.body.innerHTML = `<div data-clax-file="a.ts" data-clax-line="3"><span id=s>a</span></div><div data-clax-file="a.ts" data-clax-line="7"><span id=e>b</span></div><div data-clax-file="b.ts" data-clax-line="1"><span id=o>c</span></div>`;
    const r = document.createRange();
    r.setStart(document.getElementById("e")!.firstChild!, 0);
    r.setEnd(document.getElementById("s")!.firstChild!, 1);
    const across = document.createRange();
    across.setStart(document.getElementById("s")!.firstChild!, 0);
    across.setEnd(document.getElementById("o")!.firstChild!, 1);
    expect(locateCode(document.getElementById("s"))).toEqual({ file: "a.ts", start: 3, end: 3 });
    expect(locateCode(across)).toBeNull();
    expect(locateCode(document.body)).toBeNull();
    void r;
  });

  it("the extension's presence is its attribute; a handoff no one takes is not taken", () => {
    expect(extensionPresent(window)).toBe(false);
    document.documentElement.setAttribute(EXTENSION_ATTR, "");
    expect(extensionPresent(window)).toBe(true);
    const req = { mode: "element" as const, url: "u", title: "t" };
    expect(handToExtension(req, window)).toBe(false);
    const take = (e: Event) => e.preventDefault();
    document.addEventListener(EXTENSION_EVENT, take);
    expect(handToExtension(req, window)).toBe(true);
    document.removeEventListener(EXTENSION_EVENT, take);
  });

  it("logs refuses a missing source", () => {
    expect(() => logs({} as never)).toThrow(/source/);
  });
});
