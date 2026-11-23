// The artifact view's question surfaces (Inbox's count, the questions at the
// top of the thread list), for the owner and a connected person: loaded as
// the view becomes ready, in a browser known to have an inbox
// (`view/deciders.svelte.ts`), none of it in the artifact entry. The thread
// list shows once its questions are drawn above it (`decided`), so they do
// not push its cards down after they show; the first read of the inbox is
// the bootstrap block's, or the one asked beside the view's data, so that is
// a chunk's wait. A read or a module slower than `QUESTIONS_WAIT_MS` is not
// waited for longer: the list shows, and the questions come above it later.
import { after } from "../clock";
import { pageStream } from "../stream";
import type { FirstInbox } from "./deciders.svelte";
import { guardedAction, keyboardTrail } from "./trail";

/** The longest the thread list waits for its questions, from when they are asked. */
export const QUESTIONS_WAIT_MS = 3000;

/** Shows the question surfaces of artifact `id`, seeding the feeds with
 * `first` (else reading them first); `onCount` hears Inbox's unread count;
 * `decided` runs once, when the surfaces are mounted (or failed to load),
 * or `QUESTIONS_WAIT_MS` after this was called, whichever is first. The
 * result ends them, and ends a load still under way. */
export function artifactQuestions(id: string, first: Promise<FirstInbox> | null, onCount: (n: number) => void, decided: () => void): () => void {
  let ended = false;
  let stop: (() => void) | null = null;
  let told = false;
  const decide = () => { if (!told) { told = true; decided(); } };
  const cap = after(QUESTIONS_WAIT_MS, decide);
  void import("../q").then(async m => {
    const f = first ? await first : null;
    if (f) m.seed(f.summary, f.open);
    else await m.firstRead();
    if (!ended) stop = m.artifact(id, { keyboardTrail, guardedAction }, pageStream(), document, onCount);
  }, () => {}).finally(() => { cap(); decide(); });
  return () => {
    ended = true;
    cap();
    decide();
    stop?.();
    stop = null;
  };
}
