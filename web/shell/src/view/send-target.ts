// Which agent a Send goes to (spec §10, "Participants and attention"): the
// one this viewer last sent to on the artifact while it is live, else the
// most recently active live agent (the daemon's list order), else none, and
// the send goes without `to`. Remembered per browser; storage may throw.
import type { AgentView } from "./working-model";

const key = (aid: string) => `clax.sendTo.${aid}`;
export const liveAgents = (agents: AgentView[]) => agents.filter(a => a.live);

export function rememberTarget(aid: string, handle: string): void {
  try { localStorage.setItem(key(aid), handle); } catch { /* this page only */ }
}

export function defaultTarget(aid: string, agents: AgentView[]): string | null {
  const live = liveAgents(agents);
  let last: string | null = null;
  try { last = localStorage.getItem(key(aid)); } catch { /* none */ }
  if (last && live.some(a => a.handle === last)) return last;
  return live[0]?.handle ?? null;
}
