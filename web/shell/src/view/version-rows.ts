// The version menu's rows (spec §8): who published each version and when, the threads it addressed, what this viewer did about them, and its note. Loaded with the menu's panel.
import type { Version } from "../api";
import { relativeTime } from "../format";
import type { Thread } from "../threads";
import { agentName } from "./history-model";

/** A thread a version addressed: its pin number (null when it has none on
 * this page), and whether this viewer answered it and it is still open. */
export type Chip = { id: string; n: number | null; open: boolean };
export type Row = { n: number; current: boolean; latest: boolean; who: string; when: string; chips: Chip[]; did: string | null; note: string | null; label: string | null };
/** `numbers`: the pin numbering the sidebar uses; `me`: this viewer's public ID. */
export type RowInput = { versions: Version[]; latest: number; shown: number; now: Date; threads: Thread[]; numbers: Map<string, number>; me: string | null };

/** The versions newest first. */
export function versionRows(i: RowInput): Row[] {
  const byId = new Map(i.threads.map(t => [t.id, t]));
  return [...i.versions].sort((a, b) => b.n - a.n).map(v => {
    const chips: Chip[] = [];
    const did: string[] = [];
    for (const id of v.addresses ?? []) {
      const t = byId.get(id);
      if (!t) continue;
      const mineAfter = t.comments.some(c => c.author_public_id === i.me && c.created_at > v.created_at);
      chips.push({ id, n: i.numbers.get(id) ?? null, open: t.status === "open" && mineAfter });
      const num = i.numbers.get(id);
      if (t.status === "resolved" && t.resolved_by === `viewer:${i.me}`) did.push(`you resolved ${num ? `#${num}` : "it"}`);
      else if (t.status === "open" && mineAfter) did.push(`you replied on ${num ? `#${num}` : "it"}, still open`);
    }
    return {
      n: v.n, current: v.n === i.shown, latest: v.n === i.latest, who: v.agent_harness ? agentName(v.agent_harness) : "command line",
      when: relativeTime(v.created_at, i.now), chips, did: did.length ? did.join("; ") : null,
      note: v.note ?? (v.n === 1 ? "First publish" : null), label: v.label,
    };
  });
}

/** The thread's first comment on one line, at most 80 characters. */
export function excerpt(t: Thread): string {
  const s = (t.comments[0]?.body ?? "").split(/\s+/).filter(Boolean).join(" ");
  return s.length > 80 ? `${s.slice(0, 80)}…` : s;
}
