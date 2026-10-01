import type { Thread, Viewer } from "../threads";
import type { Loaded } from "./artifact-controller";

/** The daemon's first-load data for `/a/…` (spec §8 Time to usable). */
export type Boot = { v: 1; artifact: Loaded; threads: Thread[]; viewer: Viewer | null; frame: { mode: "subdomain" | "sandbox"; src: string } | null };

/** The bootstrap block in `doc`, or null when there is none, it does not
 * parse, or it is not version 1. */
export function readBoot(doc: Document = document): Boot | null {
  const text = doc.getElementById("clax-boot")?.textContent;
  if (!text) return null;
  try {
    const b = JSON.parse(text) as Boot;
    return b && b.v === 1 && b.artifact?.artifact && Array.isArray(b.artifact.versions) && Array.isArray(b.threads) ? b : null;
  } catch {
    return null;
  }
}

/** What `artifact.html`'s inline listener kept before the shell mounted,
 * oldest first: every `message` to the shell and every `load` of an
 * `<iframe>`. The listener stops. */
export function takeEarly(win: Window = window): Event[] {
  const early = (win as unknown as { __claxEarly?: { take(): Event[] } }).__claxEarly;
  return early ? early.take() : [];
}

export const FRAME_COOKIE = "clax_frame";

/** Tells the daemon which frame the next load of this browser can get in its
 * HTML. The cookie holds the mode only; the shell still decides the mode on
 * every load and replaces a served frame that disagrees. */
export function rememberFrameMode(origin: string | null, doc: Document = document): void {
  try {
    doc.cookie = `${FRAME_COOKIE}=${origin ? "subdomain" : "sandbox"}; Path=/; Max-Age=2592000; SameSite=Lax`;
  } catch { /* cookies unavailable */ }
}
