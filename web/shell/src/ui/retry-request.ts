// A card's Retry of the writes and question copies its artifact's link did
// not send (a thread card's, a question card's): the call the Publish
// popover's Retry makes (`retryLink`, POST
// /api/artifacts/<id>/remotes/<remote>/retry). `RetryButton` loads this
// module with its first press, so none of it weighs on the cards' chunks.
import { ApiError } from "../api";
import { retryLink } from "./publish-api";

/** Retries `artifact`'s link on `remote`: null once the daemon took it, else
 * what the card's line says of the failure (the daemon's message for a
 * refusal, else that Clax did not answer). */
export async function retryCopies(artifact: string, remote: string): Promise<string | null> {
  try {
    await retryLink(artifact, remote);
    return null;
  } catch (e) {
    return `Retry failed: ${e instanceof ApiError && e.status ? e.message.replace(/^\d+ /, "") : "Clax did not answer"}`;
  }
}
