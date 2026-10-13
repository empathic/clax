// Agents as the top bar names them (spec 2026-10-08-multi-agent-multi-person
// §11 "Display"): by name, and, where more than one person's agents
// appear, another person's with that person: "sam-notes (Sam's)".
import type { AgentView } from "./working-model";

/** `names` (handle → name) with another person's agents named with their
 * person, when `agents` hold more than one person's; `me` is this viewer's
 * public ID. A new map; `names` is left as it is. */
export function withPeople(names: Map<string, string>, agents: AgentView[], me: string | null | undefined): Map<string, string> {
  const out = new Map(names);
  if (new Set(agents.map(a => a.person?.public_id)).size < 2) return out;
  for (const a of agents) if (a.person && a.person.public_id !== me) out.set(a.handle, `${out.get(a.handle) ?? a.name ?? a.harness} (${a.person.name}'s)`);
  return out;
}
