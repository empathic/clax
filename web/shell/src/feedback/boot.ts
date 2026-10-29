// The shell's feedback entry point (spec 2026-10-29-feedback-loop §10.3):
// it reads the answer of `GET /_app/feedback/config` (`configOnce`, which
// each entry asks first thing), and only for a person who may report does
// it load the integration (`state.svelte.ts` and `feedback.ts`, chunks of
// their own), the overlay from the instance, and the header's menu. For
// anyone else that one request is all: no menu, no shortcut, no overlay.
import type { HostContext } from "../../../overlay/overlay";
import { type AppView, type ClaxGlobal, type FeedbackConfig, askConfig, configOnce } from "./config";

/** The config route's body as a `FeedbackConfig`, or null when this person
 * may not report (the integration off, no session, no `feedback` flag) or
 * the body is not one: the shell then shows no entry point. */
export function readConfig(body: unknown): FeedbackConfig | null {
  const b = body as { enabled?: unknown; instance?: unknown; queue?: unknown; user?: unknown; context?: unknown } | null;
  if (b?.enabled !== true || typeof b.instance !== "string" || !b.instance || typeof b.queue !== "string" || !b.queue) return null;
  const context = b.context && typeof b.context === "object" ? (b.context as HostContext) : {};
  const user = typeof b.user === "string" && b.user ? b.user : null;
  return { instance: b.instance.replace(/\/+$/, ""), queue: b.queue, user, context };
}

/** Asks the config route afresh and reads its answer. */
export function loadConfig(f: typeof fetch = fetch): Promise<FeedbackConfig | null> {
  return askConfig(f).then(readConfig);
}

let started = false;

/** Starts the shell's feedback, once per page: `where` names the view a
 * capture starts in (default from the page's address). A failure at any
 * step leaves the shell without it. */
export function bootFeedback(where?: () => AppView): void {
  if (started) return;
  started = true;
  void configOnce().then(readConfig).then(cfg => cfg && import("./state.svelte").then(m => m.start(cfg, where)))
    .catch(e => { console.warn("Clax feedback is off:", e); });
}

export interface ForgetDeps {
  /** The config route's body; default this page's (`configOnce`). */
  config?: () => Promise<unknown>;
  /** Loads the overlay's global; default `loadOverlay` from `feedback.ts`. */
  load?: (doc: Document, instance: string) => Promise<ClaxGlobal>;
  doc?: Document;
}

/** Forgets, as the person signs out, the reporter key and token the
 * overlay keeps for them (`Clax.forgetReporter`), so whoever signs in
 * next on this browser shares neither. Call it before the session ends:
 * the config route then still names the person. For a person who may not
 * report it does nothing more than read the config; it never throws, and
 * a failure leaves sign-out to go on. */
export async function forgetFeedback(deps: ForgetDeps = {}): Promise<void> {
  try {
    const cfg = readConfig(await (deps.config ?? configOnce)());
    if (!cfg?.user) return;
    const load = deps.load ?? (await import("./feedback")).loadOverlay;
    const Clax = await load(deps.doc ?? document, cfg.instance);
    await Clax.forgetReporter({ instance: cfg.instance, queue: cfg.queue, user: cfg.user });
  } catch (e) {
    console.warn("Clax feedback: the reporter key was not forgotten:", e);
  }
}
