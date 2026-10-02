import type { Artifact } from "../api";

/** The artifacts whose title or description contains `query` (trimmed, any case); all of them for an empty query. */
export function filterArtifacts(list: Artifact[], query: string): Artifact[] {
  const q = query.trim().toLowerCase();
  if (!q) return list;
  return list.filter(a => a.title.toLowerCase().includes(q) || (a.description ?? "").toLowerCase().includes(q));
}

/** Who published the artifact: its owner session's harness, else the command line. */
export function publisherText(a: Artifact): string {
  return a.owner_harness ? `published by ${a.owner_harness}` : "published from the command line";
}
