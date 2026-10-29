import { describe, expect, it, vi } from "vitest";
import { type DisplayDeps, captureDisplay, displayCaptureSupported } from "../src/display-capture";

/** A fake track of the given surface; `handle` is what `getCaptureHandle`
 * answers (absent: the browser has no capture handles). */
function track(surface: string, handle?: { handle: string } | null) {
  const t = { kind: "video", stop: vi.fn(), getSettings: () => ({ displaySurface: surface }) } as Record<string, unknown>;
  if (handle !== undefined) t.getCaptureHandle = () => handle;
  return t;
}

/** A page of 1000 x 500 CSS px whose display capture answers `tracks`, and
 * whose capture handle config is recorded in `configs`. */
function page(tracks: Record<string, unknown>[] | DOMException, extra: Record<string, unknown> = {}) {
  const asked: unknown[] = [];
  const configs: { handle: string; permittedOrigins: string[] }[] = [];
  const stream = { getTracks: () => tracks, getVideoTracks: () => (tracks as Record<string, unknown>[]).filter(t => t.kind === "video") };
  const win = {
    innerWidth: 1000, innerHeight: 500,
    location: { origin: "https://app.example", href: "https://app.example/" },
    document,
    navigator: {
      mediaDevices: {
        getDisplayMedia: async (o: unknown) => { asked.push(o); if (!Array.isArray(tracks)) throw tracks; return stream; },
        setCaptureHandleConfig: (c: { handle: string; permittedOrigins: string[] }) => { configs.push(c); },
      },
    },
    matchMedia: () => ({ matches: false }),
    getComputedStyle: (e: Element) => window.getComputedStyle(e),
    ...extra,
  } as unknown as Window;
  (win as { top: unknown }).top = win;
  return { win, asked, configs };
}

/** A grabbed frame of 2000 x 1000 device px, and a canvas recording what is drawn on it. */
function drawing() {
  const draws: unknown[][] = [];
  const fills: unknown[][] = [];
  const sizes: number[][] = [];
  const deps: DisplayDeps = {
    grab: async () => ({ source: "FRAME" as unknown as CanvasImageSource, width: 2000, height: 1000 }),
    canvas: (w, h) => {
      sizes.push([w, h]);
      const ctx = { fillStyle: "", drawImage: (...a: unknown[]) => { draws.push(a); }, fillRect: (...a: unknown[]) => { fills.push([ctx.fillStyle, ...a]); } };
      return { ctx: ctx as unknown as CanvasRenderingContext2D, png: async () => new Uint8Array([137, 80, 78, 71]).buffer };
    },
    masks: () => [],
  };
  return { deps, draws, fills, sizes };
}

describe("display capture", () => {
  it("asks for this tab, and crops the frame to the rect", async () => {
    const v = track("browser", null);
    const { win, asked } = page([v]);
    const d = drawing();
    const res = await captureDisplay(win, { x: 100, y: 50, w: 200, h: 100 }, d.deps);
    expect(res.ok).toBe(true);
    expect(asked[0]).toMatchObject({ preferCurrentTab: true, selfBrowserSurface: "include", video: { displaySurface: "browser" }, audio: false });
    // The frame is twice the viewport: the rect is taken at that scale.
    expect(d.sizes).toEqual([[400, 200]]);
    expect(d.draws).toEqual([["FRAME", 200, 100, 400, 200, 0, 0, 400, 200]]);
  });

  it("takes the whole frame without a rect, at most MAX_SIDE on the long side", async () => {
    const { win } = page([track("browser")]);
    const d = drawing();
    await captureDisplay(win, undefined, d.deps);
    expect(d.sizes).toEqual([[1600, 800]]);
    expect(d.draws).toEqual([["FRAME", 0, 0, 2000, 1000, 0, 0, 1600, 800]]);
  });

  it("covers the page's redacted boxes in the frame", async () => {
    const { win } = page([track("browser")]);
    const d = drawing();
    d.deps.masks = () => [{ x: 150, y: 60, w: 20, h: 10 }];
    await captureDisplay(win, { x: 100, y: 50, w: 200, h: 100 }, d.deps);
    expect(d.fills).toEqual([["#000", 100, 20, 40, 20]]);
  });

  it("stops every track", async () => {
    const v = track("browser");
    const a = { kind: "audio", stop: vi.fn() };
    const { win } = page([v, a]);
    await captureDisplay(win, undefined, drawing().deps);
    expect(v.stop).toHaveBeenCalled();
    expect(a.stop).toHaveBeenCalled();
    const w = track("window");
    const other = page([w]);
    await captureDisplay(other.win, undefined, drawing().deps);
    expect(w.stop).toHaveBeenCalled();
  });

  it("display_capture_denied_falls_back_to_dom", async () => {
    const { win } = page(new DOMException("Permission denied", "NotAllowedError"));
    const res = await captureDisplay(win, undefined, drawing().deps);
    expect(res).toMatchObject({ ok: false, reason: "denied" });
  });

  it("display_capture_of_another_tab_is_refused", async () => {
    const { win } = page([track("window")]);
    expect(await captureDisplay(win, undefined, drawing().deps)).toMatchObject({ ok: false, reason: "other_surface" });
    // A tab whose capture handle is not this page's is another tab.
    const tab = page([track("browser", { handle: "someone-else" })]);
    expect(await captureDisplay(tab.win, undefined, drawing().deps)).toMatchObject({ ok: false, reason: "other_surface" });
  });

  it("knows this tab by its capture handle, and says when it cannot tell", async () => {
    let handle = "";
    const t = track("browser");
    t.getCaptureHandle = () => ({ handle });
    const { win, configs } = page([t], {});
    (win.navigator.mediaDevices as { setCaptureHandleConfig?: unknown }).setCaptureHandleConfig = ((c: { handle: string; permittedOrigins: string[] }) => { configs.push(c); handle = c.handle; }) as never;
    const res = await captureDisplay(win, undefined, drawing().deps);
    expect(configs[0].permittedOrigins).toEqual(["https://app.example"]);
    expect(configs[0].handle).not.toBe("");
    expect(res).toMatchObject({ ok: true, thisTab: true });
    const plain = page([track("browser")]);
    expect(await captureDisplay(plain.win, undefined, drawing().deps)).toMatchObject({ ok: true, thisTab: "unknown" });
  });

  it("keeps the focus on this tab where a CaptureController allows", async () => {
    const behaviours: string[] = [];
    class CaptureController { setFocusBehavior(b: string) { behaviours.push(b); } }
    const { win, asked } = page([track("browser")], { CaptureController });
    await captureDisplay(win, undefined, drawing().deps);
    expect(behaviours).toEqual(["no-focus-change"]);
    expect((asked[0] as { controller?: unknown }).controller).toBeInstanceOf(CaptureController);
  });

  it("unsupported without getDisplayMedia", async () => {
    const { win } = page([track("browser")]);
    (win.navigator as { mediaDevices: unknown }).mediaDevices = {};
    expect(displayCaptureSupported(win)).toBe(false);
    expect(await captureDisplay(win, undefined, drawing().deps)).toMatchObject({ ok: false, reason: "unsupported" });
  });

  it("is not offered on coarse-pointer phones, nor in a frame", () => {
    const { win } = page([]);
    expect(displayCaptureSupported(win)).toBe(true);
    const phone = page([], { matchMedia: (q: string) => ({ matches: q.includes("coarse") }) });
    expect(displayCaptureSupported(phone.win)).toBe(false);
    const framed = page([]);
    (framed.win as { top: unknown }).top = {};
    expect(displayCaptureSupported(framed.win)).toBe(false);
  });

  it("fails with a reason when no frame comes", async () => {
    const v = track("browser");
    const { win } = page([v]);
    const d = drawing();
    d.deps.grab = async () => { throw new Error("no frame"); };
    const res = await captureDisplay(win, undefined, d.deps);
    expect(res).toMatchObject({ ok: false, reason: "failed" });
    expect(v.stop).toHaveBeenCalled();
  });
});
