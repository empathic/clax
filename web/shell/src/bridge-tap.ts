// The dev server's bridge tap: every message between the shell and a
// content frame's bridge (web/contract's clax:* vocabulary), both ways, with
// a sequence number and the frame's origin. Only under `pnpm dev`
// (import.meta.env.DEV): a build replaces the flag with false and drops the
// tap whole, which bundle-size.mjs checks.
//
// In the page's console: `claxBridgeTap.table()` shows what was said so far
// (the last 500), `claxBridgeTap.live = true` logs each one as it passes,
// `claxBridgeTap.clear()` starts again.

/** One message the tap saw. */
export interface Tapped {
  seq: number;
  at: number;
  dir: "shell→frame" | "frame→shell";
  frame: string;
  type: string;
  message: unknown;
}

const KEEP = 500;

interface Tap {
  log: Tapped[];
  live: boolean;
  table(): void;
  clear(): void;
}

let seq = 0;
let tap: Tap | null = null;

function theTap(): Tap {
  if (tap) return tap;
  const t: Tap = {
    log: [],
    live: false,
    table: () => console.table(t.log.map(r => ({ seq: r.seq, at: r.at, dir: r.dir, frame: r.frame, type: r.type }))),
    clear: () => { t.log.length = 0; },
  };
  (globalThis as { claxBridgeTap?: Tap }).claxBridgeTap = t;
  return (tap = t);
}

/** Records `message` crossing between the shell and the frame at `frame`. */
export function tapBridge(dir: Tapped["dir"], frame: string, message: unknown): void {
  if (!import.meta.env.DEV) return;
  const t = theTap();
  const type = message && typeof message === "object" && typeof (message as { type?: unknown }).type === "string" ? (message as { type: string }).type : typeof message;
  const row: Tapped = { seq: ++seq, at: Math.round(performance.now()), dir, frame, type, message };
  t.log.push(row);
  if (t.log.length > KEEP) t.log.splice(0, t.log.length - KEEP);
  if (t.live) console.debug("[clax bridge]", row.seq, dir, frame, type, message);
}
