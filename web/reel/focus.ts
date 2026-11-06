// A vignette's focus script (web/reel/vignettes/<id>.focus.json): what the
// reel lights, and when, as named targets. The script names each target by a
// Playwright selector; the deterministic recorder writes every target's box,
// per frame where it changes, to the recording's focus-rects.json, beside
// its beats.json, and copies the script there as focus.json. The composer
// (web/reel/compose/camera.py) reads both and renders the shots, so a UI
// change moves the lit regions with the elements, on the next recording.
//
//   {
//     "targets": { "<name>": "<selector>" | "frame:<selector>", ... },
//     "shots": [{ "at": "<time>", "until"?: "<time>",
//                 "targets": ["<name>", ...], "label": "..." }, ...],
//     "clean"?: [{ "at": "<time>", "until": "<time>" }],
//     "overrides"?: { "<name>": [x, y, w, h] }
//   }
//
// A time is a frame, BEAT[+-seconds] (a beat of beats.json) or
// @TARGET[+-seconds]: the first frame, from the shot before's, where that
// target is on screen, so a shot can lead the element it lights.
// A selector is found in the shell's page; with "frame:", in the artifact's
// content frame (the version on screen). Playwright's CSS reaches into open
// shadow roots. "overrides" are pixel boxes (the recording's device pixels)
// that stand in for a target: an escape hatch, not the source of truth.
import type { Frame, Page } from "@playwright/test";

export type FocusScript = {
  targets: Record<string, string>;
  shots: { at: string | number; until?: string | number; targets: string[]; label?: string }[];
  clean?: { at: string | number; until: string | number; label?: string }[];
  overrides?: Record<string, [number, number, number, number]>;
};

/** A box in the recording's device pixels: [x, y, width, height]. */
export type Box = [number, number, number, number];

const TIME = /^@?[a-z][a-z0-9_]*([+-]\d+(\.\d+)?)?$/;
const NAME = /^[a-z][a-z0-9_]*$/;

/** The script in `json`, checked: every shot names targets the script
 * declares (or overrides), and every time is a frame or BEAT[+-seconds]. */
export function parseFocus(json: string, where: string): FocusScript {
  const s = JSON.parse(json) as FocusScript;
  const fail = (m: string): never => {
    throw new Error(`${where}: ${m}`);
  };
  if (!s || typeof s !== "object" || typeof s.targets !== "object" || !Array.isArray(s.shots)) fail("wants targets {} and shots []");
  for (const [name, sel] of Object.entries(s.targets)) {
    if (!NAME.test(name)) fail(`target ${JSON.stringify(name)}: wants a name of a-z, 0-9 and _`);
    if (typeof sel !== "string" || !sel.replace(/^frame:/, "").trim()) fail(`target ${name}: wants a selector`);
  }
  const known = new Set([...Object.keys(s.targets), ...Object.keys(s.overrides ?? {})]);
  const time = (t: unknown, what: string) => {
    if (typeof t === "number" ? !Number.isInteger(t) || t < 0 : typeof t !== "string" || !TIME.test(t)) fail(`${what}: ${JSON.stringify(t)} is not a frame, BEAT[+-seconds] or @TARGET[+-seconds]`);
  };
  const at = (t: unknown, what: string) => {
    time(t, what);
    if (typeof t === "string" && t.startsWith("@") && !(t.slice(1).replace(/[+-].*$/, "") in s.targets)) fail(`${what}: no target ${t.slice(1).replace(/[+-].*$/, "")} to wait for`);
  };
  s.shots.forEach((shot, i) => {
    at(shot.at, `shots[${i}].at`);
    if (shot.until !== undefined) at(shot.until, `shots[${i}].until`);
    if (!Array.isArray(shot.targets) || !shot.targets.length) fail(`shots[${i}]: wants targets`);
    for (const t of shot.targets) if (!known.has(t)) fail(`shots[${i}]: no target ${t}`);
  });
  (s.clean ?? []).forEach((c, i) => {
    at(c.at, `clean[${i}].at`);
    at(c.until, `clean[${i}].until`);
  });
  for (const [name, box] of Object.entries(s.overrides ?? {})) {
    if (!Array.isArray(box) || box.length !== 4 || !box.every(v => Number.isFinite(v))) fail(`overrides.${name}: wants [x, y, w, h]`);
  }
  return s;
}

/** Each target's box on the page now, in device pixels (`scale` per CSS
 * pixel), or null when it is not on screen: its first visible match. A
 * "frame:" target is looked for in the content frames (those under
 * /_clax/v/), the last one first: the version on screen. */
export async function targetBoxes(page: Page, targets: Record<string, string>, scale: number): Promise<Record<string, Box | null>> {
  const out: Record<string, Box | null> = {};
  for (const [name, spec] of Object.entries(targets)) {
    const inFrame = spec.startsWith("frame:");
    const sel = inFrame ? spec.slice("frame:".length) : spec;
    const frames: Frame[] = inFrame ? page.frames().filter(f => f.url().includes("/_clax/v/")).reverse() : [page.mainFrame()];
    out[name] = null;
    for (const f of frames) {
      const box = await firstBox(f, sel);
      if (box) {
        out[name] = [Math.round(box.x * scale), Math.round(box.y * scale), Math.round(box.width * scale), Math.round(box.height * scale)];
        break;
      }
    }
  }
  return out;
}

async function firstBox(f: Frame, sel: string): Promise<{ x: number; y: number; width: number; height: number } | null> {
  try {
    const all = f.locator(sel);
    const n = await all.count();
    for (let i = 0; i < n; i++) {
      const box = await all.nth(i).boundingBox({ timeout: 1000 });
      if (box && box.width > 0 && box.height > 0) return box;
    }
  } catch (e) {
    // A frame the page replaced while it was asked: not on screen this frame.
    if (!f.isDetached() && !/detached|navigat|Target closed|Execution context/i.test(String(e))) throw e;
  }
  return null;
}

/** Collects targets' boxes frame by frame, keeping a change list per target:
 * [[frame, box | null], ...], a new entry only where the box changed. */
export class FocusLog {
  readonly rects: Record<string, [number, Box | null][]> = {};

  record(frame: number, boxes: Record<string, Box | null>): void {
    for (const [name, box] of Object.entries(boxes)) {
      const list = (this.rects[name] ??= []);
      const last = list.length ? list[list.length - 1]![1] : undefined;
      if (last === undefined || JSON.stringify(last) !== JSON.stringify(box)) list.push([frame, box]);
    }
  }

  /** focus-rects.json: the frame rate, the device pixel scale and each target's change list. */
  json(fps: number, scale: number): string {
    return `${JSON.stringify({ fps, scale, targets: this.rects })}\n`;
  }
}
