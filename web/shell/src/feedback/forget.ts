// Forgetting the person's reporter on sign-out (docs/feedback.md, section
// 2): a chunk of its own, loaded by the shell's sign-out buttons alone.
import { readConfig } from "./boot";
import { type ClaxGlobal, configOnce } from "./config";

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
