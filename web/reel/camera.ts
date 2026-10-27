// The reel's cameras (spec 2026-10-27-sizzle-reel-design §7): what turns
// the camera's page into a clip. The fast camera lets Playwright record the
// page's video (`recordVideo` on the camera's context, record.ts) while real
// time passes; it is for iterating on a vignette, seconds per run, never
// the same twice, and at the viewport's CSS size (Playwright scales a video
// down to fit, never up).
import type { Camera } from "./director.ts";

/** The reel's frame rate: beats.json counts frames at it. */
export const FPS = 30;

/** The wall clock every camera page starts at, so relative times and dates
 * on screen read the same each run (spec §7.1). */
export const REEL_EPOCH = new Date("2026-11-03T09:41:00Z");

/** The fast camera for a video that began recording at `startedAt` (the
 * camera page's creation, `Date.now()`): its frame is the real time since. */
export function fastCamera(startedAt: number, fps = FPS): Camera & { elapsedMs(): number } {
  const frameMs = 1000 / fps;
  const elapsedMs = () => Date.now() - startedAt;
  return {
    frameMs,
    beats: [],
    elapsedMs,
    frame: () => Math.round(elapsedMs() / frameMs),
    // The one wait on real time in the recorder: the length of the shot,
    // while the video records it, not a stand-in for a condition.
    roll: ms => new Promise(resolve => setTimeout(resolve, ms)),
    // Real time passes regardless: there is nothing to pause.
    still: async () => {},
  };
}
