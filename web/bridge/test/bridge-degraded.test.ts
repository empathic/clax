import { beforeAll, describe, expect, it } from "vitest";
import { RETRY_MS } from "../src/part-loader";
import { failParts, requests, settle } from "../src/parts-static";
import { dispatchTrusted } from "./trusted";

// The bridge framed by a stand-in shell on a parsed page, with parts that
// cannot load: what it reports, and what the page and the shell get instead.
type Msg = { type: string; part?: string; pickId?: string; clipError?: string; clipPng?: unknown; id?: string; name?: string };
const posted: Msg[] = [];
const shell = { postMessage: (m: Msg) => { posted.push(m); } };
const send = (data: unknown) => dispatchTrusted(window, new MessageEvent("message", { data, origin: location.origin, source: shell as unknown as Window }));
const degraded = (part: string) => posted.filter(m => m.type === "clax:degraded" && m.part === part);
/** The shell's test clock (shell/src/clock.ts), which the bridge's part retries read. */
const claxTestClock = () => (globalThis as unknown as { claxTestClock: { advance(ms: number): number } }).claxTestClock;
const press = (el: Element) => { for (const t of ["mousedown", "mouseup", "click"]) dispatchTrusted(el, new MouseEvent(t, { bubbles: true, cancelable: true })); };

beforeAll(async () => {
  Object.defineProperty(window, "parent", { value: shell, configurable: true });
  document.body.innerHTML = `<h2 id="h">Goals</h2>`;
  const script = document.createElement("script");
  script.setAttribute("src", "/_clax/bridge.js?v=0123456789ab");
  script.dataset.artifact = "7q3k9mzx2b4t"; script.dataset.version = "1"; script.dataset.contract = "0.2.61"; script.dataset.file = "index.html"; script.dataset.origin = location.origin;
  document.head.appendChild(script);
  Object.defineProperty(document, "currentScript", { value: script, configurable: true });
  failParts(["caps", "clip", "room"]);
  await import("../src/bridge");
});

describe("parts that cannot load", () => {
  it("are reported to the shell once it has welcomed the page, once per failed attempt", async () => {
    // `db` is the page's own origin's: its members and the origin's client
    // load at once, framed or not, with no shell to report a failure to yet.
    const use = (window as unknown as { claude: { use(n: string): Promise<unknown> } }).claude.use("db");
    // M2: a capability whose page-side members cannot load resolves null, as a refused one.
    await expect(use).resolves.toBeNull();
    await settle();
    expect(requests.filter(r => r === "caps")).toHaveLength(1);
    expect(posted.filter(m => m.type === "clax:degraded")).toEqual([]);
    expect(posted.filter(m => m.type === "clax:use")).toEqual([]);
    send({ type: "clax:welcome", mode: "view" });
    await settle();
    // After the backoff, the next need loads again, and its failure is reported.
    claxTestClock().advance(RETRY_MS);
    const again = (window as unknown as { claude: { use(n: string): Promise<unknown> } }).claude.use("comments");
    await expect(again).resolves.toBeNull();
    await settle();
    expect(requests.filter(r => r === "caps")).toHaveLength(2);
    expect(degraded("caps")).toHaveLength(1);
    // A further need inside the backoff neither loads again nor reports again.
    const third = (window as unknown as { claude: { use(n: string): Promise<unknown> } }).claude.use("downloads");
    await expect(third).resolves.toBeNull();
    await settle();
    expect(requests.filter(r => r === "caps")).toHaveLength(2);
    expect(degraded("caps")).toHaveLength(1);
  });

  it("a room part that cannot load resolves use(\"room\") null and is reported as room", async () => {
    failParts(["clip", "room"]);
    claxTestClock().advance(RETRY_MS * 2);
    // The site declares `room`.
    const real = globalThis.fetch;
    globalThis.fetch = (async (url: string) => new Response(JSON.stringify(String(url).endsWith("/artifact") ? { artifact: { capabilities: { room: {} } } } : { kind: "visitor", level: "public", can_edit: false }))) as typeof fetch;
    try {
      const use = (window as unknown as { claude: { use(n: string): Promise<unknown> } }).claude.use("room");
      await expect(use).resolves.toBeNull();
      await settle();
    } finally {
      globalThis.fetch = real;
    }
    expect(requests).toContain("room");
    expect(degraded("room")).toHaveLength(1);
    // Never asked of the shell: the room is the origin's.
    expect(posted.filter(m => m.type === "clax:use" && m.name === "room")).toEqual([]);
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
