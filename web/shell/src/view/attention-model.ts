// Needs your eyes (spec §8, "Gallery"): from this viewer's attention, which
// artifacts float to the top and which markers each card carries. A
// never-viewed artifact does not need your eyes for its version alone.
import type { Artifact, AttentionSummary } from "../api";
import { orderArtifacts } from "./gallery-model";

export type Marker = { kind: "you" | "new" | "rep" | "ag" | "oth"; text: string };
const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

export function needsEyes(a: Artifact, att?: AttentionSummary): boolean {
  if (!att) return false;
  return att.addressed.length > 0 || att.new_replies.length > 0 || (att.seen !== null && a.current_version > att.seen);
}

export function markers(a: Artifact, att: AttentionSummary | undefined, working: string[]): Marker[] {
  const out: Marker[] = [];
  if (att?.addressed.length) out.push({ kind: "you", text: `${att.addressed.length} addressed in v${att.addressed_v ?? a.current_version}` });
  if (att && att.seen !== null && a.current_version > att.seen) out.push({ kind: "new", text: `v${a.current_version} new` });
  if (att?.new_replies.length) out.push({ kind: "rep", text: plural(att.new_replies.length, "new reply", "new replies") });
  for (const w of working) out.push({ kind: "ag", text: w });
  if (att?.open_in.length) out.push({ kind: "oth", text: `${att.open_in.length} open` });
  return out;
}

export function groups(list: Artifact[], att: Record<string, AttentionSummary>): { needs: Artifact[]; rest: Artifact[] } {
  const needs = list.filter(a => needsEyes(a, att[a.id])).sort((x, y) => y.updated_at.localeCompare(x.updated_at));
  const rest = orderArtifacts(list.filter(a => !needs.includes(a)));
  return { needs, rest };
}

export const seenText = (att?: AttentionSummary): string | null => (att?.seen != null ? `seen v${att.seen}` : null);
