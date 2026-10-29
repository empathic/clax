// Whether this browser is one of the owner's, or has an inbox (the owner's
// or a connected person's), asked only where those surfaces may load (the
// gallery's and the artifact view's questions and inbox). Kept out of the eager entries: those load it after their first paint.
import { getToken } from "./api";
import { backoff } from "./lifecycle";

/** Whether this is a browser of the owner's: the daemon serves it the token
 * (`GET /api/token`: on a local daemon to a browser holding an owner
 * browser session, on a hosted daemon to the owner's signed-in browser as
 * its shell token). A refusal (401 or 403) is the answer no; any other
 * failure (the daemon restarting, say) is asked again after a backoff, so
 * it resolves only once it knows, or with false once `signal` aborts (the
 * asker is gone: it asks no more). */
export async function ownerBrowser(signal?: AbortSignal): Promise<boolean> {
  if (await getToken()) return !signal?.aborted;
  return ask("/api/token", r => r.status === 403 || r.status === 401, signal);
}

/** A viewer as `GET /api/viewers/me` gives it: `person` for a connected
 * person's browser, `person_revoked_at` once the person is revoked. */
export type Me = { person?: boolean; person_revoked_at?: string | null } | null | undefined;

/** Whether this browser has an inbox (spec
 * 2026-10-08-multi-agent-multi-person §18): a connected person's, as `me`
 * (the page's own `GET /api/viewers/me`, asked once, as it answers) says, or the owner's
 * (as [`ownerBrowser`]). A LAN viewer's has none, and asks nothing of the
 * inbox to learn it; a connected person's needs no other request. The
 * inbox's routes stay the authority: a person revoked since is refused
 * when the feeds start. */
export async function inboxBrowser(me: Promise<Me>, signal?: AbortSignal): Promise<boolean> {
  const v = await me.catch(() => null);
  if (signal?.aborted) return false;
  return (!!v?.person && !v.person_revoked_at) || ownerBrowser(signal);
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
