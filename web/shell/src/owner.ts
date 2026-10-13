// Whether this browser is one of the owner's, or has an inbox (the owner's
// or a connected person's), asked only where those surfaces may load (the
// gallery's and the artifact view's questions and inbox). Kept out of the eager entries: those load it after their first paint.
import { getToken } from "./api";
import { backoff } from "./lifecycle";

/** Whether this is a browser of the owner's: the daemon serves it the token
 * (`GET /api/token`, to loopback browsers only). A refusal (403) is the
 * answer no; any other failure (the daemon restarting, say) is asked again
 * after a backoff, so it resolves only once it knows, or with false once
 * `signal` aborts (the asker is gone: it asks no more). */
export async function ownerBrowser(signal?: AbortSignal): Promise<boolean> {
  if (await getToken()) return !signal?.aborted;
  return ask("/api/token", r => r.status === 403, signal);
}

/** Whether this browser has an inbox: the owner's, or a connected
 * person's, to whom `GET /api/inbox` answers (spec
 * 2026-10-08-multi-agent-multi-person §18); a LAN viewer's has none (a
 * 4xx). As [`ownerBrowser`], a failure that is no answer is asked again
 * after a backoff, and an abort answers false. */
export async function inboxBrowser(signal?: AbortSignal): Promise<boolean> {
  return (await ownerBrowser(signal)) || ask("/api/inbox?limit=1", r => r.status >= 400 && r.status < 500, signal);
}

/** Asks `url` until it answers: true for a success, false for a refusal
 * (`no`) or once `signal` aborts; anything else is asked again after a backoff. */
async function ask(url: string, no: (r: Response) => boolean, signal?: AbortSignal): Promise<boolean> {
  const gone = () => !!signal?.aborted;
  for (let n = 0; !gone(); n++) {
    const r = await fetch(url, { cache: "no-store", signal }).catch(() => null);
    if (gone()) break;
    if (r?.ok) return true;
    if (r && no(r)) return false;
    await new Promise<void>(f => {
      const t = setTimeout(done, backoff(n));
      function done() { clearTimeout(t); signal?.removeEventListener("abort", done); f(); }
      signal?.addEventListener("abort", done);
    });
  }
  return false;
}
