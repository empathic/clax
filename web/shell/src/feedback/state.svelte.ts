// The running feedback integration, for a person who may report problems
// with Clax (`boot.ts` loads this only then): the overlay, the badge it
// tells, the header's menu (`FeedbackMenu.svelte`) and the keys sheet's row.
import type { AppView, Feedback, FeedbackConfig } from "./feedback";
import { setFeedbackMenu } from "./slot.svelte";

let running: Feedback | null = $state(null);
let count = $state(0);

/** The running integration once it has started, else null. */
export const feedback = { get current(): Feedback | null { return running; } };
/** How many of the person's reports wait on them. */
export const badge = { get current(): number { return count; } };

/** The keys sheet's row for the report shortcut, while it is bound. */
export function feedbackKeyRows(): { keys: string[]; what: string }[] {
  return running ? [{ keys: running.shortcut.keys, what: "Report a problem with Clax" }] : [];
}

/** Starts the overlay for `cfg` and fills the headers' menu slot. */
export async function start(cfg: FeedbackConfig, where?: () => AppView): Promise<void> {
  const [fb, M] = await Promise.all([import("./feedback").then(m => m.startFeedback(cfg, { where })), import("./FeedbackMenu.svelte")]);
  fb.onBadge(n => { count = n; });
  running = fb;
  setFeedbackMenu(M.default);
}
