import { beforeAll, describe, expect, it } from "vitest";
import { requests, settle } from "../src/parts-static";
import { dispatchTrusted } from "./trusted";

// The bridge loaded from <head> while the document is still parsing: no part
// is requested until the parse ends, whatever the shell asks for meanwhile,
// so a part's module load never comes before the page's own import maps.
const posted: { type: string; requestId?: string | null }[] = [];
const shell = { postMessage: (m: { type: string }) => { posted.push(m); } };
const setReadyState = (state: DocumentReadyState) => Object.defineProperty(document, "readyState", { value: state, configurable: true });
const send = (data: unknown) => dispatchTrusted(window, new MessageEvent("message", { data, origin: location.origin, source: shell as unknown as Window }));
const anchor = { kind: "element", selector: "h2", quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" };

beforeAll(async () => {
  Object.defineProperty(window, "parent", { value: shell, configurable: true });
  setReadyState("loading");
  document.body.remove();
  const script = document.createElement("script");
  script.setAttribute("src", "/_clax/bridge.js?v=0123456789ab");
  script.dataset.artifact = "7q3k9mzx2b4t"; script.dataset.version = "1"; script.dataset.contract = "0.2.61"; script.dataset.file = "index.html";
  document.head.appendChild(script);
  Object.defineProperty(document, "currentScript", { value: script, configurable: true });
  await import("../src/bridge");
});

describe("the parts while the page parses", () => {
  it("requests none before the parse ends, then comment mode first", async () => {
    send({ type: "clax:welcome", mode: "comment" });
    send({ type: "clax:comment-mode", on: true });
    send({ type: "clax:resolve-anchors", requestId: "r1", anchors: [{ id: "a", anchor }] });
    send({ type: "clax:key", key: "Alt", down: true });
    await new Promise(r => setTimeout(r, 20));
    expect(requests).toEqual([]);
    document.documentElement.appendChild(document.createElement("body")).innerHTML = "<h2>Goals</h2>";
    setReadyState("interactive");
    document.dispatchEvent(new Event("readystatechange"));
    await settle();
    expect(requests[0]).toBe("comment");
    expect(requests).toContain("clip");
    expect(document.documentElement.style.cursor).toBe("crosshair");
    expect(posted.filter(m => m.type === "clax:anchors" && m.requestId === "r1")).toHaveLength(1);
  });
});
