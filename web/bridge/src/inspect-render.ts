// The inspection's screenshot (`inspect.ts`): the page's viewport as it shows
// now, rendered with modern-screenshot as clips are (`clip.ts`: images from
// other origins left out, a never-attached helper frame in an opaque-origin
// sandbox), at the scale the inspection asks for. Loaded with the clip part.
// What the library would paint unlike the page is painted as the page shows
// it (`faithfulClone`): an empty field's placeholder in its placeholder
// colour, not as typed text, and an image that failed to load as an
// outlined box with its alt text, not as a solid one.
import { createContext, destroyContext, domToPng } from "modern-screenshot";
import { CLIP_TIMEOUT_MS, clipBackground, crossOriginImage, dataUrlToBuffer } from "./clip";

const TRANSPARENT = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

const GREY = "#9aa0a6";
const glyphSize = (w: number, h: number) => [Math.max(1, Math.round(w)), Math.max(1, Math.round(h))];
const esc = (s: string) => s.replace(/[&<>"']/g, c => `&#${c.charCodeAt(0)};`);

/** A broken-image glyph `w` x `h` CSS pixels (rounded, 1 at least), as an
 * SVG data URL: an outline, a small picture-frame icon and `alt`. */
export function brokenImage(w: number, h: number, alt: string): string {
  const [W, H] = glyphSize(w, h);
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${W}" height="${H}"><rect x=".5" y=".5" width="${W - 1}" height="${H - 1}" fill="none" stroke="${GREY}"/>`
    + `<path d="M3 3h10v10H3zM4 12l3-4 2 2 2-3 2 5" fill="none" stroke="${GREY}"/>`
    + (alt ? `<text x="16" y="12" font-family="system-ui,sans-serif" font-size="11" fill="#5f6368">${esc(alt.slice(0, 200))}</text>` : "")
    + "</svg>";
  return `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;
}

/** The clone hook that paints what modern-screenshot would not paint as
 * `root` shows it: each clone of an empty field with a placeholder gets the
 * field's `::placeholder` colour as its own, and each clone of an image that
 * failed to load (complete, with no natural width) shows `brokenImage` at the
 * image's size. Clones are matched to their originals by the attribute
 * (`placeholder`, `src`) and document order. */
export function faithfulClone(root: Element, win: Window): (cloned: Node) => void {
  const queue = <T>(m: Map<string, T[]>, k: string, v: T) => { const l = m.get(k); if (l) l.push(v); else m.set(k, [v]); };
  const fields = new Map<string, string[]>();
  for (const el of root.querySelectorAll<HTMLInputElement | HTMLTextAreaElement>("input[placeholder], textarea[placeholder]")) {
    if (!el.value) queue(fields, el.getAttribute("placeholder")!, win.getComputedStyle(el, "::placeholder").color);
  }
  const broken = new Map<string, { url: string; size: number[]; text: boolean }[]>();
  for (const img of root.querySelectorAll<HTMLImageElement>("img[src]")) {
    if (!img.complete || img.naturalWidth > 0) continue;
    const r = img.getBoundingClientRect();
    queue(broken, img.getAttribute("src")!, { url: brokenImage(r.width, r.height, img.alt), size: glyphSize(r.width, r.height), text: !!img.alt });
  }
  return cloned => {
    if (!(cloned instanceof Element)) return;
    const name = cloned.localName;
    const style = (cloned as HTMLElement).style;
    if ((name === "input" || name === "textarea") && cloned.hasAttribute("placeholder")) {
      const color = fields.get(cloned.getAttribute("placeholder")!)?.shift();
      if (color && !(cloned as HTMLInputElement).value) { style.color = color; style.setProperty("-webkit-text-fill-color", color); }
    } else if (name === "img" && cloned.hasAttribute("src")) {
      const glyph = broken.get(cloned.getAttribute("src")!)?.shift();
      if (!glyph) return;
      cloned.removeAttribute("srcset");
      cloned.setAttribute("src", glyph.url);
      style.width = `${glyph.size[0]}px`;
      style.height = `${glyph.size[1]}px`;
      // A broken image with alt text lays out as that text does (its box's
      // bottom at the text's, not on the baseline as an image's is).
      if (glyph.text) style.verticalAlign = "text-bottom";
    }
  };
}

/** A PNG of what `win`'s viewport shows (its root element, cropped to the
 * viewport) at `scale` CSS pixels to one image pixel; rejects after
 * `timeoutMs`. The root, not the body, is rendered, as a block formatting
 * context: a first child's top margin that collapses through the body then
 * stays above the body in the picture as on the page, and absolutely
 * positioned elements keep the document as their containing block. */
export async function renderViewport(win: Window, scale: number, timeoutMs = CLIP_TIMEOUT_MS): Promise<ArrayBuffer> {
  const doc = win.document;
  const root = doc.documentElement;
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
    onCloneEachNode: faithfulClone(root, win),
    backgroundColor: clipBackground(root, win),
    // The root at its own size, moved so the viewport sits in the picture.
    style: { margin: "0", display: "flow-root", width: `${box.width}px`, height: `${box.height}px`, transform: `translate(${box.left}px, ${box.top}px)`, transformOrigin: "0 0" },
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
