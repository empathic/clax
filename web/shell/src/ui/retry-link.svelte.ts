// One card's Retry of the writes and question copies its artifact's link did
// not send (a thread card's, a question card's): the call the Publish
// popover's Retry makes (`retryLink`, POST
// /api/artifacts/<id>/remotes/<remote>/retry), loaded with the first press.
// A Retry moves no event of its own, so the view it was taken on reads as
// sending again until a newer view comes (`retried`). A Retry that failed
// says why on the card's line (`error`) until the next press or a newer view.
// A closure, not a class: a class's runes compile to private fields, whose
// helpers would weigh on the question module's chunk.
import { ApiError } from "../api";

/** What a failed Retry says: the daemon's message for a refusal, else that it did not answer. */
export const retryError = (e: unknown): string =>
  `Retry failed: ${e instanceof ApiError && e.status ? e.message.replace(/^\d+ /, "") : "Clax did not answer"}`;

export type RetryLink<V> = {
  /** A Retry is in flight. */
  readonly busy: boolean;
  /** Retry was taken on `view` and succeeded: what it had not sent reads as sending again. */
  retried(view: V): boolean;
  /** Why the Retry taken on `view` failed; null when none did. */
  error(view: V): string | null;
  /** Retries `artifact`'s link on `remote`, taken on `view`; one at a time. */
  run(view: V, artifact: string, remote: string): Promise<void>;
};

/** A card's Retry state (see above). */
export function retryLink<V>(): RetryLink<V> {
  let busy = $state(false);
  let on = $state.raw<V | null>(null);
  let failed = $state.raw<{ view: V; text: string } | null>(null);
  return {
    get busy() { return busy; },
    retried: view => on === view,
    error: view => (failed?.view === view ? failed.text : null),
    async run(view, artifact, remote) {
      if (busy) return;
      busy = true;
      failed = null;
      try {
        const api = await import("./publish-api");
        await api.retryLink(artifact, remote);
        on = view;
      } catch (e) {
        failed = { view, text: retryError(e) };
      } finally {
        busy = false;
      }
    },
  };
}
