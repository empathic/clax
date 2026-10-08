// The artifact view's question surfaces (the top bar's Inbox, the sidebar's
// questions), for the owner. The view's stream module loads this once the
// view knows its viewer, and none of it is in the artifact entry: it waits
// for the browser to be idle, asks whether this is an owner's browser, and
// only then loads the question module.
import { ownerBrowser } from "../owner";
import { pageStream } from "../stream";
import { guardedAction, keyboardTrail } from "./trail";

/** Shows the question surfaces of artifact `id` in an owner's browser; the
 * result ends them, and ends a check or a load still under way. */
export function artifactQuestions(id: string): () => void {
  const life = new AbortController();
  let stop: (() => void) | null = null;
  const load = () => void import("../q").then(m => { if (!life.signal.aborted) stop = m.artifact(id, { keyboardTrail, guardedAction }, pageStream()); }, () => {});
  const owner = () => void ownerBrowser(life.signal).then(o => { if (o && !life.signal.aborted) load(); });
  let cancel: () => void;
  if (typeof requestIdleCallback === "function") {
    const h = requestIdleCallback(owner, { timeout: 2000 });
    cancel = () => cancelIdleCallback(h);
  } else {
    const h = setTimeout(owner, 200);
    cancel = () => clearTimeout(h);
  }
  return () => {
    cancel();
    life.abort();
    stop?.();
    stop = null;
  };
}
