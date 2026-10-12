import { beforeAll, describe, expect, it, vi } from "vitest";
import { settle } from "../src/parts-static";
import { dispatchTrusted } from "./trusted";

// The bridge framed by a stand-in shell that asks it for an inspection. The
// screenshot library does not run under jsdom: the viewport render is a stub.
vi.mock("../src/inspect-render", () => ({ renderViewport: vi.fn(async (_w: Window, scale: number) => new Uint8Array(scale === 1 ? [1, 2, 3] : [1, 2]).buffer) }));

type Posted = { type: string; requestId?: string; [k: string]: unknown };
const posted: Posted[] = [];
const transfers: unknown[][] = [];
const shell = { postMessage: (m: Posted, _o: string, t: unknown[] = []) => { posted.push(m); transfers.push(t); } };
const page = { postMessage: () => {} };
const send = (data: unknown, source: unknown = shell) => dispatchTrusted(window, new MessageEvent("message", { data, origin: location.origin, source: source as Window }));
const answerTo = (id: string) => posted.filter(m => m.type === "clax:inspection" && m.requestId === id);

let pageError: (m: string) => void;
beforeAll(async () => {
  Object.defineProperty(window, "parent", { value: shell, configurable: true });
  // The console as the bridge finds it (kept quiet here).
  vi.spyOn(console, "error").mockImplementation(() => {});
  const script = document.createElement("script");
  script.setAttribute("src", "/_clax/bridge.js?v=0123456789ab");
  script.dataset.artifact = "7q3k9mzx2b4t"; script.dataset.version = "1"; script.dataset.contract = "0.2.61"; script.dataset.file = "index.html";
  document.head.appendChild(script);
  Object.defineProperty(document, "currentScript", { value: script, configurable: true });
  await import("../src/bridge");
  // A page script, after the bridge: its error is in the inspection.
  pageError = m => console.error(m);
  document.title = "Groceries";
  document.body.innerHTML = "<h1>Groceries</h1><img src=\"https://cdn.test/gone.png\">";
  document.querySelector("img")!.dispatchEvent(new Event("error"));
  send({ type: "clax:welcome", mode: "view", app: true });
});

describe("the bridge's inspection", () => {
  it("answers clax:inspect with the requestId, the png transferred, the page's errors, failed loads and outline", async () => {
    pageError("the page broke");
    vi.useFakeTimers();
    try {
      send({ type: "clax:inspect", requestId: "i1", settleMs: 500, maxBytes: 2048 });
      await vi.advanceTimersByTimeAsync(600);
      await vi.waitFor(() => expect(answerTo("i1")).toHaveLength(1));
    } finally {
      vi.useRealTimers();
    }
    const [a] = answerTo("i1");
    expect(a).toMatchObject({ title: "Groceries", text: "# Groceries", failed: ["https://cdn.test/gone.png"] });
    expect(a.errors).toContainEqual({ kind: "console", text: "the page broke" });
    expect(new Uint8Array(a.png as ArrayBuffer)).toEqual(new Uint8Array([1, 2, 3]));
    expect(transfers[posted.indexOf(a)]).toEqual([a.png]);
  });

  it("answers with an error when the screenshot stays over maxBytes", async () => {
    vi.useFakeTimers();
    try {
      send({ type: "clax:inspect", requestId: "i2", settleMs: 0, maxBytes: 1 });
      await vi.advanceTimersByTimeAsync(10);
      await vi.waitFor(() => expect(answerTo("i2")).toHaveLength(1));
    } finally {
      vi.useRealTimers();
    }
    expect(answerTo("i2")[0]).toEqual({ type: "clax:inspection", requestId: "i2", error: expect.stringMatching(/over/) });
  });

  it("takes the order only from the shell, and only with a requestId", async () => {
    send({ type: "clax:inspect", requestId: "i3", settleMs: 0 }, page);
    send({ type: "clax:inspect", settleMs: 0 });
    await settle();
    await new Promise(r => setTimeout(r, 50));
    expect(posted.filter(m => m.type === "clax:inspection" && (m.requestId === "i3" || m.requestId === undefined))).toEqual([]);
  });
});
