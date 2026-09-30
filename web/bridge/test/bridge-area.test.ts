import { afterAll, beforeAll, describe, expect, it } from "vitest";

// The bridge framed by a stand-in shell, on a parsed page: how it passes
// `sameVersion` from the shell's orders to area re-anchoring.
const posted: { type: string; requestId?: string | null; results?: { id: string; found: boolean }[] }[] = [];
const shell = { postMessage: (m: { type: string }) => { posted.push(m); } };
const send = (data: unknown) => window.dispatchEvent(new MessageEvent("message", { data, origin: location.origin, source: shell as unknown as Window }));
const focusBox = () => document.querySelector("clax-overlay")!.shadowRoot!.querySelector<HTMLElement>(".f")!;

// An area drawn on a section that, by selector alone, now finds a section
// with another child count and unlike text: another element on a later
// version, live content on its own. The viewport width differs from the
// drawing's, so only the fingerprint decides.
const area = {
  kind: "area", selector: "#live", quote: null, prefix: null, suffix: null, html_hash: null, custom_name: null, file: "index.html",
  rect: { x: 0, y: 0, w: 100, h: 50, scrollX: 0, scrollY: 0, viewportW: 5000 },
  area: { x: 0, y: 0, w: 0.5, h: 0.5, tag: "section", text: "Quarterly revenue by region and", children: 5 },
};

beforeAll(async () => {
  Object.defineProperty(window, "parent", { value: shell, configurable: true });
  // jsdom does not scroll; the centring of an area is not what is tested here.
  window.scrollBy = () => {};
  document.body.innerHTML = `<section id="live"><h2>Open issues: 43</h2></section>`;
  const script = document.createElement("script");
  script.setAttribute("src", "/_clax/bridge.js?v=0123456789ab");
  script.dataset.artifact = "7q3k9mzx2b4t"; script.dataset.version = "2"; script.dataset.contract = "0.2.61"; script.dataset.file = "index.html";
  document.head.appendChild(script);
  Object.defineProperty(document, "currentScript", { value: script, configurable: true });
  await import("../src/bridge");
  send({ type: "clax:welcome", mode: "view" });
});

// No detached thread is left for the bridge to retry while the test document is torn down.
afterAll(() => { send({ type: "clax:resolve-anchors", requestId: "end", anchors: [] }); });

describe("sameVersion from the shell", () => {
  it("skips the fingerprint when resolving threads of the version shown, and checks it for others", () => {
    send({ type: "clax:resolve-anchors", requestId: "r1", anchors: [{ id: "same", anchor: area, sameVersion: true }, { id: "other", anchor: area, sameVersion: false }, { id: "unsaid", anchor: area }] });
    const res = posted.filter(m => m.type === "clax:anchors" && m.requestId === "r1").at(-1)!.results!;
    expect(res.map(r => [r.id, r.found])).toEqual([["same", true], ["other", false], ["unsaid", false]]);
  });

  it("outlines the focused area by the same rule", () => {
    send({ type: "clax:resolve-anchors", requestId: "r2", anchors: [{ id: "same", anchor: area, sameVersion: true }, { id: "other", anchor: area, sameVersion: false }] });
    send({ type: "clax:focus", id: "other" });
    expect(focusBox().style.display).not.toBe("block");
    send({ type: "clax:focus", id: "same" });
    expect(focusBox().style.display).toBe("block");
    send({ type: "clax:focus", id: null });
    expect(focusBox().style.display).toBe("none");
  });

  it("scrolls to an area of the version shown, and not to one of another version", async () => {
    const wait = () => new Promise(r => setTimeout(r, 450));
    send({ type: "clax:scroll-to", anchor: area, sameVersion: false });
    await wait();
    expect(focusBox().style.display).toBe("none");
    send({ type: "clax:scroll-to", anchor: area, sameVersion: true });
    await wait();
    expect(focusBox().style.display).toBe("block");
  });
});
