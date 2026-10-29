// The browser's own screenshot of this tab (spec 2026-10-29-feedback-loop
// §4.1 "Exact screenshot"): `getDisplayMedia` asked for the current tab,
// one frame taken, every track stopped at once. It draws what a DOM render
// cannot (cross-origin frames, canvases, video) but needs the reporter's
// permission, and is not offered on phones nor in a frame (whose viewport is
// not the tab's). The captured surface is checked: anything but a browser
// tab is refused, and so is a tab that carries another page's capture handle;
// where the browser has no capture handles, or the tab shows none, the result
// says it could not tell (`thisTab: "unknown"`). The page's `data-clax-redact` boxes are covered in
// the frame, as a DOM render draws them.

import { REDACT_ATTR } from "./anchor";
import { MAX_CLIP_BYTES, MAX_SIDE, type Rect, blockColour, clipBackground } from "./clip";

/** What a display capture gave: a PNG, and whether it is known to be this
 * tab; or why there is none. */
export type DisplayResult =
  | { ok: true; png: ArrayBuffer; thisTab: boolean | "unknown" }
  | { ok: false; reason: "unsupported" | "denied" | "other_surface" | "failed"; message: string };

/** One captured frame, `width` x `height` device px. */
export interface Frame {
  source: CanvasImageSource;
  width: number;
  height: number;
  close?(): void;
}

/** A canvas to draw the picture on, and its PNG. */
export interface PngCanvas {
  ctx: CanvasRenderingContext2D;
  png(): Promise<ArrayBuffer>;
}

/** What `captureDisplay` uses, each replaceable: the browser's
 * `getDisplayMedia`, the grab of one frame from the track, the canvas the
 * picture is drawn on, and the boxes covered as redacted (viewport CSS px,
 * measured once the frame is taken). */
export interface DisplayDeps {
  getDisplayMedia?: (opts: DisplayMediaStreamOptions) => Promise<MediaStream>;
  grab?: (track: MediaStreamTrack, stream: MediaStream, win: Window) => Promise<Frame>;
  canvas?: (w: number, h: number, win: Window) => PngCanvas;
  masks?: (win: Window) => Rect[];
}

/** The capture options this tab is asked for with (`getDisplayMedia`): the
 * current tab offered first, a tab only, no audio, no switching. */
const DISPLAY_OPTIONS = {
  video: { displaySurface: "browser" },
  audio: false,
  preferCurrentTab: true,
  selfBrowserSurface: "include",
  surfaceSwitching: "exclude",
  monitorTypeSurfaces: "exclude",
} as const;

type CaptureHandleConfig = { handle: string; permittedOrigins: string[] };
type Media = MediaDevices & { setCaptureHandleConfig?: (c: CaptureHandleConfig) => void };
type HandleTrack = MediaStreamTrack & { getCaptureHandle?: () => { handle?: string } | null };

const MESSAGES: Record<Exclude<DisplayResult, { ok: true }>["reason"], string> = {
  unsupported: "This browser cannot capture the tab.",
  denied: "Screen sharing was not allowed.",
  other_surface: "The capture was not this tab.",
  failed: "The tab could not be captured.",
};

const fail = (reason: Exclude<DisplayResult, { ok: true }>["reason"], detail?: string): DisplayResult =>
  ({ ok: false, reason, message: detail ? `${MESSAGES[reason]} (${detail})` : MESSAGES[reason] });

const mediaOf = (win: Window): Media | undefined => win.navigator?.mediaDevices as Media | undefined;

/** Whether this page can offer an exact screenshot: the browser has
 * `getDisplayMedia`, the page is the tab's top document (a frame's viewport
 * is not the tab's), and the device is not a coarse-pointer phone. */
export function displayCaptureSupported(win: Window): boolean {
  if (typeof mediaOf(win)?.getDisplayMedia !== "function") return false;
  try {
    if (win.top !== win) return false;
  } catch {
    return false;
  }
  return !win.matchMedia?.("(pointer: coarse)").matches;
}

/** A random capture handle naming this page to its own capture. */
function nonce(win: Window): string {
  const c = (win as Window & { crypto?: Crypto }).crypto ?? globalThis.crypto;
  if (typeof c?.randomUUID === "function") return c.randomUUID();
  const b = new Uint8Array(16);
  c.getRandomValues(b);
  return Array.from(b, x => x.toString(16).padStart(2, "0")).join("");
}

/** The boxes of the page's `data-clax-redact` elements in view, through open shadow roots. */
export function redactedBoxes(win: Window): Rect[] {
  const out: Rect[] = [];
  const visit = (root: Document | ShadowRoot) => {
    for (const el of Array.from(root.querySelectorAll("*"))) {
      if (el.shadowRoot) visit(el.shadowRoot);
      if (!el.hasAttribute(REDACT_ATTR)) continue;
      const b = el.getBoundingClientRect();
      if (b.width > 0 && b.height > 0 && b.right > 0 && b.bottom > 0 && b.left < win.innerWidth && b.top < win.innerHeight) out.push({ x: b.left, y: b.top, w: b.width, h: b.height });
    }
  };
  visit(win.document);
  return out;
}

/** One frame of `track`: `ImageCapture.grabFrame` where the browser has it,
 * else the first frame a muted `<video>` of the stream presents. */
async function grabFrame(track: MediaStreamTrack, stream: MediaStream, win: Window): Promise<Frame> {
  const IC = (win as Window & { ImageCapture?: new (t: MediaStreamTrack) => { grabFrame(): Promise<ImageBitmap> } }).ImageCapture;
  if (IC) {
    try {
      const bmp = await new IC(track).grabFrame();
      return { source: bmp, width: bmp.width, height: bmp.height, close: () => bmp.close() };
    } catch {
      // Some engines refuse a display track here: the video path takes it.
    }
  }
  const video = win.document.createElement("video");
  video.muted = true;
  video.playsInline = true;
  video.srcObject = stream;
  await video.play();
  await new Promise<void>((resolve, reject) => {
    const v = video as HTMLVideoElement & { requestVideoFrameCallback?: (cb: () => void) => number };
    if (typeof v.requestVideoFrameCallback === "function") { v.requestVideoFrameCallback(() => resolve()); return; }
    if (video.readyState >= 2) { resolve(); return; }
    video.addEventListener("loadeddata", () => resolve(), { once: true });
    video.addEventListener("error", () => reject(new Error("the captured video did not play")), { once: true });
  });
  return { source: video, width: video.videoWidth, height: video.videoHeight, close: () => { video.pause(); video.srcObject = null; } };
}

/** A canvas of the page's document, and its PNG. */
function pageCanvas(w: number, h: number, win: Window): PngCanvas {
  const c = win.document.createElement("canvas");
  c.width = w;
  c.height = h;
  const ctx = c.getContext("2d");
  if (!ctx) throw new Error("no canvas to draw the capture on");
  return {
    ctx,
    png: () => new Promise((resolve, reject) => c.toBlob(b => (b ? b.arrayBuffer().then(resolve, reject) : reject(new Error("the capture could not be encoded"))), "image/png")),
  };
}

/** The picture of `crop` (viewport CSS px; the whole frame when absent)
 * from `frame`, a capture of a viewport `vw` x `vh`, with `masks` covered
 * in `colour`: at most `MAX_SIDE` px on the long side and `MAX_CLIP_BYTES`,
 * halving its size until it fits. */
async function picture(frame: Frame, vw: number, vh: number, crop: Rect | undefined, masks: Rect[], colour: string, canvas: (w: number, h: number) => PngCanvas): Promise<ArrayBuffer> {
  const sx = frame.width / vw;
  const sy = frame.height / vh;
  const c = crop ?? { x: 0, y: 0, w: vw, h: vh };
  const src = {
    x: Math.max(0, Math.round(c.x * sx)),
    y: Math.max(0, Math.round(c.y * sy)),
  };
  const sw = Math.max(1, Math.min(frame.width - src.x, Math.round(c.w * sx)));
  const sh = Math.max(1, Math.min(frame.height - src.y, Math.round(c.h * sy)));
  let fit = Math.min(1, MAX_SIDE / Math.max(sw, sh));
  for (let tries = 0; ; tries++) {
    const w = Math.max(1, Math.round(sw * fit));
    const h = Math.max(1, Math.round(sh * fit));
    const { ctx, png } = canvas(w, h);
    ctx.drawImage(frame.source, src.x, src.y, sw, sh, 0, 0, w, h);
    // Viewport px to picture px.
    const kx = w / c.w;
    const ky = h / c.h;
    ctx.fillStyle = colour;
    for (const m of masks) ctx.fillRect(Math.round((m.x - c.x) * kx), Math.round((m.y - c.y) * ky), Math.round(m.w * kx), Math.round(m.h * ky));
    const buf = await png();
    if (buf.byteLength <= MAX_CLIP_BYTES) return buf;
    if (tries >= 3) throw new Error("the capture is too large to keep, even scaled down");
    fit /= 2;
  }
}

/** The browser's screenshot of this tab, after the reporter allows it,
 * cropped to `crop` (viewport CSS px) when given. The caller steps its own
 * interface aside first. Never rejects: a refusal, an unsupported browser,
 * another surface or a failure answers `ok: false` with its reason. */
export async function captureDisplay(win: Window, crop?: Rect, deps: DisplayDeps = {}): Promise<DisplayResult> {
  const media = mediaOf(win);
  const ask = deps.getDisplayMedia ?? (typeof media?.getDisplayMedia === "function" ? (o: DisplayMediaStreamOptions) => media.getDisplayMedia(o) : null);
  if (!ask) return fail("unsupported");
  // This page names itself to its own capture; a frame (or a browser
  // without capture handles) cannot, and the check then says so.
  const handle = nonce(win);
  try {
    media?.setCaptureHandleConfig?.({ handle, permittedOrigins: [win.location.origin] });
  } catch {
    // Not the top document, or no capture handles here.
  }
  const opts: Record<string, unknown> = { ...DISPLAY_OPTIONS };
  const CC = (win as Window & { CaptureController?: new () => { setFocusBehavior?(b: string): void } }).CaptureController;
  if (CC) {
    try {
      const controller = new CC();
      controller.setFocusBehavior?.("no-focus-change");
      opts.controller = controller;
    } catch {
      // Focus stays where the browser puts it.
    }
  }
  let stream: MediaStream;
  try {
    stream = await ask(opts as DisplayMediaStreamOptions);
  } catch (e) {
    const name = (e as { name?: string })?.name;
    if (name === "NotAllowedError" || name === "AbortError") return fail("denied");
    if (name === "NotSupportedError" || name === "TypeError") return fail("unsupported", (e as Error).message);
    return fail("failed", e instanceof Error ? e.message : String(e));
  }
  try {
    const track = stream.getVideoTracks()[0] as HandleTrack | undefined;
    if (!track) return fail("failed", "no video in the capture");
    if (track.getSettings().displaySurface !== "browser") return fail("other_surface");
    let thisTab: boolean | "unknown" = "unknown";
    if (typeof track.getCaptureHandle === "function") {
      const got = track.getCaptureHandle();
      if (got?.handle === handle) thisTab = true;
      // Another page's handle: another tab. No handle leaves it unknown.
      else if (got?.handle) return fail("other_surface");
    }
    let frame: Frame;
    try {
      frame = await (deps.grab ?? grabFrame)(track, stream, win);
    } catch (e) {
      return fail("failed", e instanceof Error ? e.message : String(e));
    }
    try {
      if (!frame.width || !frame.height) return fail("failed", "the captured frame is empty");
      const masks = (deps.masks ?? redactedBoxes)(win);
      const colour = blockColour(clipBackground(win.document.documentElement, win));
      const make = deps.canvas ?? pageCanvas;
      const png = await picture(frame, win.innerWidth, win.innerHeight, crop, masks, colour, (w, h) => make(w, h, win));
      return { ok: true, png, thisTab };
    } catch (e) {
      return fail("failed", e instanceof Error ? e.message : String(e));
    } finally {
      frame.close?.();
    }
  } finally {
    for (const t of stream.getTracks()) t.stop();
  }
}
