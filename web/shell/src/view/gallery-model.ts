import type { Artifact } from "../api";

/** The artifacts whose title or description contains `query` (trimmed, any case); all of them for an empty query. */
export function filterArtifacts(list: Artifact[], query: string): Artifact[] {
  const q = query.trim().toLowerCase();
  if (!q) return list;
  return list.filter(a => a.title.toLowerCase().includes(q) || (a.description ?? "").toLowerCase().includes(q));
}

/** Who published the card: a harness session, an agent session, or null for the command line. */
export function publisherText(a: Artifact): string | null {
  if (!a.owner_session_id) return null;
  return `published by ${a.owner_harness ? `${a.owner_harness} session` : "an agent session"}`;
}
