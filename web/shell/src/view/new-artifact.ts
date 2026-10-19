// The gallery's New (docs/contract.md "Starting an artifact from a brief"):
// a person writes a title and a brief, the daemon makes the artifact with a
// placeholder page and the brief as its first thread, and the person picks a
// live agent to build it, which the brief is sent to (the send gives the
// agent its watch). With no live agent, a prompt to paste into one. Loaded
// with the dialog, on the first press of New.
import { getToken } from "../api";

/** The longest title the daemon takes, in characters. */
export const MAX_TITLE = 200;
/** The longest brief the daemon takes (a comment's limit), in characters. */
export const MAX_BRIEF = 10_000;

/** What `POST /api/artifacts:new` answers, as the dialog uses it. */
export type Started = { artifact: { id: string; title: string }; url: string; thread: { id: string } };

/** One agent as `GET /api/agents` lists it: another person's agent has no
 * handle there, only its name. `person` is its person's public ID; null for
 * the owner's agents. */
export type ListedAgent = { handle: string | null; name: string | null; person: string | null; live: boolean; harness?: string | null };

/** An agent the dialog offers: `to` is what the send names (its handle, else
 * its name); `who` is "Sam's agent" for another person's, else null. */
export type Pick = { to: string; name: string; who: string | null };
export type PickGroup = { label: string; mine: boolean; agents: Pick[] };

/** Who is asking: this browser's viewer as `GET /api/viewers/me` gives it. */
export type Me = { public_id?: string; owner?: boolean } | null | undefined;

/** Why the form cannot be sent yet, or null: a title and a brief, within
 * the daemon's limits (it checks them again). */
export function formProblem(title: string, brief: string): string | null {
  const t = title.trim();
  if (!t) return "Give it a title.";
  if ([...t].length > MAX_TITLE) return `A title is at most ${MAX_TITLE} characters.`;
  if (/[\p{Cc}\p{Cf}]/u.test(t)) return "A title is one line of plain text.";
  if (!brief.trim()) return "Say what you want built.";
  if ([...brief.trim()].length > MAX_BRIEF) return `A brief is at most ${MAX_BRIEF.toLocaleString("en-US")} characters.`;
  return null;
}

/** The live agents, grouped as the Send menu groups them: yours first, then
 * the owner's, then each other person's, by `names` (public ID → display
 * name). An agent that has neither a handle nor a name cannot be named, and
 * is left out. */
export function pickGroups(listed: ListedAgent[], me: Me, names: Map<string, string> = new Map()): PickGroup[] {
  const groups = new Map<string, PickGroup & { rank: number }>();
  for (const a of listed) {
    if (!a.live) continue;
    const to = a.handle ?? a.name;
    if (!to) continue;
    const owners = a.person === null;
    // The owner's agents are the owner's own; a person's, theirs.
    const mine = owners ? !!me?.owner : !!me?.public_id && a.person === me.public_id;
    const key = mine ? "" : owners ? "owner" : a.person!;
    const whose = owners ? "The owner" : names.get(a.person!) ?? "Another person";
    const g = groups.get(key) ?? { label: mine ? "Your agents" : `${whose}'s agents`, mine, rank: mine ? 0 : owners ? 1 : 2, agents: [] };
    groups.set(key, g);
    g.agents.push({ to, name: a.name ?? a.harness ?? to, who: mine ? null : `${whose}'s agent` });
  }
  return [...groups.values()].sort((a, b) => a.rank - b.rank).map(({ label, mine, agents }) => ({ label, mine, agents }));
}

/** The prompt to paste into an agent (Claude Code with the Clax plugin, or
 * any harness Clax is set up in) so it watches the artifact at `url` and
 * builds what its brief asks. */
export function connectPrompt(url: string, title: string): string {
  return `Build the Clax artifact "${title}" at ${url}. Watch it with the clax watch tool, read its first comment thread (the brief) with comments_read, publish your page as a new version of that artifact, and reply on the brief thread when it is done.`;
}

/** The headers of a write from this browser: the owner's shell sends its
 * token; anyone else's request is their browser's (its cookie). */
async function headers(): Promise<Record<string, string>> {
  const h: Record<string, string> = { "content-type": "application/json" };
  const t = await getToken();
  if (t) h.authorization = `Bearer ${t}`;
  return h;
}

/** The daemon's reason for a refusal, else the status. */
async function reason(res: Response): Promise<string> {
  const b = await res.json().catch(() => null) as { error?: { message?: string } } | null;
  return b?.error?.message ?? (res.statusText || `HTTP ${res.status}`);
}

/** Starts an artifact titled `title` from `brief`. Throws with the
 * daemon's reason when it refuses. */
export async function start(title: string, brief: string): Promise<Started> {
  const res = await fetch("/api/artifacts:new", { method: "POST", headers: await headers(), body: JSON.stringify({ title: title.trim(), brief: brief.trim() }) });
  if (!res.ok) throw new Error(await reason(res));
  return res.json() as Promise<Started>;
}

/** Every live agent this browser may see (`GET /api/agents`); none when it
 * may not list them. */
export async function liveAgents(): Promise<ListedAgent[]> {
  const t = await getToken();
  const res = await fetch("/api/agents", { headers: t ? { authorization: `Bearer ${t}` } : {}, cache: "no-store" }).catch(() => null);
  if (!res?.ok) return [];
  const b = await res.json().catch(() => null) as { agents?: ListedAgent[] } | null;
  return (b?.agents ?? []).filter(a => a.live);
}

/** The display names of the people `ids` name (`GET /api/viewers?ids=`). */
export async function personNames(ids: string[]): Promise<Map<string, string>> {
  const out = new Map<string, string>();
  const want = [...new Set(ids)].slice(0, 64);
  if (!want.length) return out;
  const res = await fetch(`/api/viewers?ids=${want.map(encodeURIComponent).join(",")}`).catch(() => null);
  if (!res?.ok) return out;
  const b = await res.json().catch(() => null) as { viewers?: { id: string; display_name: string | null }[] } | null;
  for (const v of b?.viewers ?? []) if (v.display_name) out.set(v.id, v.display_name);
  return out;
}

/** What a send of the brief came to: delivered to the agent's queue, or
 * held until the agent's person approves its author. */
export type Sent = { waiting: boolean };

/** Sends the brief thread `tid` of artifact `aid` to the agent `to` names,
 * which then watches the artifact. Throws with the daemon's reason. */
export async function sendBrief(aid: string, tid: string, to: string): Promise<Sent> {
  const res = await fetch(`/api/artifacts/${encodeURIComponent(aid)}/threads/${encodeURIComponent(tid)}/send`, { method: "POST", headers: await headers(), body: JSON.stringify({ to }) });
  if (!res.ok) throw new Error(await reason(res));
  const b = await res.json().catch(() => null) as { thread?: { feedback_state?: { state?: string } | string | null } } | null;
  const fs = b?.thread?.feedback_state;
  const state = typeof fs === "string" ? fs : fs?.state;
  return { waiting: state === "withheld" };
}
