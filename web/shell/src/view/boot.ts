import type { Attention } from "../api";
import type { Thread, Viewer } from "../threads";
import type { Loaded } from "./artifact-controller";

/** The daemon's first-load data for `/a/…` (spec §8 Time to usable). */
export type Boot = {
  v: 1; artifact: Loaded; threads: Thread[]; viewer: Viewer | null; frame: { mode: "subdomain" | "sandbox"; src: string } | null;
  /** The cookie's viewer's attention on this artifact; absent without a viewer. */
  attention?: Attention | null;
};

/** How this document was reached (`back_forward` for history), from Navigation Timing. */
function navigationType(): string {
  try {
    return (performance.getEntriesByType("navigation")[0] as PerformanceNavigationTiming | undefined)?.type ?? "";
  } catch {
    return "";
  }
}

/** The bootstrap block in `doc`, or null when there is none, it does not
 * parse, or it is not version 1. A page reached through history may come
 * from the browser's cache without asking the daemon, so its viewer can be
 * stale: it and its attention are left out (`viewer: null`, `attention:
 * null`), and the shell looks the viewer up.
 * The threads and the artifact are reloaded on the stream's first `ready`. */
export function readBoot(doc: Document = document, navType: string = navigationType()): Boot | null {
  const text = doc.getElementById("clax-boot")?.textContent;
  if (!text) return null;
  try {
    const b = JSON.parse(text) as Boot;
    if (!(b && b.v === 1 && b.artifact?.artifact && Array.isArray(b.artifact.versions) && Array.isArray(b.threads))) return null;
    return navType === "back_forward" ? { ...b, viewer: null, attention: null } : b;
  } catch {
    return null;
  }
}

/** What `artifact.html`'s inline listener kept before the shell mounted,
 * oldest first: every `message` from the served frame's window (another
 * window's messages are never kept, so they cannot crowd out the frame's
 * hello) and every `load` of an `<iframe>`. The listener stops. Past 256
 * events it keeps nothing at all
 * (a flooding page loses its early hello, and is heard from its next one):
 * a gap in the middle could hide a load that must close the gate. */
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
