// The gallery's watch of the page's event stream.
import { type StreamEvent, pageStream } from "./stream";

/** Watches the `gallery` topic on the page's one stream: every artifact's
 * `version`, `thread` (a summary without comments), `thread_deleted`,
 * `artifact_deleted` and `working` (a summary per agent); returns the unwatch. */
export function subscribeGallery(onEvent: (e: StreamEvent) => void): () => void {
  return pageStream().watch(["gallery"], onEvent);
}
