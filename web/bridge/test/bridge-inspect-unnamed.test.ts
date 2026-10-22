import { beforeAll, describe, expect, it, vi } from "vitest";
import { dispatchTrusted } from "./trusted";

// The bridge framed by a shell that did not name the frame for inspection
// (any view but the app's inspect view): the page's console is left alone.
vi.mock("../src/inspect-render", () => ({ renderViewport: vi.fn(async () => new Uint8Array([1]).buffer) }));

type Posted = { type: string; requestId?: string; [k: string]: unknown };
const posted: Posted[] = [];
const shell = { postMessage: (m: Posted) => { posted.push(m); } };
const send = (data: unknown) => dispatchTrusted(window, new MessageEvent("message", { data, origin: location.origin, source: shell as unknown as Window }));
const pageConsoleError = () => {};

beforeAll(async () => {
  Object.defineProperty(window, "parent", { value: shell, configurable: true });
  window.name = "";
  console.error = pageConsoleError;
  const script = document.createElement("script");
  script.setAttribute("src", "/_clax/bridge.js?v=0123456789ab");
  script.dataset.artifact = "7q3k9mzx2b4t"; script.dataset.version = "1"; script.dataset.contract = "0.2.61"; script.dataset.file = "index.html";
  document.head.appendChild(script);
  Object.defineProperty(document, "currentScript", { value: script, configurable: true });
  await import("../src/bridge");
  send({ type: "clax:welcome", mode: "view" });
});

describe("the bridge outside the inspect view", () => {
  it("leaves console.error as the page found it", () => {
    expect(console.error).toBe(pageConsoleError);
  });

  it("runs no tests and no migration", async () => {
    send({ type: "clax:test", requestId: "t1", files: [] });
    send({ type: "clax:migrate", requestId: "m1", file: "migrate.js", from: 1, to: 2 });
    send({ type: "clax:inspect", requestId: "u0", settleMs: 0 });
    await vi.waitFor(() => expect(posted.filter(m => m.type === "clax:inspection")).toHaveLength(1), { timeout: 3000 });
    expect(posted.filter(m => m.type === "clax:tests" || m.type === "clax:migration")).toEqual([]);
    posted.length = 0;
  });

  it("still answers an inspection, with no errors kept", async () => {
    console.error("not kept");
    send({ type: "clax:inspect", requestId: "u1", settleMs: 0 });
    await vi.waitFor(() => expect(posted.filter(m => m.type === "clax:inspection")).toHaveLength(1), { timeout: 3000 });
    expect(posted.find(m => m.type === "clax:inspection")).toMatchObject({ requestId: "u1", errors: [] });
  });
});
