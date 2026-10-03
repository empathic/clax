// "Rally of 10" (spec §8, "Look"): an easter egg, shown once per browser per
// artifact when its tenth version is first viewed. Storage may throw.
export function rallyOnce(aid: string, version: number): boolean {
  if (version !== 10) return false;
  try {
    const k = `clax.rally.${aid}`;
    if (localStorage.getItem(k)) return false;
    localStorage.setItem(k, "1");
    return true;
  } catch { return false; }
}
