// Whether this browser is one of the owner's, asked only where the owner's
// surfaces may load (the gallery's and the artifact view's questions and
// inbox). Kept out of the eager entries: those load it after their first paint.
import { getToken } from "./api";
import { backoff } from "./lifecycle";

/** Whether this is a browser of the owner's: the daemon serves it the token
 * (`GET /api/token`, to loopback browsers only). A refusal (403) is the
 * answer no; any other failure (the daemon restarting, say) is asked again
 * after a backoff, so it resolves only once it knows, or with false once
 * `signal` aborts (the asker is gone: it asks no more). */
export async function ownerBrowser(signal?: AbortSignal): Promise<boolean> {
  if (await getToken()) return !signal?.aborted;
  const gone = () => !!signal?.aborted;
  for (let n = 0; !gone(); n++) {
    const r = await fetch("/api/token", { cache: "no-store", signal }).catch(() => null);
    if (gone()) break;
    if (r?.ok) return true;
    if (r?.status === 403) return false;
    await new Promise<void>(f => {
      const t = setTimeout(done, backoff(n));
      function done() { clearTimeout(t); signal?.removeEventListener("abort", done); f(); }
      signal?.addEventListener("abort", done);
    });
  }
  return false;
}
