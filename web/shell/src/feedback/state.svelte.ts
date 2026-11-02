// The running feedback integration, for a person who may report problems
// with Clax (`boot.ts` loads this only then): the overlay, the badge it
// tells, and the slot it fills (`slot.svelte.ts`: the header's menu,
// `FeedbackMenu.svelte`, the keys sheet's row, F from the artifact's frame).
import type { AppView, Feedback, FeedbackConfig } from "./feedback";
import { setFeedbackMenu, setReportKey } from "./slot.svelte";

let running: Feedback | null = $state(null);
let count = $state(0);

/** The running integration once it has started, else null. */
export const feedback = { get current(): Feedback | null { return running; } };
/** How many of the person's reports wait on them. */
export const badge = { get current(): number { return count; } };

/** Starts the overlay for `cfg` and fills the slot. */
export async function start(cfg: FeedbackConfig, where?: () => AppView): Promise<void> {
  const [fb, M] = await Promise.all([import("./feedback").then(m => m.startFeedback(cfg, { where })), import("./FeedbackMenu.svelte")]);
  fb.onBadge(n => { count = n; });
  running = fb;
  setReportKey(() => fb.report());
  setFeedbackMenu(M.default, [...fb.shortcut.keys]);
}
