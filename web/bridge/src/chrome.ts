// The page's edge colours for the app shell (`clax:chrome`): the app paints
// the system's status and navigation bars in them, so the bars read as the
// page's own. Loaded with the caps part on the app shell's welcome, so a page
// outside the app loads none of it.

import { type Timers, windowTimers } from "./long-press";

/** How often, at most, the edges are sampled again while the page changes. */
export const CHROME_EVERY_MS = 150;

/** `#rrggbb` for a computed colour that is opaque: `rgb()`/`rgba()`, or
 * `color(srgb …)` (what `color-mix(in srgb, …)` computes to); null for
 * anything else, or an alpha below 1. */
function opaque(c: string): string | null {
  const srgb = c.startsWith("color(srgb ");
  const n = c.match(/[\d.]+/g)?.map(Number);
  if (!n || n.length < 3 || n[3] < 1 || !(srgb || c.startsWith("rgb"))) return null;
  return "#" + n.slice(0, 3).map(v => Math.round(srgb ? v * 255 : v).toString(16).padStart(2, "0")).join("");
}

/** The first opaque background walking up from `el` to the root, else
 * white. */
function backdrop(win: Window, el: Element | null): string {
  for (let e: Element | null = el ?? win.document.body; e; e = e.parentElement) {
    const c = opaque(win.getComputedStyle(e).backgroundColor);
    if (c) return c;
  }
  return "#ffffff";
}

type At = (x: number, y: number) => Element | null;

/** The backgrounds of the elements at the middle of the viewport's top and
 * bottom edges. */
export function sampleChrome(win: Window, at: At = (x, y) => win.document?.elementFromPoint?.(x, y) ?? null): { top: string; bottom: string } {
  const x = win.innerWidth / 2;
  return { top: backdrop(win, at(x, 0)), bottom: backdrop(win, at(x, Math.max(0, win.innerHeight - 1))) };
}

/** Posts `clax:chrome` with the page's edge colours: soon after it starts,
 * then after the page changes, scrolls or resizes or the light or dark
 * appearance changes, at most once every
 * `CHROME_EVERY_MS`, and only when a colour changed. It runs for the
 * document's life. */
export function reportChrome(win: Window, post: (m: { type: "clax:chrome"; top: string; bottom: string }) => void, timers: Timers = windowTimers(win), at?: At): void {
  let timer: number | null = null;
  let last = "";
  const soon = () => {
    timer ??= timers.set(() => {
      timer = null;
      const c = sampleChrome(win, at);
      if (c.top + c.bottom !== last) { last = c.top + c.bottom; post({ type: "clax:chrome", ...c }); }
    }, CHROME_EVERY_MS);
  };
  new MutationObserver(soon).observe(win.document, { subtree: true, childList: true, attributes: true, attributeFilter: ["class", "style"] });
  win.addEventListener("scroll", soon, { passive: true });
  win.addEventListener("resize", soon);
  // A page styled for the light and the dark appearance changes no DOM when
  // the appearance flips.
  win.matchMedia?.("(prefers-color-scheme: dark)").addEventListener?.("change", soon);
  soon();
}
