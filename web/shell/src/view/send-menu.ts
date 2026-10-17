// The Send menu's groups (spec 2026-10-08-multi-agent-multi-person §10):
// every live agent that may receive the thread, by name, "Your agents"
// first, then the owner's, then each other person's ("Sam's agents") in
// the order their agents are listed; an agent whose person must approve the thread's
// authors first says so ("Sam approves first").
import type { Viewer } from "../threads";
import { shownName } from "./agent-display";
import type { AgentView } from "./working-model";

/** Who approves a send of threads `threads` of artifact `aid` first (null:
 * the owner), as the daemon answers this viewer; null for no answer. */
export type AskFirsts = (aid: string, threads: string[]) => Promise<(string | null)[] | null>;
/** How the Send menu asks: unset, it fetches the route from the page's own
 * origin; a host whose origin is not the daemon's (the extension's panel)
 * sets its own. */
export const firstsVia: { ask: AskFirsts | null } = { ask: null };

/** `person`: "Sam's agent" for another person's agent, else null. */
export type SendItem = { handle: string; name: string; person: string | null; first: string | null };
export type SendGroup = { label: string; mine: boolean; agents: SendItem[] };

/** The live `agents` in groups, each by its name (else `names`', else its
 * harness). `me` is this viewer's public ID (an
 * agent whose person has it is "yours", as is one without a person, from a
 * daemon before people, as `ownTarget` takes it); `undefined` when
 * unknown, where the owner's agents are taken as yours. `firsts` is the
 * thread's `approves_first` (null: the owner). */
export function sendGroups(agents: AgentView[], me: string | null | undefined, firsts: (string | null)[] = [], names?: Map<string, string>): SendGroup[] {
  const groups = new Map<string, SendGroup & { owner: boolean }>();
  for (const a of agents.filter(x => x.live)) {
    const p = a.person;
    const owners = !p || !!p.owner;
    const mine = !p || (me === undefined ? owners : p.public_id === me);
    const key = mine ? "" : p?.public_id ?? "?";
    const who = p?.name ?? "Another person";
    const g = groups.get(key) ?? { label: mine ? "Your agents" : `${who}'s agents`, mine, owner: owners, agents: [] };
    groups.set(key, g);
    const asks = firsts.includes(owners ? null : p.public_id);
    g.agents.push({ handle: a.handle, name: a.name ? shownName(a.name, a.via) : names?.get(a.handle) ?? a.harness, person: mine ? null : `${who}'s agent`, first: asks ? (mine ? "You approve first" : `${who} approves first`) : null });
  }
  // Yours first, then the owner's, then each other person's in the daemon's order.
  const rank = (g: { mine: boolean; owner: boolean }) => (g.mine ? 0 : g.owner ? 1 : 2);
  return [...groups.values()].sort((a, b) => rank(a) - rank(b)).map(g => ({ label: g.label, mine: g.mine, agents: g.agents }));
}

/** Whose agents a Send from viewer `v` takes as its own, as `ownTarget`'s
 * `me`: the viewer's for the owner's browser and a connected person's; for
 * a viewer who is neither (a LAN viewer), the owner's, which that viewer's
 * comments reach without approval (spec 2026-10-08 §10, O6), so one press
 * sends to the owner's newest live agent as on a single-owner install;
 * `undefined` while the viewer is unknown. */
export function sendsFor(agents: AgentView[], v: Viewer | null | undefined): string | null | undefined {
  if (!v) return undefined;
  if (v.owner || v.person) return v.public_id;
  const owner = agents.find(a => a.person?.owner)?.person;
  return owner ? owner.public_id : v.public_id;
}

/** The Send target where this viewer chose none: their own most recently
 * active live agent (the daemon's order), else none (spec 2026-10-08 §10:
 * no one-press send to another person's agent). An agent without a person
 * (a daemon from before people) is taken as the viewer's; with `me`
 * unknown, no agent with a person is. */
export function ownTarget(agents: AgentView[], me: string | null | undefined): string | null {
  return agents.find(a => a.live && (!a.person || a.person.public_id === me))?.handle ?? null;
}
