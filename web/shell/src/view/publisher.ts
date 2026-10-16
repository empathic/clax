// The artifact view's line under the title. Its own module, so the gallery's
// and the artifact view's first-paint bundles share none of the gallery's model.
import type { Artifact } from "../api";

/** Who published the artifact: its owner session's agent name, else its harness, else the command
 * line; a live page by its URL without the scheme (`Live page · localhost:5173/settings`). */
export function publisherText(a: Artifact): string {
  const by = a.owner_name || a.owner_harness;
  return a.live ? `Live page · ${a.live.page_url.replace(/^\w+:\/\//, "")}` : by ? `published by ${by}` : "published from the command line";
}
