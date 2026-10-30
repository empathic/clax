// Forgetting the person's reporter on sign-out (docs/feedback.md, section
// 2): a chunk of its own, loaded by the shell's sign-out buttons alone.
import { readConfig } from "./boot";
import { type ClaxGlobal, configOnce } from "./config";

/** How long sign-out waits for the forgetting (the config request and the
 * overlay script's load), in ms. */
export const FORGET_TIMEOUT_MS = 3000;

export interface ForgetDeps {
  /** The config route's body; default this page's (`configOnce`). */
  config?: () => Promise<unknown>;
  /** Loads the overlay's global; default `loadOverlay` from `feedback.ts`. */
  load?: (doc: Document, instance: string) => Promise<ClaxGlobal>;
  doc?: Document;
  /** The running integration, whose overlay forgets its own client;
   * default the one this page started (`state.svelte.ts`). */
  running?: () => Promise<{ forget(): Promise<void> } | null>;
  /** Calls `fire` once `ms` have passed; the result cancels it. Default
   * `setTimeout`. */
  timer?: (ms: number, fire: () => void) => () => void;
}

/** Forgets, as the person signs out, the reporter key and token the
 * overlay keeps for them, so whoever signs in next on this browser shares
 * neither: through the running overlay's `forget()` when this page started
 * one (which also stops its client writing them back), else
 * `Clax.forgetReporter`. Call it before the session ends: the config route
 * then still names the person. For a person who may not report it does
 * nothing more than read the config. It never throws, and gives up after
 * [`FORGET_TIMEOUT_MS`], so sign-out always goes on. */
export async function forgetFeedback(deps: ForgetDeps = {}): Promise<void> {
  const timer = deps.timer ?? ((ms, fire) => { const t = setTimeout(fire, ms); return () => clearTimeout(t); });
  let cancel = () => {};
  const timedOut = new Promise<void>(resolve => { cancel = timer(FORGET_TIMEOUT_MS, () => { console.warn("Clax feedback: forgetting the reporter key timed out"); resolve(); }); });
  const work = (async () => {
    try {
      const running = await (deps.running ?? (() => import("./state.svelte").then(m => m.feedback.current)))();
      if (running) return await running.forget();
      const cfg = readConfig(await (deps.config ?? configOnce)());
      if (!cfg?.user) return;
      const load = deps.load ?? (await import("./feedback")).loadOverlay;
      const Clax = await load(deps.doc ?? document, cfg.instance);
      await Clax.forgetReporter({ instance: cfg.instance, queue: cfg.queue, user: cfg.user });
    } catch (e) {
      console.warn("Clax feedback: the reporter key was not forgotten:", e);
    }
  })();
  await Promise.race([work, timedOut]);
  cancel();
}
