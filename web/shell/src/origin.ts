const CACHE_KEY = "artifax.origin-ok";

export function artifactOrigin(id: string, loc: Location = location): string | null {
  if (loc.hostname !== "localhost" && loc.hostname !== "127.0.0.1") return null;
  const port = loc.port ? `:${loc.port}` : "";
  return `${loc.protocol}//${id}.localhost${port}`;
}

function readCache(): boolean | null {
  try { const v = sessionStorage.getItem(CACHE_KEY); return v === null ? null : v === "1"; } catch { return null; }
}
function writeCache(ok: boolean) { try { sessionStorage.setItem(CACHE_KEY, ok ? "1" : "0"); } catch { /* storage unavailable */ } }

/** Whether the browser resolves `<id>.localhost`; probed once per session. */
export async function probeOrigin(origin: string, fetchImpl: typeof fetch = fetch, timeoutMs = 1000): Promise<boolean> {
  const cached = readCache();
  if (cached !== null) return cached;
  const ctl = new AbortController();
  const timer = setTimeout(() => ctl.abort(), timeoutMs);
  let ok = false;
  try { ok = (await fetchImpl(`${origin}/healthz`, { signal: ctl.signal, mode: "cors" })).ok; } catch { ok = false; }
  clearTimeout(timer);
  writeCache(ok);
  return ok;
}

export function contentSrc(id: string, n: number, origin: string | null): string {
  return origin ? `${origin}/v/${n}/` : `/c/${id}/v/${n}/`;
}
