// The gallery's attention grouping, markers, working chips and rosters, and
// its live refreshes, are not needed for the first paint: the gallery loads
// this module once its first list has painted, and until then shows one list
// in the usual order.
import type { Artifact, AttentionSummary } from "../api";
import { markers } from "../view/attention-model";
import { agentNames, chips, type Working } from "../view/working-model";

export { groups } from "../view/attention-model";
export { default as Markers } from "./Markers.svelte";
export { default as NeedsGroup } from "./NeedsGroup.svelte";
export { default as Roster } from "./Roster.svelte";
export { default as Seen } from "./Seen.svelte";
export { WorkingFeed } from "./working-feed.svelte";
export { CardSync } from "./card-sync";

/** A card's markers, its working agents named as its participants name them. */
export const cardMarkers = (a: Artifact, att: AttentionSummary | undefined, working: Working[]) =>
  markers(a, att, chips(working, agentNames(working, a.participants?.agents ?? [])));
