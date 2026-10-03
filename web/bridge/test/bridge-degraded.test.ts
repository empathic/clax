import { beforeAll, describe, expect, it } from "vitest";
import { failParts, requests, settle } from "../src/parts-static";
import { dispatchTrusted } from "./trusted";

// The bridge framed by a stand-in shell on a parsed page, with parts that
// cannot load: what it reports, and what the page and the shell get instead.
type Msg = { type: string; part?: string; pickId?: string; clipError?: string; clipPng?: unknown; id?: string; name?: string };
const posted: Msg[] = [];
const shell = { postMessage: (m: Msg) => { posted.push(m); } };
const send = (data: unknown) => dispatchTrusted(window, new MessageEvent("message", { data, origin: location.origin, source: shell as unknown as Window }));
const degraded = (part: string) => posted.filter(m => m.type === "clax:degraded" && m.part === part);
const press = (el: Element) => { for (const t of ["mousedown", "mouseup", "click"]) dispatchTrusted(el, new MouseEvent(t, { bubbles: true, cancelable: true })); };

beforeAll(async () => {
  Object.defineProperty(window, "parent", { value: shell, configurable: true });
  document.body.innerHTML = `<h2 id="h">Goals</h2>`;
  const script = document.createElement("script");
  script.setAttribute("src", "/_clax/bridge.js?v=0123456789ab");
  script.dataset.artifact = "7q3k9mzx2b4t"; script.dataset.version = "1"; script.dataset.contract = "0.2.61"; script.dataset.file = "index.html";
  document.head.appendChild(script);
  Object.defineProperty(document, "currentScript", { value: script, configurable: true });
  failParts(["caps", "clip", "room"]);
  await import("../src/bridge");
});

describe("parts that cannot load", () => {
  it("are reported only after the welcome, once per failed attempt", async () => {
    // The page asks for a capability before the welcome: nothing is loaded or reported yet.
    const use = (window as unknown as { claude: { use(n: string): Promise<unknown> } }).claude.use("db");
    await settle();
    expect(requests).not.toContain("caps");
    expect(posted.filter(m => m.type === "clax:degraded")).toEqual([]);
    send({ type: "clax:welcome", mode: "view" });
    await settle();
    // The page's use() is now sent; the shell grants it, and the caps part fails.
    const req = posted.find(m => m.type === "clax:use" && m.name === "db")!;
    expect(req).toBeDefined();
    send({ type: "clax:use-result", id: req.id, granted: true, config: {} });
    // M2: a capability whose page-side members cannot load resolves null, as a refused one.
    await expect(use).resolves.toBeNull();
    await settle();
    expect(degraded("caps")).toHaveLength(1);
    // A second need inside the backoff neither loads again nor reports again.
    const again = (window as unknown as { claude: { use(n: string): Promise<unknown> } }).claude.use("comments");
    await settle();
    const req2 = posted.find(m => m.type === "clax:use" && m.name === "comments")!;
    send({ type: "clax:use-result", id: req2.id, granted: true, config: {} });
    await expect(again).resolves.toBeNull();
    await settle();
    expect(requests.filter(r => r === "caps")).toHaveLength(1);
    expect(degraded("caps")).toHaveLength(1);
  });

  it("a room part that cannot load resolves use(\"room\") null and is reported as room, without loading caps", async () => {
    const before = requests.filter(r => r === "caps").length;
    const use = (window as unknown as { claude: { use(n: string): Promise<unknown> } }).claude.use("room");
    await settle();
    const req = posted.find(m => m.type === "clax:use" && m.name === "room")!;
    send({ type: "clax:use-result", id: req.id, granted: true, config: {} });
    await expect(use).resolves.toBeNull();
    await settle();
    expect(degraded("room")).toHaveLength(1);
    expect(requests.filter(r => r === "caps")).toHaveLength(before);
  });

  it("a granted sample loads its own part and resolves its callable namespace, never through caps", async () => {
    const before = requests.filter(r => r === "caps").length;
    const use = (window as unknown as { claude: { use(n: string): Promise<unknown> } }).claude.use("sample");
    await settle();
    const req = posted.find(m => m.type === "clax:use" && m.name === "sample")!;
    send({ type: "clax:use-result", id: req.id, granted: true, config: {} });
    const ns = await use as { json: unknown; limits: unknown };
    expect(typeof ns).toBe("function");
    expect(Object.isFrozen(ns)).toBe(true);
    expect(Object.keys(ns).sort()).toEqual(["json", "limits"]);
    expect(requests).toContain("sample");
    expect(requests.filter(r => r === "caps")).toHaveLength(before);
  });

  it("posts a pick whose clip could not render with the reason, once the composer is ready", async () => {
    send({ type: "clax:comment-mode", on: true });
    await settle();
    expect(document.documentElement.style.cursor).toBe("crosshair");
    expect(degraded("clip")).toHaveLength(1);
    press(document.getElementById("h")!);
    const start = posted.find(m => m.type === "clax:pick-start")!;
    expect(start).toBeDefined();
    send({ type: "clax:composer-ready", pickId: start.pickId });
    await settle();
    const pick = posted.find(m => m.type === "clax:pick" && m.pickId === start.pickId)!;
    expect(pick.clipPng).toBeUndefined();
    expect(pick.clipError).toBe("the clip part is blocked");
    expect(degraded("comment")).toEqual([]);
  });
});
