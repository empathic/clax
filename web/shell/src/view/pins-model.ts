import type { AnchorResult } from "../../../bridge/src/protocol";
import type { Thread } from "../threads";

/** Room a pin keeps from the stage's right edge: its own 22 px plus 16 px for a
 * classic scrollbar in the frame. */
export const PIN_RIGHT_ROOM = 38;

export type PinPlace = { thread: Thread; n: number; left: number; top: number };

/** Where each pin goes over the frame: the open threads on `file` (none when
 * it is null, a document that did not greet) that are not detached, numbered
 * like the sidebar's Open section; only those found with a rectangle not
 * wholly above the frame get a pin, at the rectangle's top right, never past
 * `stage` (its width; 0 while unmeasured) minus the scrollbar's room. */
export function pinPlaces(threads: Thread[], resolved: Record<string, AnchorResult>, file: string | null, stage: number): PinPlace[] {
  const attached = threads.filter(t => t.status === "open" && t.anchor.file === file && !(resolved[t.id] && !resolved[t.id].found));
  const out: PinPlace[] = [];
  attached.forEach((t, i) => {
    const r = resolved[t.id]?.rect;
    if (!r || r.y + r.h <= 0) return;
    let left = r.x + r.w - 12;
    if (stage > 0) left = Math.min(left, stage - PIN_RIGHT_ROOM);
    out.push({ thread: t, n: i + 1, left: Math.max(0, left), top: Math.max(0, r.y - 12) });
  });
  return out;
}
