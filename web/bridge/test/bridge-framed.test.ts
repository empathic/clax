import { beforeAll, describe, expect, it, vi } from "vitest";
import { commentsContext } from "../src/comments-context";
import { settle } from "../src/parts-static";
import { dispatchTrusted } from "./trusted";

// The bridge framed by a stand-in shell, loaded from <head> while the document
// is still parsing and has no <body> yet.
const posted: { type: string; requestId?: string | null }[] = [];
const shell = { postMessage: (m: { type: string }) => { posted.push(m); } };
const errors: string[] = [];
const setReadyState = (state: DocumentReadyState) => Object.defineProperty(document, "readyState", { value: state, configurable: true });
const send = (data: unknown) => dispatchTrusted(window, new MessageEvent("message", { data, origin: location.origin, source: shell as unknown as Window }));
const frames = () => new Promise<void>(r => requestAnimationFrame(() => requestAnimationFrame(() => r())));
const anchor = { kind: "element", selector: "h2", quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" };
const parsed = () => {
  if (!document.body) document.documentElement.appendChild(document.createElement("body")).innerHTML = "<h2>Goals</h2>";
  setReadyState("interactive");
  document.dispatchEvent(new Event("readystatechange"));
  document.dispatchEvent(new Event("DOMContentLoaded"));
};
const anchorsFor = (requestId: string | null) => posted.filter(m => m.type === "clax:anchors" && m.requestId === requestId);

beforeAll(async () => {
  addEventListener("error", e => { errors.push(e.message); });
  Object.defineProperty(window, "parent", { value: shell, configurable: true });
  setReadyState("loading");
  document.body.remove();
  const script = document.createElement("script");
  script.setAttribute("src", "/_clax/bridge.js?v=0123456789ab");
  script.dataset.artifact = "7q3k9mzx2b4t"; script.dataset.version = "1"; script.dataset.contract = "0.2.61"; script.dataset.file = "index.html"; script.dataset.origin = location.origin;
  document.head.appendChild(script);
  Object.defineProperty(document, "currentScript", { value: script, configurable: true });
  await import("../src/bridge");
  send({ type: "clax:welcome", mode: "view" });
});

describe("bridge orders that arrive before <body>", () => {
  it("a resize or scroll before the parse neither resolves anchors nor throws", async () => {
    expect(document.body).toBeNull();
    send({ type: "clax:resolve-anchors", requestId: "r1", anchors: [{ id: "a", anchor }] });
    dispatchEvent(new Event("resize"));
    dispatchEvent(new Event("scroll"));
    await frames();
    expect(errors).toEqual([]);
    expect(posted.filter(m => m.type === "clax:anchors")).toEqual([]);
    parsed();
    await settle();
    expect(anchorsFor("r1")).toHaveLength(1);
  });

  it("a thread not found re-resolves when the page renders its content later, without a scroll", async () => {
    const late = { ...anchor, selector: "#late" };
    send({ type: "clax:resolve-anchors", requestId: "r3", anchors: [{ id: "b", anchor: late }] });
    await settle();
    expect(anchorsFor("r3")).toEqual([expect.objectContaining({ results: [expect.objectContaining({ found: false })] })]);
    const before = anchorsFor(null).length;
    document.body.insertAdjacentHTML("beforeend", "<div id=\"late\">Rendered late</div>");
    await vi.waitFor(() => expect(anchorsFor(null).length).toBeGreaterThan(before));
    const after = anchorsFor(null).slice(before);
    expect(after).toEqual([expect.objectContaining({ results: [expect.objectContaining({ id: "b", found: true })] })]);
  });

  it("a parse stopped in <head> (no <body>) resolves nothing and does not throw", async () => {
    setReadyState("loading");
    document.body.remove();
    send({ type: "clax:resolve-anchors", requestId: "r4", anchors: [{ id: "a", anchor }] });
    setReadyState("interactive");
    document.dispatchEvent(new Event("readystatechange"));
    await settle();
    expect(errors).toEqual([]);
    expect(anchorsFor("r4")).toEqual([expect.objectContaining({ results: [expect.objectContaining({ found: false })] })]);
  });

  it("a resolution deferred for the parse is dropped when custom anchors went live meanwhile", async () => {
    setReadyState("loading");
    send({ type: "clax:resolve-anchors", requestId: "r2", anchors: [{ id: "a", anchor }] });
    commentsContext.live = true;
    try {
      parsed();
      await settle();
      expect(anchorsFor("r2")).toEqual([]);
    } finally {
      commentsContext.live = false;
    }
  });
});

describe("leaving the page", () => {
  it("says clax:bye on pagehide, and nothing when the page is kept in the back/forward cache", () => {
    const byes = () => posted.filter(m => m.type === "clax:bye").length;
    dispatchEvent(new PageTransitionEvent("pagehide", { persisted: true }));
    expect(byes()).toBe(0);
    dispatchEvent(new PageTransitionEvent("pagehide", { persisted: false }));
    expect(byes()).toBe(1);
  });
});

describe("the shell's request to greet again", () => {
  it("is answered with the page's hello, marked as this document's again", () => {
    posted.length = 0;
    send({ type: "clax:greet" });
    expect(posted).toEqual([{ type: "clax:hello", artifact: "7q3k9mzx2b4t", version: 1, file: "index.html", again: true }]);
  });
});

describe("the shell's storage-access request", () => {
  it("is answered with what the browser decided", async () => {
    const answers = () => posted.filter(m => m.type === "clax:storage-access");
    const decide = (v: unknown) => Object.defineProperty(document, "requestStorageAccess", { value: v, configurable: true });
    try {
      for (const [ask, granted] of [[async () => undefined, true], [async () => { throw new DOMException("no", "NotAllowedError"); }, false], [undefined, false]] as const) {
        posted.length = 0;
        decide(ask);
        send({ type: "clax:storage-access" });
        await expect.poll(answers).toEqual([{ type: "clax:storage-access", granted }]);
      }
    } finally {
      delete (document as { requestStorageAccess?: unknown }).requestStorageAccess;
    }
  });
});

describe("the shell's fragment", () => {
  it("moves the page itself, in place", async () => {
    const entries = history.length;
    const path = location.pathname;
    send({ type: "clax:hash", hash: "#goals" });
    expect(location.hash).toBe("#goals");
    expect(location.pathname).toBe(path);
    // Only ever the fragment changes, whatever the value.
    for (const [hash, to] of [["?x", "#x"], ["/x", "#x"], ["//evil", "#/evil"]]) {
      send({ type: "clax:hash", hash });
      expect(location.pathname + location.search + location.hash).toBe(path + to);
    }
    // Against the page's own URL, not a <base href> naming another site.
    const base = document.createElement("base");
    base.href = "http://other.test/cdn/";
    document.head.append(base);
    const origin = location.origin;
    send({ type: "clax:hash", hash: "#based" });
    base.remove();
    expect(location.origin + location.pathname + location.hash).toBe(origin + path + "#based");
    send({ type: "clax:hash", hash: "" });
    expect(location.hash).toBe("");
    expect(history.length).toBe(entries);
  });
});

describe("the app shell's scroll", () => {
  it("is reported only after a welcome with app: true, which also restores the position it carries", async () => {
    const scrolls = () => posted.filter(m => m.type === "clax:scroll");
    const scrollTo = vi.spyOn(window, "scrollTo").mockImplementation(() => {});
    Object.defineProperty(document.documentElement, "scrollHeight", { value: 5000, configurable: true });
    try {
      posted.length = 0;
      send({ type: "clax:welcome", mode: "view", restoreScroll: 300 });
      await settle();
      dispatchEvent(new Event("scroll"));
      expect(scrolls()).toEqual([]);
      expect(scrollTo).not.toHaveBeenCalled();
      send({ type: "clax:welcome", mode: "view", app: true, restoreScroll: 420 });
      // The report and the restore are in the caps part, loaded on that welcome.
      await settle();
      expect(scrollTo).toHaveBeenCalledWith(0, 420);
      Object.defineProperty(window, "scrollY", { value: 420, configurable: true });
      dispatchEvent(new Event("scroll"));
      expect(scrolls()).toEqual([{ type: "clax:scroll", file: "index.html", y: 420 }]);
    } finally {
      scrollTo.mockRestore();
      Object.defineProperty(window, "scrollY", { value: 0, configurable: true });
      Object.defineProperty(document.documentElement, "scrollHeight", { value: 0, configurable: true });
    }
    // The scroll's reflow of the earlier tests' anchors runs on the next frame.
    await frames();
  });
});
