import { afterEach, describe, expect, it, vi } from "vitest";
import type { RenderResult } from "../../bridge/src/clip";
import type { DisplayResult } from "../../bridge/src/display-capture";
import type { Picked } from "./picker";

// modern-screenshot is replaced: each render records the size and the move
// of the picture it was asked for, and answers a tiny PNG.
const renders: { width?: number; height?: number; transform?: string }[] = [];
vi.mock("modern-screenshot", () => ({
  createContext: async (_el: Element, opts: { width?: number; height?: number; style?: Record<string, string> }) => {
    renders.push({ width: opts.width, height: opts.height, transform: opts.style?.transform });
    return { opts };
  },
  domToPng: async () => "data:image/png;base64,iVBORw==",
  destroyContext: () => {},
}));

const { captureExact, captureFor, pngSize, steppingAside } = await import("./capture");

const sized = (el: Element, left: number, top: number, width: number, height: number) => {
  (el as HTMLElement).getBoundingClientRect = () => ({ left, top, width, height, right: left + width, bottom: top + height, x: left, y: top, toJSON() {} }) as DOMRect;
};
const png = (...bytes: number[]) => new Uint8Array(bytes).buffer;
/** A blob's bytes (jsdom's Blob has no `arrayBuffer`). */
const bytesOf = (b: Blob) => new Promise<Uint8Array>((resolve, reject) => {
  const r = new FileReader();
  r.onload = () => resolve(new Uint8Array(r.result as ArrayBuffer));
  r.onerror = () => reject(r.error);
  r.readAsArrayBuffer(b);
});

/** An element pick at 100,100 (200 x 100) in the viewport, whose clip shows
 * the region at 50,50 (400 x 300) at `scale` image px per CSS px, and left out
 * `omitted` (in the image's pixels). */
function pick(omitted: RenderResult["omitted"] = [], render?: () => Promise<RenderResult>, scale = 1): Picked {
  return {
    anchor: { selector: "#card" } as Picked["anchor"],
    mode: "element",
    rect: () => ({ x: 100, y: 100, w: 200, h: 100 }),
    render: render ?? (async () => ({ png: png(1, 2, 3), omitted, at: { x: 50, y: 50, w: 400, h: 300 }, scale })),
  };
}

afterEach(() => { renders.length = 0; vi.restoreAllMocks(); });

describe("capture", () => {
  it("screen mode crops to the viewport", async () => {
    const root = document.documentElement;
    sized(root, 0, -300, 1024, 3000);
    vi.spyOn(window, "scrollY", "get").mockReturnValue(300);
    const c = await captureFor("screen", null, window, { supported: false });
    expect(renders).toEqual([{ width: window.innerWidth, height: window.innerHeight, transform: "translate(0px, -300px)" }]);
    expect(c.info).toEqual({ mode: "screen", method: "dom", omitted: [] });
    expect(c.pick).toBeNull();
    expect(c.png).toBeInstanceOf(Blob);
    expect(c.error).toBeNull();
  });

  it("suggests exact when an iframe is omitted under the pick", async () => {
    const under = await captureFor("element", pick([{ kind: "iframe", rect: { x: 150, y: 120, w: 300, h: 200 } }]), window, { supported: true });
    expect(under.suggestExact).toBe(true);
    expect(under.info).toEqual({ mode: "element", method: "dom", omitted: [{ kind: "iframe", rect: { x: 150, y: 120, w: 300, h: 200 } }] });
    // The pick is at 50,50 (200 x 100) in its clip's image: this one is beside it.
    const beside = await captureFor("element", pick([{ kind: "iframe", rect: { x: 300, y: 150, w: 50, h: 50 } }]), window, { supported: true });
    expect(beside.suggestExact).toBe(false);
    // In an image drawn at twice the scale the pick covers 100,100 (400 x 200): under it.
    const sharp = await captureFor("element", pick([{ kind: "iframe", rect: { x: 300, y: 150, w: 50, h: 50 } }], undefined, 2), window, { supported: true });
    expect(sharp.suggestExact).toBe(true);
    // Where the pick shows in the image, for its outline, and the image's scale.
    expect([under.pick, under.scale]).toEqual([{ x: 50, y: 50, w: 200, h: 100 }, 1]);
    expect([sharp.pick, sharp.scale]).toEqual([{ x: 100, y: 100, w: 400, h: 200 }, 2]);
    // Where the browser cannot capture the tab, exact is never suggested.
    const phone = await captureFor("element", pick([{ kind: "iframe", rect: { x: 150, y: 120, w: 300, h: 200 } }]), window, { supported: false });
    expect(phone.suggestExact).toBe(false);
  });

  it("names a range pick's mode, and keeps a failed render's reason", async () => {
    const range = { ...pick(), mode: "range" as const, render: async () => { throw new Error("the anchored element has no size"); } };
    const c = await captureFor("element", range, window, { supported: true });
    expect(c).toMatchObject({ png: null, error: "the anchored element has no size", info: { mode: "range", method: "dom" } });
  });

  it("an exact capture of a pick is cropped to the pick, with the overlay stepped aside", async () => {
    const host = document.createElement("clax-overlay");
    // Another Clax overlay (the extension's) guards its host's style: it is
    // never touched.
    const other = document.createElement("clax-overlay");
    document.documentElement.append(host, other);
    document.body.innerHTML = `<aside id="chat" data-clax-ignore style="opacity: 0.5">chat</aside><div id="sh"></div>`;
    const shadow = document.getElementById("sh")!.attachShadow({ mode: "open" });
    shadow.innerHTML = `<div id="tip" data-clax-ignore>tip</div>`;
    const chat = document.getElementById("chat")!;
    const tip = shadow.getElementById("tip")!;
    const extra = document.createElement("div");
    document.body.append(extra);
    let seen: string[] = [];
    let crop: unknown;
    const display = async (_w: Window, c?: unknown): Promise<DisplayResult> => {
      seen = [host, other, chat, tip, extra].map(e => `${e.style.getPropertyValue("opacity")}${e.style.getPropertyPriority("opacity")}`);
      crop = c;
      return { ok: true, png: png(9), thisTab: true };
    };
    const c = await captureExact("element", pick(), window, { display, host, aside: [extra], painted: async () => {} });
    expect(crop).toEqual({ x: 100, y: 100, w: 200, h: 100 });
    expect(seen).toEqual(["0important", "", "0important", "0important", "0important"]);
    expect([host, other, chat, tip, extra].map(e => e.style.getPropertyValue("opacity"))).toEqual(["", "", "0.5", "", ""]);
    expect(c).toMatchObject({ error: null, note: null, info: { mode: "element", method: "display" } });
    expect(await bytesOf(c.png!)).toEqual(new Uint8Array([9]));
    host.remove();
    other.remove();
    document.body.innerHTML = "";
  });

  it("measures an exact screenshot's scale from its PNG, and places both captures on the page", async () => {
    vi.spyOn(window, "scrollY", "get").mockReturnValue(300);
    // A 300 x 150 PNG of the 200 x 100 crop: shrunk from twice the scale to 1.5.
    const header = new Uint8Array(24);
    header.set([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13, 0x49, 0x48, 0x44, 0x52]);
    new DataView(header.buffer).setUint32(16, 300);
    new DataView(header.buffer).setUint32(20, 150);
    expect(pngSize(header.buffer)).toEqual({ w: 300, h: 150 });
    expect(pngSize(png(1, 2, 3))).toBeNull();
    const display = async (): Promise<DisplayResult> => ({ ok: true, png: header.buffer, thisTab: true });
    const c = await captureExact("element", pick(), window, { display, painted: async () => {} });
    expect([c.scale, c.at, c.size]).toEqual([1.5, { x: 100, y: 400, w: 200, h: 100 }, { w: 300, h: 150 }]);
    const dom = await captureFor("element", pick(), window, { supported: false });
    expect([dom.scale, dom.at]).toEqual([1, { x: 50, y: 350, w: 400, h: 300 }]);
  });

  it("steps back in when the capture fails", async () => {
    const el = document.createElement("div");
    el.style.setProperty("opacity", "0.7", "important");
    await expect(steppingAside([el], () => Promise.reject(new Error("refused")), async () => {})).rejects.toThrow("refused");
    expect([el.style.getPropertyValue("opacity"), el.style.getPropertyPriority("opacity")]).toEqual(["0.7", "important"]);
  });

  it("says when it cannot tell the capture was this tab", async () => {
    const display = async (): Promise<DisplayResult> => ({ ok: true, png: png(9), thisTab: "unknown" });
    const c = await captureExact("screen", null, window, { display, painted: async () => {} });
    expect(c.note).toMatch(/this tab/);
  });

  it("exact failure keeps the dom clip with a message", async () => {
    const denied = async (): Promise<DisplayResult> => ({ ok: false, reason: "denied", message: "denied by the reporter" });
    const c = await captureExact("element", pick(), window, { display: denied, painted: async () => {} });
    expect(c.error).toBe("You cancelled screen sharing — the page render is used instead.");
    expect(c.info.method).toBe("dom");
    expect(await bytesOf(c.png!)).toEqual(new Uint8Array([1, 2, 3]));
    const other = async (): Promise<DisplayResult> => ({ ok: false, reason: "other_surface", message: "a window" });
    const fallback = await captureFor("element", pick(), window, { supported: true });
    const o = await captureExact("element", pick([], () => Promise.reject(new Error("not rendered again"))), window, { display: other, painted: async () => {}, fallback });
    expect(o.error).toBe("That was another tab or window — the page render is used instead.");
    expect(o.png).toBe(fallback.png);
  });
});
