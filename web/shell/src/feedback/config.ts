// What the shell asks first, on every page: whether this person may report
// problems with Clax (`GET /_app/feedback/config`, the product's own route),
// and with what context. It is the one request the feedback integration
// makes for a person who may not; `feedback.ts` loads only for one who may.
import type { HostContext } from "../../../overlay/overlay";

/** Where the shell asks whether this person may report, and with what context. */
export const CONFIG_PATH = "/_app/feedback/config";
/** Where the shell's backend mints a reporter token for this person. */
export const TOKEN_PATH = "/_app/feedback/token";

/** `GET /_app/feedback/config`'s answer for a person who may file. */
export interface FeedbackConfig {
  /** The Clax instance the overlay files to (its base URL). */
  instance: string;
  /** The feedback queue's ID. */
  queue: string;
  /** The release, flags and app the person's page runs, as the backend
   * knew them when the page loaded. */
  context: HostContext;
}

/** The overlay's classic script's global, as `overlay.d.ts` declares it. */
export type ClaxGlobal = Window["Clax"];

/** Where in the shell the person is when a capture starts. */
export interface AppView {
  /** `gallery`, `artifact`, or the gallery page's name (`queues`, `people`, `me`, …). */
  view: string;
  /** The artifact shown, on an artifact view. */
  artifact: string | null;
}

/** `GET /_app/feedback/config`, or null when this person may not report
 * (the integration off, no session, no `feedback` flag) or the route could
 * not be read: the shell then shows no entry point. */
export async function loadConfig(f: typeof fetch = fetch): Promise<FeedbackConfig | null> {
  try {
    const r = await f(CONFIG_PATH, { credentials: "same-origin", headers: { accept: "application/json" } });
    if (!r.ok) return null;
    const b = (await r.json()) as { enabled?: unknown; instance?: unknown; queue?: unknown; context?: unknown };
    if (b?.enabled !== true || typeof b.instance !== "string" || !b.instance || typeof b.queue !== "string" || !b.queue) return null;
    const context = b.context && typeof b.context === "object" ? (b.context as HostContext) : {};
    return { instance: b.instance.replace(/\/+$/, ""), queue: b.queue, context };
  } catch {
    return null;
  }
}
