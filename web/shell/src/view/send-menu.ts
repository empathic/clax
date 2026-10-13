// The Send menu's groups (spec 2026-10-08-multi-agent-multi-person §10):
// every live agent that may receive the thread, by name, "Your agents"
// first, then each other person's ("Sam's agents") in the order their
// agents are listed; an agent whose person must approve the thread's
// authors first says so ("Sam approves first").
import type { AgentView } from "./working-model";

export type SendItem = { handle: string; name: string; first: string | null };
export type SendGroup = { label: string; mine: boolean; agents: SendItem[] };

/** The live `agents` in groups, each by its name (else `names`', else its
 * harness). `me` is this viewer's public ID (an
 * agent whose person has it is "yours"); `undefined` when unknown, where
 * the owner's agents are taken as yours (the extension's panel is the
 * owner's). `firsts` is the thread's `approves_first` (null: the owner). */
export function sendGroups(agents: AgentView[], me: string | null | undefined, firsts: (string | null)[] = [], names?: Map<string, string>): SendGroup[] {
  const groups = new Map<string, SendGroup>();
  for (const a of agents.filter(x => x.live)) {
    const p = a.person;
    const owners = !p || !!p.owner;
    const mine = me === undefined ? owners : !!p && p.public_id === me;
    const key = mine ? "" : p?.public_id ?? "?";
    const who = p?.name ?? "Another person";
    const g = groups.get(key) ?? { label: mine ? "Your agents" : `${who}'s agents`, mine, agents: [] };
    groups.set(key, g);
    const asks = firsts.includes(owners ? null : p.public_id);
    g.agents.push({ handle: a.handle, name: a.name ?? names?.get(a.handle) ?? a.harness, first: asks ? (mine ? "You approve first" : `${who} approves first`) : null });
  }
  return [...groups.values()].sort((a, b) => Number(b.mine) - Number(a.mine));
}
