// The parts from source, for unit tests (vitest aliases clax-bridge-parts here).
// `failParts` makes the named parts' loads fail, and `requests` lists every
// load asked for, in order.
import type { CapsPart, ClipPart, CommentPart, Parts, RoomPart } from "./parts/types";

const loads: Promise<unknown>[] = [];
export const requests: string[] = [];
const failing = new Set<string>();

/** Makes every later load of `names` fail (an empty list ends that). */
export function failParts(names: string[]): void {
  failing.clear();
  for (const n of names) failing.add(n);
}

const loader = <T>(name: string, f: () => Promise<unknown>) => (_attempt = 0): Promise<T> => {
  requests.push(name);
  const p = (failing.has(name) ? Promise.reject(new Error(`the ${name} part is blocked`)) : f()) as Promise<T>;
  loads.push(p);
  return p;
};

export function loadParts(_bridgeSrc: string): Parts {
  return {
    comment: loader<CommentPart>("comment", () => import("./parts/comment")),
    clip: loader<ClipPart>("clip", () => import("./parts/clip")),
    caps: loader<CapsPart>("caps", () => import("./parts/caps")),
    room: loader<RoomPart>("room", () => import("./parts/room")),
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
