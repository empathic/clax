import type { Artifact } from "../api";

/** The artifacts whose title or description contains `query` (trimmed, any case); all of them for an empty query. */
export function filterArtifacts(list: Artifact[], query: string): Artifact[] {
  const q = query.trim().toLowerCase();
  if (!q) return list;
  return list.filter(a => a.title.toLowerCase().includes(q) || (a.description ?? "").toLowerCase().includes(q));
}

/** Pinned first, then the most recently updated. */
export function orderArtifacts(list: Artifact[]): Artifact[] {
  return [...list].sort((a, b) => Number(b.pinned) - Number(a.pinned) || b.updated_at.localeCompare(a.updated_at));
}

/** The "rally of 10" easter egg: the artifact is at its tenth version. */
export const rally = (a: Artifact): boolean => a.current_version === 10;
