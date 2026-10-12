// The inspection's screenshot (`inspect.ts`): the page's viewport as it shows
// now, rendered with modern-screenshot as clips are (`clip.ts`: images from
// other origins left out, a never-attached helper frame in an opaque-origin
// sandbox), at the scale the inspection asks for. Loaded with the clip part.
import { createContext, destroyContext, domToPng } from "modern-screenshot";
import { CLIP_TIMEOUT_MS, clipBackground, crossOriginImage, dataUrlToBuffer } from "./clip";

const TRANSPARENT = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

/** A PNG of what `win`'s viewport shows (its body, cropped to the viewport)
 * at `scale` CSS pixels to one image pixel; rejects after `timeoutMs`. */
export async function renderViewport(win: Window, scale: number, timeoutMs = CLIP_TIMEOUT_MS): Promise<ArrayBuffer> {
  const doc = win.document;
  const root = doc.body ?? doc.documentElement;
  const box = root.getBoundingClientRect();
  const w = Math.max(1, Math.round(doc.documentElement.clientWidth || win.innerWidth));
  const h = Math.max(1, Math.round(doc.documentElement.clientHeight || win.innerHeight));
  const origin = new URL(win.location.href).origin;
  const ctx = await createContext(root, {
    scale,
    width: w,
    height: h,
    filter: n => !crossOriginImage(n, origin),
    fetch: { placeholderImage: TRANSPARENT },
    backgroundColor: clipBackground(root, win),
    // The root at its own size, moved so the viewport sits in the picture.
    style: { margin: "0", width: `${box.width}px`, height: `${box.height}px`, transform: `translate(${box.left}px, ${box.top}px)`, transformOrigin: "0 0" },
    timeout: timeoutMs,
  });
  if (win.origin === "null") ctx.sandbox = doc.createElement("iframe");
  let timer: ReturnType<typeof setTimeout> | undefined;
  const late = new Promise<never>((_, reject) => { timer = setTimeout(() => reject(new Error(`the screenshot took longer than ${timeoutMs} ms`)), timeoutMs); });
  try {
    return await Promise.race([domToPng(ctx).then(dataUrlToBuffer), late]);
  } finally {
    clearTimeout(timer);
    destroyContext(ctx);
  }
}
