// The version changelog as Echo shows it (spec §8, §10): no band over the
// page. A load decides the Addressed group (frozen until the next decision),
// the version button's dot and the summary's line, from this viewer's
// attention; the version menu reads as a changelog.
import type { Attention, Version } from "../api";

export type Decided = { n: number; ids: string[]; dot: boolean; line: string | null };

/** The changelog for the latest version `latest`: the threads of the group
 * (those it addressed that this viewer is in and has not looked at), the dot
 * (a version newer than this viewer's last view exists) and the summary line
 * (decided: Q11). Nothing for a pinned or anonymous view. */
export function decide(versions: Version[], latest: number, att: Attention | null, pinned: boolean): Decided {
  const none = { n: latest, ids: [], dot: false, line: null };
  if (pinned || !att) return none;
  const v = versions.find(x => x.n === latest);
  const addressed = new Set(att.addressed);
  const ids = (v?.addresses ?? []).filter(id => addressed.has(id));
  const seen = att.seen;
  const dot = seen !== null && latest > seen;
  let line: string | null = null;
  if (seen !== null && latest - seen > 1) {
    const k = new Set(versions.filter(x => x.n > seen).flatMap(x => x.addresses ?? []).filter(id => addressed.has(id))).size;
    if (k) line = `${latest - seen} new versions · ${k} addressed`;
  } else if (ids.length) line = `v${latest} addressed ${ids.length}`;
  return { n: latest, ids, dot, line };
}
