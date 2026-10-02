// Clax's `working()` state for the page (clax-extensions.d.ts
// `WorkingState`). Loaded by the comments handler on the page's first
// working call.
import { harnessLabel, newestFirst, type Working } from "../view/working-model";

/** Agents newest first; threads as the handles this document holds, the
 * rest counted. No record key, no session, no store ID. */
export function pageWorking(list: Working[], handleOf: (id: string) => string | undefined) {
  const agents = newestFirst(list).map(w => {
    const threads = w.thread_ids.map(handleOf).filter((h): h is string => h !== undefined);
    return { harness: w.harness, label: harnessLabel(w.harness), message: w.message, since: w.started_at, threads, otherThreads: w.thread_ids.length - threads.length };
  });
  return { working: agents.length > 0, agents };
}
