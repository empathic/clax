// Presence (spec §10, "Presence"): here or away, and where, if shared.
import type { Participants } from "../api";
import { relativeTime } from "../format";
import { anchorLabel } from "../threads";
import type { ViewState } from "./artifact-controller";

export type PresenceView = { public_id: string; display_name: string | null; state: "here" | "away" | "gone"; where: string | null; since: string };
export const AWAY_AFTER_MS = 5 * 60_000;
export const stateFor = (visible: boolean, idleMs: number): "here" | "away" => (visible && idleMs < AWAY_AFTER_MS ? "here" : "away");

/** Where this viewer is looking: the thread they selected, else the anchor they are writing on. */
export function whereLabel(s: ViewState): string | null {
  const t = s.threads.find(x => x.id === s.selected);
  if (t) return anchorLabel(t.anchor);
  return s.draft?.anchor ? anchorLabel(s.draft.anchor) : null;
}

export function personLine(p: PresenceView, now: Date): string {
  if (p.state === "gone") return `last here ${relativeTime(p.since, now)}`;
  if (p.state === "away") return "away";
  return p.where ? `here, looking at ${p.where}` : "here";
}

/** Everyone to list: the artifact's people, then present viewers who never
 * commented (presence supplies their names). A gone entry adds no one. */
export function roster(people: Participants["people"], presence: PresenceView[]): Participants["people"] {
  const known = new Set(people.map(p => p.public_id));
  const extra = presence.filter(p => p.state !== "gone" && !known.has(p.public_id)).map(p => ({ public_id: p.public_id, display_name: p.display_name, seen: null }));
  return [...people, ...extra];
}

/** Public ID → state, for the roster's here and away marks. */
export const presenceMap = (presence: PresenceView[]): Map<string, PresenceView["state"]> => new Map(presence.map(p => [p.public_id, p.state]));
