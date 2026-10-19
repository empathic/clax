// The thread drawer's place and size wider than a phone, as this browser
// remembers it for every artifact (localStorage; every access may throw:
// private windows), and its moves and resizes. The artifact page's head
// script (`clax-drawer`, shell/artifact.html) applies the stored prefs as
// `applyPrefs` does before the first paint, unchecked, and the drawer's box
// (SidebarIsland.svelte, DrawerChrome.svelte) clamps every size to the
// viewer; the drawer's controls (DrawerChrome.svelte, with the sidebar's
// lazy chunk) check the prefs (`normal`) and clamp each drag and key with
// the same limits.
export type DrawerLayout = "right" | "left" | "bottom" | "float";
/** `w`: the width docked left or right; `h`: the height docked at the
 * bottom; `x`, `y`, `fw`, `fh`: the floating window's place in the viewer
 * and size, all four or none (its default place: the viewer's top right,
 * 16 px in, its height less 32 px). */
export type DrawerPrefs = { layout: DrawerLayout; w: number; h: number; x?: number; y?: number; fw?: number; fh?: number };
export const DRAWER_KEY = "clax.drawer";

/** The prefs as stored, unchecked; none when there are none. */
export function readPrefs(): Partial<DrawerPrefs> {
  try { const o: unknown = JSON.parse(localStorage.getItem(DRAWER_KEY) ?? "{}"); if (o && typeof o === "object") return o; } catch { /* the defaults */ }
  return {};
}

/** Lays the drawer out by `p`, as the head script does: the layout as the
 * root's `data-drawer` (none: docked right), and each size that is a number
 * as a custom property on the root, `--drawer-<field>`; the others removed. */
export function applyPrefs(p: Partial<DrawerPrefs>): void {
  const root = document.documentElement;
  root.dataset.drawer = p.layout ?? "";
  for (const k of ["w", "h", "x", "y", "fw", "fh"] as const) root.style.setProperty(`--drawer-${k}`, Number.isFinite(p[k]) ? `${p[k]}px` : "");
}

export const LAYOUTS: readonly DrawerLayout[] = ["left", "bottom", "right", "float"];
export type Rect = { x: number; y: number; w: number; h: number };
export type Size = { w: number; h: number };

export const DEFAULT_WIDTH = 392;
export const DEFAULT_HEIGHT = 280;
/** Docked left or right: the drawer's width, and the least width the page keeps. */
export const MIN_WIDTH = 280;
export const MAX_WIDTH = 720;
export const MIN_STAGE_W = 320;
/** Docked at the bottom: the drawer's least height, and the least height the page keeps. */
export const MIN_HEIGHT = 160;
export const MIN_STAGE_H = 160;
/** The floating window's least size; at most the viewer's. */
export const MIN_FLOAT_W = 280;
export const MIN_FLOAT_H = 200;
/** How far an arrow key moves an edge or the window, and with Shift. */
export const STEP = 16;
export const BIG_STEP = 64;

export const DEFAULTS: DrawerPrefs = { layout: "right", w: DEFAULT_WIDTH, h: DEFAULT_HEIGHT };

const clamp = (v: number, lo: number, hi: number) => Math.round(Math.min(Math.max(v, lo), Math.max(lo, hi)));
const num = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);

/** The stored prefs checked field by field: what is missing or wrong is the default. */
export function normal(o: Partial<Record<keyof DrawerPrefs, unknown>>): DrawerPrefs {
  const p: DrawerPrefs = {
    layout: LAYOUTS.includes(o.layout as DrawerLayout) ? o.layout as DrawerLayout : DEFAULTS.layout,
    w: num(o.w) ? clamp(o.w, MIN_WIDTH, MAX_WIDTH) : DEFAULT_WIDTH,
    h: num(o.h) ? Math.round(Math.max(o.h, MIN_HEIGHT)) : DEFAULT_HEIGHT,
  };
  if (num(o.x) && num(o.y) && num(o.fw) && num(o.fh)) {
    Object.assign(p, { x: Math.round(Math.max(0, o.x)), y: Math.round(Math.max(0, o.y)), fw: Math.round(Math.max(MIN_FLOAT_W, o.fw)), fh: Math.round(Math.max(MIN_FLOAT_H, o.fh)) });
  }
  return p;
}

/** The docked width shown in a viewer `viewW` wide. */
export const dockWidth = (w: number, viewW: number) => clamp(w, MIN_WIDTH, Math.min(MAX_WIDTH, viewW - MIN_STAGE_W));
/** The docked height shown in a viewer `viewH` high. */
export const dockHeight = (h: number, viewH: number) => clamp(h, MIN_HEIGHT, viewH - MIN_STAGE_H);

/** `r` resized to its limits and moved wholly inside the viewer. */
export function clampFloat(r: Rect, view: Size): Rect {
  const w = clamp(r.w, MIN_FLOAT_W, view.w);
  const h = clamp(r.h, MIN_FLOAT_H, view.h);
  return { x: clamp(r.x, 0, view.w - w), y: clamp(r.y, 0, view.h - h), w, h };
}

/** What a drag or a key moves: the docked edge, the floating window's right
 * edge (`fw`) or bottom edge (`fh`), its corner (both), or the window. */
export type Grip = "dock" | "fw" | "fh" | "corner" | "move";

/** The prefs once `grip` moved by (`dx`, `dy`) from `start`, the drawer's
 * box in the viewer's coordinates when the move began, in a viewer of
 * `view`'s size; every size clamped to what the viewer shows. */
export function moved(p: DrawerPrefs, grip: Grip, start: Rect, dx: number, dy: number, view: Size): DrawerPrefs {
  if (grip === "dock") {
    if (p.layout === "bottom") return { ...p, h: dockHeight(start.h - dy, view.h) };
    if (p.layout === "left" || p.layout === "right") return { ...p, w: dockWidth(start.w + (p.layout === "left" ? dx : -dx), view.w) };
    return p;
  }
  if (p.layout !== "float") return p;
  const r = { ...start };
  if (grip === "move") { r.x += dx; r.y += dy; }
  if (grip === "fw" || grip === "corner") r.w = Math.min(start.w + dx, view.w - start.x);
  if (grip === "fh" || grip === "corner") r.h = Math.min(start.h + dy, view.h - start.y);
  const f = clampFloat(r, view);
  return { ...p, x: f.x, y: f.y, fw: f.w, fh: f.h };
}

/** The prefs with `grip`'s size back to its default: the floating window's
 * place and size together. */
export function reset(p: DrawerPrefs, grip: Grip): DrawerPrefs {
  if (grip === "move" || p.layout === "float") return { layout: p.layout, w: p.w, h: p.h };
  return p.layout === "bottom" ? { ...p, h: DEFAULT_HEIGHT } : { ...p, w: DEFAULT_WIDTH };
}

/** The arrow keys' (and Home's and End's) move of `grip`, as a drag's
 * (`dx`, `dy`), or null for another key. Home and End take a docked edge
 * to its least and greatest size. A separator takes the arrows across it:
 * Left and Right a vertical one, Up and Down a horizontal one. */
export function keyMove(key: string, shift: boolean, grip: Grip, layout: DrawerLayout): [number, number] | null {
  const s = shift ? BIG_STEP : STEP;
  const arrows: Record<string, [number, number]> = { ArrowLeft: [-s, 0], ArrowRight: [s, 0], ArrowUp: [0, -s], ArrowDown: [0, s] };
  if (key in arrows) {
    const [dx, dy] = arrows[key];
    const horizontal = grip === "fh" || (grip === "dock" && layout === "bottom");
    const vertical = grip === "fw" || (grip === "dock" && layout !== "bottom");
    if ((horizontal && dx) || (vertical && dy)) return null;
    return [dx, dy];
  }
  if ((key === "Home" || key === "End") && grip === "dock") {
    const far = key === "End" ? 1e6 : -1e6;
    if (layout === "bottom") return [0, -far];
    return [layout === "left" ? far : -far, 0];
  }
  return null;
}

/** A separator's size as it announces it (`aria-valuenow`, min, max), or null for the window's move. */
export function sizeOf(grip: Grip, p: DrawerPrefs, box: Rect, view: Size): { now: number; min: number; max: number } | null {
  if (grip === "dock" && p.layout === "bottom") return { now: Math.round(box.h), min: MIN_HEIGHT, max: Math.max(MIN_HEIGHT, view.h - MIN_STAGE_H) };
  if (grip === "dock") return { now: Math.round(box.w), min: MIN_WIDTH, max: Math.max(MIN_WIDTH, Math.min(MAX_WIDTH, view.w - MIN_STAGE_W)) };
  if (grip === "fw") return { now: Math.round(box.w), min: MIN_FLOAT_W, max: Math.max(MIN_FLOAT_W, Math.round(view.w - box.x)) };
  if (grip === "fh") return { now: Math.round(box.h), min: MIN_FLOAT_H, max: Math.max(MIN_FLOAT_H, Math.round(view.h - box.y)) };
  return null;
}
