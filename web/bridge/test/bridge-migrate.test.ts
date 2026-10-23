import { beforeAll, describe, expect, it, vi } from "vitest";
import { dispatchTrusted } from "./trusted";

// The bridge in the app's migration view: the shell named the frame
// `clax-migrate` before the page loaded. The bridge, the document's first
// script, ends the page's markup while the document still parses, and runs
// an app bundle's migration on clax:migrate.
type Posted = { type: string; requestId?: string; [k: string]: unknown };
const posted: Posted[] = [];
const shell = { postMessage: (m: Posted) => { posted.push(m); } };
const send = (data: unknown) => dispatchTrusted(window, new MessageEvent("message", { data, origin: location.origin, source: shell as unknown as Window }));
const written: string[] = [];

beforeAll(async () => {
  Object.defineProperty(window, "parent", { value: shell, configurable: true });
  window.name = "clax-migrate";
  // As the parser runs the bridge: the document is still loading.
  Object.defineProperty(document, "readyState", { value: "loading", configurable: true });
  vi.spyOn(document, "write").mockImplementation((...s: string[]) => { written.push(s.join("")); });
  const script = document.createElement("script");
  script.setAttribute("src", "/_clax/bridge.js?v=0123456789ab");
  script.dataset.artifact = "7q3k9mzx2b4t"; script.dataset.version = "1"; script.dataset.contract = "0.2.61"; script.dataset.file = "index.html"; script.dataset.origin = location.origin;
  document.head.appendChild(script);
  Object.defineProperty(document, "currentScript", { value: script, configurable: true });
  await import("../src/bridge");
  Object.defineProperty(document, "readyState", { value: "complete", configurable: true });
  document.dispatchEvent(new Event("readystatechange"));
  send({ type: "clax:welcome", mode: "view", app: true });
});

describe("the bridge in the migration view", () => {
  it("ends the page's markup before any of it parses", () => {
    expect(written).toEqual(["<plaintext hidden>"]);
  });

  it("runs the test part's migration on clax:migrate and answers clax:migration with the requestId; runs no tests", async () => {
    send({ type: "clax:test", requestId: "t1", files: [] });
    send({ type: "clax:migrate", requestId: "m1", file: "no-such-migrate.js", from: 1, to: 2, totalMs: 2000 });
    await vi.waitFor(() => expect(posted.filter(m => m.type === "clax:migration")).toHaveLength(1), { timeout: 3000 });
    expect(posted.find(m => m.type === "clax:migration")).toEqual({ type: "clax:migration", requestId: "m1", ok: false, error: expect.stringContaining("no-such-migrate.js did not load") });
    expect(posted.filter(m => m.type === "clax:tests")).toEqual([]);
  });
});
