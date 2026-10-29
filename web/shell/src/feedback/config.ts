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

/** `GET /_app/feedback/config`'s body, or null when it could not be read
 * (`readConfig` in `boot.ts` reads it). */
export function askConfig(f: typeof fetch = fetch): Promise<unknown> {
  return f(CONFIG_PATH, { credentials: "same-origin" }).then(r => (r.ok ? r.json() : null)).catch(() => null);
}

let asked: Promise<unknown> | null = null;

/** `askConfig`, once per page: `boot.ts`, which each entry runs first thing. */
export function configOnce(): Promise<unknown> {
  return (asked ??= askConfig());
}

