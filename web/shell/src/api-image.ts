// Images the shell reads under `/api/` (a thread's clip), fetched and shown
// by object URL, since an `<img src>` there would load nothing: in the Clax
// app its scheme answers nothing under `/api/` (only the app's transport
// reaches the core's API, `transport.ts`), and a local owner's browser
// proves its session only on a fetch (`owner-proof.ts` wraps `fetch`; an
// image's request carries the cookie alone, which the daemon refuses).

/** How many object URLs are kept: the most recently asked for. */
export const CLIPS_KEPT = 32;

/** The object URLs made, by path, least recently asked for first. */
const images = new Map<string, Promise<string>>();

/** An object URL of the bytes `win.fetch` reads at `path`, made once per
 * path while it is among the `CLIPS_KEPT` most recently asked for; the URL
 * of one that falls out is revoked, and the next ask fetches it again.
 * Rejects when the answer is not OK; the next ask fetches again. */
export function apiImageSrc(path: string, win: Window = window): Promise<string> {
  let p = images.get(path);
  if (p) images.delete(path);
  else {
    p = win.fetch(path).then(async r => {
      if (!r.ok) throw new Error(`${path} answered ${r.status}`);
      return URL.createObjectURL(await r.blob());
    });
    const made = p;
    made.catch(() => { if (images.get(path) === made) images.delete(path); });
  }
  images.set(path, p);
  for (const [old, q] of images) {
    if (images.size <= CLIPS_KEPT) break;
    images.delete(old);
    q.then(u => URL.revokeObjectURL(u), () => {});
  }
  return p;
}
