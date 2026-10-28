import { flushSync } from "svelte";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { Anchor } from "../../bridge/src/protocol";
import type { NewReport } from "../../sdk/src/reporter";
import type { Report, ReportPage } from "../../sdk/src/types";
import { mount, type Mounted } from "../../shell/src/test/svelte";
import { POLL_MS, type ReporterApi, type Timers } from "./model";
import Overlay from "./Overlay.svelte";
import type { Picked, Picker } from "./picker";

const anchor: Anchor = { kind: "element", selector: "#total", quote: "Total: $12", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" };

function report(id: string, bodies: string[]): Report {
  return {
    id, queue_id: "q1", artifact: { id: "a1", external_id: "e", title: "Shop", url: "https://shop.example/", metadata: null, metadata_version: 0, revision: null },
    anchor, status: "open", state: "open", reporter_id: "u1", metadata: null, metadata_version: 0, has_clip: false, claim: null, created_at: "",
    comments: bodies.map((body, i) => ({ id: `${id}c${i}`, author_kind: i ? "agent" : "reporter", author_name: i ? "Triage agent" : "u1", body, created_at: "" })),
  };
}

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

let view: Mounted<Record<string, unknown>>;
let modes: boolean[];
let done: number;
let onPick: (p: Picked) => void;
let onCancel: () => void;
let filed: NewReport[];
let pages: ReportPage[];
let listed: number;
let comments: [string, string][];
let timers: ReturnType<typeof manualTimers>;
let host: HTMLElement;
const urls = URL as unknown as { createObjectURL?: unknown; revokeObjectURL?: unknown };
const had = { create: urls.createObjectURL, revoke: urls.revokeObjectURL };

beforeEach(() => {
  urls.createObjectURL = () => "blob:clip";
  urls.revokeObjectURL = () => {};
  modes = [];
  done = 0;
  filed = [];
  pages = [];
  listed = 0;
  comments = [];
  timers = manualTimers();
  const client: ReporterApi = {
    fileReport: async r => { filed.push(r); return report("t9", [r.body]); },
    myReports: async () => { listed++; return pages.shift() ?? { reports: [], next: null }; },
    comment: async (id, body) => { comments.push([id, body]); return report(id, ["first", body]); },
  };
  const picker = (p: (x: Picked) => void, c: () => void): Picker => {
    onPick = p;
    onCancel = c;
    return { set: on => modes.push(on), done: () => { done++; }, destroy: () => {} };
  };
  host = document.createElement("div");
  document.body.append(host);
  view = mount(Overlay as never, { client, doc: document, host, picker, externalId: "shop-cart", metadata: { build: "7" }, timers }, host);
});
afterEach(() => {
  view.unmount();
  host.remove();
  urls.createObjectURL = had.create;
  urls.revokeObjectURL = had.revoke;
});

const q = <E extends HTMLElement = HTMLElement>(s: string) => view.root.querySelector<E>(s);
const button = (label: string) => [...view.root.querySelectorAll("button")].find(b => b.textContent?.trim() === label);
const type = (v: string) => { const t = q<HTMLTextAreaElement>(".composer textarea")!; t.value = v; t.dispatchEvent(new Event("input", { bubbles: true })); flushSync(); };
const click = (b: HTMLElement | undefined | null) => { if (!b) throw new Error("no such button"); b.click(); flushSync(); };

describe("the overlay", () => {
  it("starts comment mode from the launcher, and Cancel or Escape ends it", () => {
    expect(q(".launcher")).not.toBeNull();
    click(button("Report"));
    expect(modes).toEqual([true]);
    expect(q(".hint")?.textContent).toMatch(/Click what your report is about/);
    click(q(".hint button"));
    expect(modes).toEqual([true, false]);
    expect(q(".hint")).toBeNull();
    click(button("Report"));
    onCancel();
    flushSync();
    expect(modes).toEqual([true, false, true, false]);
  });

  it("opens the composer at a pick with the clip's preview, and sends the report with it", async () => {
    click(button("Report"));
    let rendered: (b: ArrayBuffer) => void = () => {};
    onPick({ anchor, render: () => new Promise(r => { rendered = r; }) });
    flushSync();
    await settle();
    expect(modes.at(-1)).toBe(false);
    expect(q(".composer .quote")?.textContent).toBe("«Total: $12»");
    expect(q(".composer [role=status]")?.textContent).toMatch(/Taking the screenshot/);
    rendered(new Uint8Array([137, 80, 78, 71]).buffer);
    await settle();
    expect(done).toBe(1);
    expect(q<HTMLImageElement>(".composer img.clip")?.getAttribute("src")).toBe("blob:clip");
    expect(button("Send")?.disabled).toBe(true);
    type("The total should include tax");
    click(button("Send"));
    await settle();
    expect(filed).toHaveLength(1);
    expect(filed[0]).toMatchObject({ body: "The total should include tax", anchor, externalId: "shop-cart", metadata: { build: "7" }, url: location.href });
    expect(filed[0].clip).toBeInstanceOf(Blob);
    expect(q(".composer")).toBeNull();
    expect(q(".notice")?.textContent).toMatch(/Report sent/);
  });

  it("sends without a clip when the clip cannot be drawn", async () => {
    click(button("Report"));
    onPick({ anchor, render: () => Promise.reject(new Error("a cross-origin image")) });
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
    onPick({ anchor, render: () => new Promise(r => { rendered = r; }) });
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
    view.unmount();
    const client: ReporterApi = { fileReport: async () => { throw Object.assign(new Error("x"), { code: "network" }); }, myReports: async () => ({ reports: [], next: null }), comment: async () => report("t", []) };
    view = mount(Overlay as never, { client, doc: document, host, picker: (p: (x: Picked) => void) => { onPick = p; return { set: () => {}, done: () => {}, destroy: () => {} }; }, timers }, host);
    click(button("Report"));
    onPick({ anchor, render: async () => new ArrayBuffer(0) });
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
    onPick({ anchor, render: async () => new ArrayBuffer(0) });
    await settle();
    click(button("Cancel"));
    expect(q(".composer")).toBeNull();
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

  it("replies on a report from Your reports", async () => {
    pages.push({ reports: [report("t1", ["Typo"])], next: null });
    click(button("Your reports"));
    await settle();
    click(q(".panel .row"));
    const t = q<HTMLTextAreaElement>(".panel textarea")!;
    t.value = "Still there";
    t.dispatchEvent(new Event("input", { bubbles: true }));
    flushSync();
    click(button("Reply"));
    await settle();
    expect(comments).toEqual([["t1", "Still there"]]);
    expect(q(".panel .thread")?.textContent).toMatch(/Still there/);
  });

  it("while comment mode is on, presses on the overlay stop before the page's listeners, and others go on", () => {
    click(button("Report"));
    const heard: string[] = [];
    const page = (e: Event) => { heard.push((e.target as Element).tagName); };
    document.addEventListener("mousedown", page, true);
    const outside = document.createElement("p");
    document.body.append(outside);
    try {
      q(".hint button")!.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, composed: true }));
      outside.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, composed: true }));
      click(q(".hint button"));
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
