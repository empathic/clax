const CACHE_KEY = "clax.origin-ok";

export function artifactOrigin(id: string, loc: Location = location): string | null {
  if (loc.hostname !== "localhost" && loc.hostname !== "127.0.0.1") return null;
  const port = loc.port ? `:${loc.port}` : "";
  return `${loc.protocol}//${id}.localhost${port}`;
}

/** This tab's cached probe result: true or false once probed, null before (or without storage). */
export function cachedOriginOk(): boolean | null {
  try { const v = sessionStorage.getItem(CACHE_KEY); return v === null ? null : v === "1"; } catch { return null; }
}

/** Whether the browser resolves `<id>.localhost`. A sure answer is cached
 * for the session ([`cachedOriginOk`]), which callers consult first.
 *
 * The artifact origin's `/healthz` and this page's own `/healthz` are asked
 * together. The artifact origin is judged against the daemon, not the
 * clock: it gets 1 s from the moment the daemon answers on its own
 * origin, so a slow daemon is waited for instead of being taken for a name
 * that does not resolve, while a name that hangs still falls back about
 * 1 s after the daemon proved itself up. A verdict is cached only
 * when it is sure: the artifact origin answered, or it failed or hung while
 * the daemon answered here. When neither answers within 10 s (or the
 * artifact origin failed and the daemon is unreachable), the answer is
 * false for now and the next view probes again. */
export async function probeOrigin(origin: string, f: typeof fetch = fetch): Promise<boolean> {
  // 1 or 0 when sure (the cached form), undefined when not; the first to
  // settle it wins, and the request still out is then aborted. A timer left
  // behind fires within 10 s and does nothing.
  const ctl = new AbortController();
  const v = await new Promise<number | undefined>(done => {
    setTimeout(done, 10_000);
    // Truthy once the daemon answered here, which starts the grace.
    const up = f("/healthz", ctl).then(r => r.ok && setTimeout(done, 1000, 0), () => false);
    f(`${origin}/healthz`, ctl).then(r => done(+r.ok), () => up.then(u => done(u ? 0 : undefined)));
  });
  ctl.abort();
  if (v !== undefined) try { sessionStorage.setItem(CACHE_KEY, `${v}`); } catch { /* storage unavailable */ }
  return v === 1;
}

export function contentSrc(id: string, n: number, origin: string | null): string {
  return origin ? `${origin}/v/${n}/` : `/c/${id}/v/${n}/`;
}

/** Where the frame shows the page published at `file`: the version's root
 * for `index.html`, else the file's path under it, each segment encoded. */
export function pageSrc(id: string, n: number, origin: string | null, file: string): string {
  const root = contentSrc(id, n, origin);
  return file === "index.html" ? root : root + file.split("/").map(encodeURIComponent).join("/");
}

