// The working signal as Echo shows it (spec §8, "Working"): pure functions
// over the daemon's working views, shared by the top bar, the gallery, the
// sidebar, the pins and the capability.
import type { Participants } from "../api";
import { shownName } from "./agent-display";

export type Working = { key: string; agent: string; harness: string; name?: string | null; message: string | null; thread_ids: string[]; started_at: string; last_heartbeat: string };
export type AgentView = Participants["agents"][number];
export type SummaryInput = { working: Working[]; names: Map<string, string>; mine: Set<string>; open: number; idle: string[]; addressed: string | null; published: number | null; rally: boolean; now: Date; live?: boolean };
export type Summary = { line1: string; agent: boolean; line2: string; elapsed: string | null };

const LABELS: Record<string, string> = { claude: "Claude Code", codex: "Codex", pi: "Pi", app: "Clax app" };
/** The harness as a product name ("Claude Code"), for the page capability. */
export const harnessLabel = (h: string): string => LABELS[h] ?? h;

/** The name the shell shows for an agent of harness `h`: the harness itself
 * ("claude"), except the Clax app's agent (harness `app`), shown "Clax app". */
export const harnessName = (h: string): string => (h === "app" ? LABELS.app : h);

export function newestFirst(list: Working[]): Working[] {
  return [...list].sort((a, b) => b.started_at.localeCompare(a.started_at) || b.key.localeCompare(a.key));
}

/** Handle → name: its agent name (spec 2026-10-08 §11), else the harness, with the handle's first four hex digits when two agents share it. */
export function agentNames(list: Working[], agents: AgentView[]): Map<string, string> {
  const all = new Map<string, string>(agents.map(a => [a.handle, shownName(a.name || harnessName(a.harness), a.via)]));
  for (const w of list) if (!all.has(w.agent)) all.set(w.agent, w.name || harnessName(w.harness));
  const count = new Map<string, number>();
  for (const h of all.values()) count.set(h, (count.get(h) ?? 0) + 1);
  return new Map([...all].map(([handle, h]) => [handle, (count.get(h) ?? 0) > 1 ? `${h} ${handle.slice(2, 6)}` : h]));
}

export function clock(since: string, now: Date): string {
  const s = Math.max(0, Math.floor((now.getTime() - new Date(since).getTime()) / 1000));
  const two = (n: number) => String(n).padStart(2, "0");
  return s >= 3600 ? `${Math.floor(s / 3600)}:${two(Math.floor(s / 60) % 60)}:${two(s % 60)}` : `${Math.floor(s / 60)}:${two(s % 60)}`;
}

export const workingThreads = (list: Working[]) => new Set(list.flatMap(w => w.thread_ids));
const plural = (n: number, w: string) => `${n} ${w}${n === 1 ? "" : "s"}`;

/** `names` for a one-line summary: the first two, else the first and how many more. */
export function fewNames(names: string[]): string {
  return names.length <= 2 ? names.join(", ") : `${names[0]} and ${names.length - 1} more`;
}

export function summary(i: SummaryInput): Summary {
  // Past two, the count alone: a long name would hide it behind the ellipsis.
  const idle = !i.idle.length ? "" : i.idle.length <= 2 ? ` · ${i.idle.join(", ")} idle` : ` · ${i.idle.length} agents idle`;
  const list = newestFirst(i.working);
  if (list.length) {
    const who = fewNames([...new Set(list.map(w => i.names.get(w.agent) ?? harnessName(w.harness)))]);
    const threads = workingThreads(list);
    const line1 = list.length === 1 && list[0].message ? `${who}: ${list[0].message}` : threads.size ? `${who} working on ${threads.size}` : `${who} working`;
    const mine = [...threads].filter(t => i.mine.has(t)).length;
    const line2 = (mine && mine === threads.size ? "all yours" : mine ? `${mine} yours` : plural(i.open, "open thread")) + idle;
    return { line1, agent: true, line2, elapsed: clock(list[0].started_at, i.now) };
  }
  if (i.published !== null) return { line1: `v${i.published} ${i.live ? "snapshot taken" : "published"}`, agent: false, line2: "reload to see it", elapsed: null };
  if (i.addressed) return { line1: i.addressed, agent: false, line2: "yours, not looked at yet", elapsed: null };
  return { line1: "Nobody working", agent: false, line2: plural(i.open, "open thread") + idle + (i.rally ? " · rally of 10" : ""), elapsed: null };
}

export function threadMarker(list: Working[], threadId: string, names: Map<string, string>): { text: string; since: string } | null {
  const w = newestFirst(list).find(x => x.thread_ids.includes(threadId));
  return w ? { text: `${names.get(w.agent) ?? harnessName(w.harness)} is working on it`, since: w.started_at } : null;
}

export function threadAgent(list: Working[], threadId: string, names: Map<string, string>): string | null {
  const w = newestFirst(list).find(x => x.thread_ids.includes(threadId));
  return w ? names.get(w.agent) ?? harnessName(w.harness) : null;
}

export function stripText(w: Working, names: Map<string, string>, numbers: Map<string, number>, mine: Set<string>): string {
  const who = names.get(w.agent) ?? harnessName(w.harness);
  if (w.message) return `${who}: ${w.message}`;
  if (!w.thread_ids.length) return `${who} is working`;
  const numbered = w.thread_ids.filter(t => numbers.has(t));
  if (numbered.length !== w.thread_ids.length || numbered.length > 3) return `${who} is working on ${plural(w.thread_ids.length, "thread")}`;
  const parts = numbered.map(t => `#${numbers.get(t)}${mine.has(t) ? " (yours)" : ""}`);
  return `${who} is working on ${parts.length > 1 ? `${parts.slice(0, -1).join(", ")} and ${parts.at(-1)}` : parts[0]}`;
}

export function chips(list: Working[], names: Map<string, string>): string[] {
  return newestFirst(list).map(w => (w.thread_ids.length ? `${names.get(w.agent) ?? harnessName(w.harness)} working on ${w.thread_ids.length}` : `${names.get(w.agent) ?? harnessName(w.harness)} working`));
}

/** An agent's person's name mid-sentence: the daemon names an unnamed owner "The
 * owner", which reads "the owner" there. */
export const midName = (name: string): string => (name === "The owner" ? "the owner" : name);
/** A person's name as a possessive mid-sentence: "Sam's", "the owner's". */
export const whose = (name: string): string => `${midName(name)}'s`;
