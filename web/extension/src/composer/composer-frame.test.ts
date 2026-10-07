import { flushSync } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mount, type Mounted } from "../../../shell/src/test/svelte";
import { clipMessage } from "./clip-message";
import ComposerFrame from "./ComposerFrame.svelte";

const PICK = "0123456789abcdef0123456789abcdef";
const T1 = "01J9AAAAAAAAAAAAAAAAAAAAAA";
const anchor = { kind: "element", selector: "#save", quote: "Save", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" };
const draft = (extra: Record<string, unknown> = {}) => ({ t: "draft", anchor, clipUrl: null, clipError: null, capturing: false, ...extra });

function fakePort() {
  const sent: unknown[] = [];
  const listeners: ((m: unknown) => void)[] = [];
  const gone: (() => void)[] = [];
  // Chrome throws on a disconnected port's postMessage.
  let closed = false;
  return {
    name: `composer:${PICK}`, sent,
    postMessage: (m: unknown) => { if (closed) throw new Error("Attempting to use a disconnected port object"); sent.push(m); },
    onMessage: { addListener: (l: (m: unknown) => void) => listeners.push(l) },
    onDisconnect: { addListener: (l: () => void) => gone.push(l) },
    disconnect: () => {},
    tell: (m: unknown) => { listeners.forEach(l => l(m)); flushSync(); },
    drop: () => { closed = true; gone.forEach(l => l()); flushSync(); },
    /** Closed without its disconnect heard yet. */
    close: () => { closed = true; },
  };
}
const settle = async () => { for (let i = 0; i < 5; i++) await new Promise(r => setTimeout(r, 0)); flushSync(); };

let port: ReturnType<typeof fakePort>;
/** The callbacks of the ResizeObservers the composer made (jsdom has none of its own). */
let resized: (() => void)[] = [];
class FakeResizeObserver { constructor(cb: () => void) { resized.push(cb); } observe() {} disconnect() {} }
const w = window as unknown as { ResizeObserver?: unknown };
let view: Mounted<Record<string, unknown>>;
let notes: unknown[];
const urls = URL as unknown as { createObjectURL?: unknown; revokeObjectURL?: unknown };
const had = { create: urls.createObjectURL, revoke: urls.revokeObjectURL };
beforeEach(() => {
  resized = [];
  w.ResizeObserver = FakeResizeObserver;
  urls.createObjectURL = () => "blob:clip";
  urls.revokeObjectURL = () => {};
  port = fakePort();
  notes = [];
  view = mount(ComposerFrame as never, { port, pickId: PICK, notify: (m: unknown) => { notes.push(m); }, shortcut: Promise.resolve("⌥⇧C") });
});
afterEach(() => {
  delete w.ResizeObserver;
  view.unmount();
  view.root.remove();
  urls.createObjectURL = had.create;
  urls.revokeObjectURL = had.revoke;
});
const textarea = () => view.root.querySelector("textarea")!;
const type = (v: string) => { textarea().value = v; textarea().dispatchEvent(new Event("input", { bubbles: true })); flushSync(); };
const notice = () => view.root.querySelector(".notice")?.textContent?.trim() ?? null;
const post = () => { textarea().dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", metaKey: true, bubbles: true, cancelable: true })); flushSync(); };

describe("the composer page", () => {
  it("names the command's shortcut when the pick has no screenshot for want of a grant", async () => {
    port.tell(draft({ clipError: "no_capture_permission" }));
    await settle();
    // The status is spoken two frames after the composer shows.
    await vi.waitFor(() => { flushSync(); expect(view.root.textContent).toContain("No screenshot: press ⌥⇧C on the page before your next pick to include one"); });
  });

  it("asks for its draft, then shows the composer for the pick with its clip", async () => {
    expect(port.sent).toEqual([{ t: "ready" }]);
    expect(view.root.querySelector("textarea")).toBeNull();
    port.tell(draft({ clipUrl: "data:image/png;base64,iVBORw==" }));
    await settle();
    expect(view.root.querySelector(".composer-quote")?.textContent).toBe("«Save»");
    expect(view.root.querySelector("img.clip")?.getAttribute("src")).toBe("blob:clip");
    expect(document.activeElement).toBe(textarea());
  });

  it("takes only the worker's well-formed messages", async () => {
    port.tell({ ...draft(), extra: 1 });
    port.tell(draft({ clipUrl: "https://example.com/x.png" }));
    await settle();
    expect(view.root.querySelector("textarea")).toBeNull();
  });

  it("posts through the worker, and says why a post failed, keeping the text", async () => {
    port.tell(draft());
    await settle();
    type("Too wide");
    post();
    expect(port.sent.at(-1)).toEqual({ t: "post", body: "Too wide" });
    port.tell({ t: "failed", message: "The daemon is not reachable." });
    await settle();
    expect(notice()).toContain("The daemon is not reachable.");
    expect(textarea().value).toBe("Too wide");
    post();
    expect(port.sent.filter(m => (m as { t: string }).t === "post")).toHaveLength(2);
    expect(notice()).toBeNull();
    port.tell({ t: "posted", threadId: T1 });
    await settle();
    expect(notice()).toBeNull();
  });

  it("cancels through the worker", async () => {
    port.tell(draft());
    await settle();
    (view.root.querySelector("button:not(.primary)") as HTMLButtonElement).click();
    expect(port.sent.at(-1)).toEqual({ t: "cancel" });
  });

  it("keeps the draft and says so when focus leaves it mid-draft, until it comes back", async () => {
    port.tell(draft());
    await settle();
    window.dispatchEvent(new Event("blur"));
    flushSync();
    expect(notice()).toBeNull(); // nothing typed yet
    window.dispatchEvent(new Event("focus"));
    type("Too wi");
    window.dispatchEvent(new Event("blur"));
    flushSync();
    expect(notice()).toMatch(/goes to the page/);
    expect(textarea().value).toBe("Too wi");
    window.dispatchEvent(new Event("focus"));
    flushSync();
    expect(notice()).toBeNull();
  });

  it("says nothing when the worker lets go after a post", async () => {
    port.tell(draft());
    await settle();
    type("Too wide");
    post();
    port.tell({ t: "posted", threadId: T1 });
    port.drop();
    expect(notice()).toBeNull();
  });

  it("says nothing when the worker lets go after a cancel", async () => {
    port.tell(draft());
    await settle();
    (view.root.querySelector("button:not(.primary)") as HTMLButtonElement).click();
    port.drop();
    expect(notice()).toBeNull();
  });

  it("restyles the shell composer's own elements, which are there", async () => {
    port.tell(draft({ clipUrl: "data:image/png;base64,iVBORw==" }));
    await settle();
    // ComposerFrame.svelte's styles name these classes of the shell's Composer.
    expect(view.root.querySelector("form.composer img.clip")).not.toBeNull();
  });

  it("says when the worker stopped listening, keeping the text to copy", async () => {
    port.tell(draft());
    await settle();
    type("Too wide");
    port.drop();
    expect(notice()).toMatch(/Copy your comment/);
    expect(textarea().value).toBe("Too wide");
    // The worker hears of it, so the page gets comment mode back.
    expect(notes).toEqual([{ t: "lost", pickId: PICK }]);
    // Cancel then closes the composer through a one-off message, without throwing.
    (view.root.querySelector("button:not(.primary)") as HTMLButtonElement).click();
    expect(notes.at(-1)).toEqual({ t: "dismiss", pickId: PICK });
  });

  it("cancels through a one-off message when the port closed unheard, and a post fails visibly", async () => {
    port.tell(draft());
    await settle();
    type("Too wide");
    port.close();
    post();
    await settle();
    expect(notice()).toMatch(/Copy your comment/);
    (view.root.querySelector("button:not(.primary)") as HTMLButtonElement).click();
    expect(notes).toEqual([{ t: "dismiss", pickId: PICK }]);
  });

  it("tells the worker nothing when it lets go after a cancel or a post", async () => {
    port.tell(draft());
    await settle();
    (view.root.querySelector("button:not(.primary)") as HTMLButtonElement).click();
    port.drop();
    expect(notes).toEqual([]);
  });
});

describe("clipMessage", () => {
  it("says why there is no screenshot, and how to get one", () => {
    // The command grants the screenshot (the toolbar icon would turn Clax off in this tab): named by its shortcut, else the context menu.
    expect(clipMessage("no_capture_permission", "⌥⇧C")).toBe("press ⌥⇧C on the page before your next pick to include one");
    expect(clipMessage("no_capture_permission", null)).toBe("right-click the page and choose Comment with Clax before your next pick to include one");
    expect(clipMessage("clip_too_large")).toMatch(/5 MiB/);
    expect(clipMessage("restricted_page")).toMatch(/does not capture/);
    expect(clipMessage("capture_failed")).toMatch(/could not capture/);
    expect(clipMessage("anything_else")).toMatch(/could not capture/);
    expect(clipMessage(null)).toBeUndefined();
  });
});

describe("the composer frame's size", () => {
  it("tells the worker its content's height whenever it changes, so the overlay fits the frame to it", () => {
    const fit = view.root.querySelector(".fit") as HTMLElement;
    let h = 180;
    fit.getBoundingClientRect = () => ({ height: h }) as DOMRect;
    expect(resized).toHaveLength(1);
    resized[0]();
    resized[0]();
    h = 212.4;
    resized[0]();
    expect(port.sent.filter(m => (m as { t: string }).t === "size")).toEqual([{ t: "size", height: 180 }, { t: "size", height: 213 }]);
  });
});
