import { flushSync } from "svelte";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
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
  return {
    name: `composer:${PICK}`, sent,
    postMessage: (m: unknown) => sent.push(m),
    onMessage: { addListener: (l: (m: unknown) => void) => listeners.push(l) },
    onDisconnect: { addListener: (l: () => void) => gone.push(l) },
    disconnect: () => {},
    tell: (m: unknown) => { listeners.forEach(l => l(m)); flushSync(); },
    drop: () => { gone.forEach(l => l()); flushSync(); },
  };
}
const settle = async () => { for (let i = 0; i < 5; i++) await new Promise(r => setTimeout(r, 0)); flushSync(); };

let port: ReturnType<typeof fakePort>;
let view: Mounted<Record<string, unknown>>;
const urls = URL as unknown as { createObjectURL?: unknown; revokeObjectURL?: unknown };
const had = { create: urls.createObjectURL, revoke: urls.revokeObjectURL };
beforeEach(() => {
  urls.createObjectURL = () => "blob:clip";
  urls.revokeObjectURL = () => {};
  port = fakePort();
  view = mount(ComposerFrame as never, { port, pickId: PICK });
});
afterEach(() => {
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

  it("says when the worker stopped listening, keeping the text to copy", async () => {
    port.tell(draft());
    await settle();
    type("Too wide");
    port.drop();
    expect(notice()).toMatch(/Copy your comment/);
    expect(textarea().value).toBe("Too wide");
  });
});

describe("clipMessage", () => {
  it("says why there is no screenshot, and how to get one", () => {
    expect(clipMessage("no_capture_permission")).toBe("click the Clax button or press ⌥⇧C to comment with a screenshot");
    expect(clipMessage("clip_too_large")).toMatch(/5 MiB/);
    expect(clipMessage("restricted_page")).toMatch(/does not capture/);
    expect(clipMessage("capture_failed")).toMatch(/could not capture/);
    expect(clipMessage("anything_else")).toMatch(/could not capture/);
    expect(clipMessage(null)).toBeUndefined();
  });
});
