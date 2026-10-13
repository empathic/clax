// The inbox's agent strip (spec 2026-10-08-multi-agent-multi-person §13):
// one chip per agent with unread items for you or a block on you,
// blocked-longest first, each saying how long it has waited on you.
import type { StripAgent } from "../api";

/** The agents blocked on you, longest first, then the rest in the order given. */
export function stripOrder(agents: StripAgent[]): StripAgent[] {
  const blocked = agents.filter(a => a.blocked).sort((a, b) => a.blocked!.since.localeCompare(b.blocked!.since));
  return [...blocked, ...agents.filter(a => !a.blocked)];
}

/** How long since `since`, at `now`: "under a minute", "12 min", "1 h 17 min", "2 h". */
export function waitedFor(since: string, now: Date): string {
  const m = Math.floor((now.getTime() - new Date(since).getTime()) / 60_000);
  if (m < 1) return "under a minute";
  if (m < 60) return `${m} min`;
  return m % 60 ? `${Math.floor(m / 60)} h ${m % 60} min` : `${m / 60} h`;
}

const plural = (n: number, w: string) => `${n} ${w}${n === 1 ? "" : "s"}`;

/** What a blocked agent waits on: "1 question", "2 approvals", "1 question, 1 approval". */
export function blockedOn(b: NonNullable<StripAgent["blocked"]>): string {
  return [b.questions ? plural(b.questions, "question") : "", b.approvals ? plural(b.approvals, "approval") : ""].filter(Boolean).join(", ");
}
