// The parts from source, for unit tests (vitest aliases clax-bridge-parts here).
import type { CapsPart, ClipPart, CommentPart, Parts } from "./parts/types";

const loads: Promise<unknown>[] = [];
const once = <T>(f: () => Promise<T>) => { let p: Promise<T> | null = null; return () => { if (!p) { p = f(); loads.push(p); } return p; }; };

export function loadParts(_bridgeSrc: string): Parts {
  return {
    comment: once(() => import("./parts/comment") as Promise<CommentPart>),
    clip: once(() => import("./parts/clip") as Promise<ClipPart>),
    caps: once(() => import("./parts/caps") as Promise<CapsPart>),
  };
}

/** Resolves once every part requested so far has loaded and the work queued
 * on it has run, including parts requested by that work. */
export async function settle(): Promise<void> {
  for (let seen = -1; seen !== loads.length;) {
    seen = loads.length;
    await Promise.allSettled(loads);
    await new Promise(r => setTimeout(r, 0));
  }
}
