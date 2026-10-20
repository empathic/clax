// Images the shell reads under `/api/` (a thread's clip), in the Clax app:
// its scheme answers nothing under `/api/` (only the app's transport reaches
// the core's API, `transport.ts`), so an `<img src>` there loads nothing.
// The shell loads this module only in the app (`onHost`); a browser points
// the image at its URL.

/** The object URLs made, by path. */
const images = new Map<string, Promise<string>>();

/** An object URL of the bytes `win.fetch` reads at `path`, made once per
 * path and kept for the document's life. Rejects when the answer is not
 * OK; the next ask fetches again. */
export function apiImageSrc(path: string, win: Window = window): Promise<string> {
  let p = images.get(path);
  if (!p) {
    p = win.fetch(path).then(async r => {
      if (!r.ok) throw new Error(`${path} answered ${r.status}`);
      return URL.createObjectURL(await r.blob());
    });
    p.catch(() => images.delete(path));
    images.set(path, p);
  }
  return p;
}
