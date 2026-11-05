// The install's ribbon: a band across the page's top-left corner, in the
// accent, saying the install's wording in capitals. Only the band takes the
// pointer; the corner square around it lets every click through.
//
// It has a host of its own at the start of <body>, not in the overlay's (the
// last thing in the document), so it comes first in the tab order and in a
// screen reader's reading order. The host is the overlay's tag, so comment
// mode, a capture and the pickers all pass over it as they do the overlay.
//
// A reporter who can file gets a real button with a focus ring along the band,
// its name "<wording> — send feedback". Anyone else gets a label, read as the
// wording. The wording is set as text, never as markup.
import { OVERLAY_TAG } from "../../bridge/src/anchor";
import { addSheet } from "../../bridge/src/sheet";

interface Form { size: number; inner: number; fontSize: number }

/** The band's two sizes. `full` is 24px thick, `compact` 20px for a narrow
 * window; each is one line of lettering, never under 10px. */
export const RIBBON_FORMS = {
  full: { size: 108, inner: 74, fontSize: 11 },
  compact: { size: 92, inner: 64, fontSize: 10 },
} as const satisfies Record<string, Form>;

const SVG = "http://www.w3.org/2000/svg";
const points = (ps: readonly (readonly [number, number])[]) => ps.map(p => p.join(",")).join(" ");
const round = (n: number) => Math.round(n * 1000) / 1000;

function art(doc: Document, name: string, f: Form, wording: string): SVGElement {
  const { size: s, inner: i } = f;
  const at = (i + s) / 4;
  const el = <K extends keyof SVGElementTagNameMap>(tag: K, attrs: Record<string, string | number>) => {
    const e = doc.createElementNS(SVG, tag);
    for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, String(v));
    return e;
  };
  // The ring is 2px, 2px in from the band's every edge; the lines are its outermost pixels.
  const ring = [[round(i + 3 * Math.SQRT2 - 3), 3], [round(s - 3 * Math.SQRT2 - 3), 3], [3, round(s - 3 * Math.SQRT2 - 3)], [3, round(i + 3 * Math.SQRT2 - 3)]] as const;
  const half = Math.SQRT1_2;
  const svg = el("svg", { class: `art ${name}`, viewBox: `0 0 ${s} ${s}`, width: s, height: s, "aria-hidden": "true", focusable: "false" });
  const text = el("text", { class: "text", x: at, y: at, dx: ".06em", dy: ".36em", "text-anchor": "middle", "font-size": f.fontSize, transform: `rotate(-45 ${at} ${at})` });
  text.textContent = wording.toUpperCase();
  svg.append(
    el("polygon", { class: "band", points: points([[i, 0], [s, 0], [0, s], [0, i]]) }),
    el("path", { class: "edge", d: `M${round(i + half)},0L0,${round(i + half)}M${round(s - half)},0L0,${round(s - half)}` }),
    text,
    el("polygon", { class: "ring", points: points(ring) }),
  );
  return svg;
}

/** Black or white, whichever reads better on the colour `hex` (`#rrggbb`):
 * either way at least 4.5:1, whatever accent an install names. */
export function inkOn(hex: string): "#000000" | "#ffffff" {
  const ch = (at: number) => {
    const v = parseInt(hex.slice(at, at + 2), 16) / 255;
    return v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
  };
  const l = 0.2126 * ch(1) + 0.7152 * ch(3) + 0.0722 * ch(5);
  return (l + 0.05) / 0.05 >= 1.05 / (l + 0.05) ? "#000000" : "#ffffff";
}

/** The band's default accent, and the overlay's: tokens.css's `--you`. */
const DEFAULT_BAND = "#e0532f";

const CSS = `
:host { all: initial; }
.ribbon { position: absolute; top: 0; left: 0; display: block; margin: 0; padding: 0; border: 0; background: none; line-height: 0; pointer-events: none; -webkit-tap-highlight-color: transparent; }
.ribbon:focus-visible { outline: none; }
.art { display: block; overflow: hidden; }
.compact { display: none; }
.band { fill: var(--band); }
button.ribbon .band { pointer-events: auto; cursor: pointer; }
.edge { fill: none; stroke: var(--ink); stroke-opacity: .35; stroke-width: 1; }
.text { fill: var(--ink); font-family: ui-monospace, SFMono-Regular, Consolas, monospace; font-weight: 600; letter-spacing: .12em; text-rendering: geometricPrecision; }
.ring { fill: none; stroke: var(--ink); stroke-width: 2; visibility: hidden; }
.ribbon:focus-visible .ring { visibility: visible; }
@media (max-width: 1100px) { .full { display: none; } .compact { display: block; } }
@media print { .ribbon { display: none; } }
@media (forced-colors: active) { .band { fill: ButtonFace; stroke: ButtonText; } .text { fill: ButtonText; } .edge { stroke: ButtonText; stroke-opacity: 1; } .ring { stroke: Highlight; } }
`;

export interface Ribbon {
  /** Removes the ribbon. */
  destroy(): void;
}

/** Draws the ribbon on `doc`. With `onPress` it is a button (passed itself, to
 * give focus back to); without, a label. */
export function mountRibbon(doc: Document, r: { wording: string; brandColor: string | null; onPress: ((button: HTMLElement) => void) | null; shortcut: string | null }): Ribbon {
  const host = doc.createElement(OVERLAY_TAG);
  // Through the CSSOM, so a page whose CSP refuses inline style allows it. Above a top bar, under a page's menus and dialogs.
  host.style.cssText = "all:initial!important;position:absolute!important;left:0!important;top:0!important;width:0!important;height:0!important;display:block!important;visibility:visible!important;opacity:1!important;z-index:21!important;";
  const root = host.attachShadow({ mode: "open" });
  const band = r.brandColor ?? DEFAULT_BAND;
  addSheet(root, `${CSS}\n:host{--band:${band};--ink:${inkOn(band)};}`);
  let el: HTMLElement;
  if (r.onPress) {
    const press = r.onPress;
    const b = doc.createElement("button");
    b.type = "button";
    b.setAttribute("aria-label", `${r.wording} — send feedback`);
    if (r.shortcut) b.setAttribute("aria-keyshortcuts", r.shortcut);
    b.addEventListener("click", () => press(b));
    el = b;
  } else {
    el = doc.createElement("span");
    el.setAttribute("role", "img");
    el.setAttribute("aria-label", r.wording);
  }
  el.className = "ribbon";
  el.append(art(doc, "full", RIBBON_FORMS.full, r.wording), art(doc, "compact", RIBBON_FORMS.compact, r.wording));
  root.append(el);
  (doc.body ?? doc.documentElement).prepend(host);
  return { destroy: () => host.remove() };
}
