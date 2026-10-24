export function relativeTime(iso: string, now: Date = new Date()): string {
  const t = new Date(iso).getTime();
  const s = Math.max(0, Math.round((now.getTime() - t) / 1000));
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.round(s / 60)} min ago`;
  if (s < 86400) return `${Math.round(s / 3600)} h ago`;
  if (s < 7 * 86400) return `${Math.round(s / 86400)} d ago`;
  return iso.slice(0, 10);
}

/** How much a person made, on one line: "3 pages · 2.5 MB" (megabytes to
 * one decimal). Here, beside the gallery's own formatting, so People's
 * chunk does not carry it. */
export const madeText = (m: { artifacts: number; asset_bytes: number }) =>
  `${m.artifacts} page${m.artifacts === 1 ? "" : "s"} · ${Math.round(m.asset_bytes / 1e5) / 10} MB`;
