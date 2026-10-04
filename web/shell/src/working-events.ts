// The gallery's watch of the page's event stream.
import type { ArtifactEvent } from "./events";
import { pageStream } from "./stream";

/** The events the gallery's chips and cards follow. */
export const GALLERY_TYPES = ["working", "version", "thread", "thread_deleted", "artifact_deleted"] as const;

/** Watches every artifact's `working`, `version`, `thread`, `thread_deleted`
 * and `artifact_deleted` events (the gallery's chips and the cards they
 * name) on the page's one stream; returns the unwatch. */
export function subscribeGallery(onEvent: (e: ArtifactEvent) => void): () => void {
  return pageStream().watch({ types: GALLERY_TYPES }, onEvent);
}
