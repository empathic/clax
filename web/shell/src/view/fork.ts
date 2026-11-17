// The top bar's Fork (spec 2026-10-11-clax-dev-hosted-design H11): a lazy
// chunk, loaded on the first press, so the artifact entry carries none of it.
import { getToken } from "../api";
import { nav } from "../nav";
import { ApiError, failureText, readJson } from "../failure";

/** The notice of a fork the daemon did not make, before its reason. */
export const FORK_FAILED = "The fork was not made";

/** Forks version `n` of artifact `id` into a new artifact of the viewer's
 * (`POST /api/artifacts/<id>/fork`) and opens it. The owner's shell sends
 * its token; anyone else's request is their browser's. A refusal is shown
 * with `notice`, with the daemon's reason. */
export async function forkVersion(id: string, n: number, notice: (text: string) => void): Promise<void> {
  try {
    const token = await getToken();
    const headers: Record<string, string> = { "content-type": "application/json" };
    if (token) headers.authorization = `Bearer ${token}`;
    const res = await fetch(`/api/artifacts/${encodeURIComponent(id)}/fork`, { method: "POST", headers, body: JSON.stringify({ version: n }) });
    const body = await readJson<{ url?: unknown }>(res);
    if (typeof body.url !== "string") throw new ApiError(res.status, "");
    nav.assign(body.url);
  } catch (e) {
    notice(failureText(FORK_FAILED, e));
  }
}
