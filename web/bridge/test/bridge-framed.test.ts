import { beforeAll, describe, expect, it } from "vitest";
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
  script.dataset.artifact = "7q3k9mzx2b4t"; script.dataset.version = "1"; script.dataset.contract = "0.2.61"; script.dataset.file = "index.html";
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
    await frames();
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

describe("the shell's fragment", () => {
  it("moves the page itself, in place", async () => {
    const entries = history.length;
    const path = location.pathname;
    send({ type: "clax:hash", hash: "#goals" });
    expect(location.hash).toBe("#goals");
    expect(location.pathname).toBe(path);
    send({ type: "clax:hash", hash: "" });
    expect(location.hash).toBe("");
    expect(history.length).toBe(entries);
  });
});
