// The agent a Send goes to as this viewer chose it (spec §10,
// "Participants and attention"): the one they last chose or sent to on the
// artifact, while it is live; else none. Remembered per browser; storage
// may throw. Where there is none, the artifact view's sidebar sends to the
// viewer's own most recent live agent (`ownTarget`, view/send-menu.ts):
// never, by default, another person's (spec 2026-10-08 §10).
import type { AgentView } from "./working-model";

const key = (aid: string) => `clax.sendTo.${aid}`;
export const liveAgents = (agents: AgentView[]) => agents.filter(a => a.live);

export function rememberTarget(aid: string, handle: string): void {
  try { localStorage.setItem(key(aid), handle); } catch { /* this page only */ }
}

export function defaultTarget(aid: string, agents: AgentView[]): string | null {
  let last: string | null = null;
  try { last = localStorage.getItem(key(aid)); } catch { /* none */ }
  return last && liveAgents(agents).some(a => a.handle === last) ? last : null;
}
