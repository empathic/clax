import { whose } from "./working-model";
import { type AnchorResult, INDEX_FILE } from "../../../bridge/src/protocol";
import type { Comment, Thread, Viewer } from "../threads";
import { hasElapsedLabel } from "../waiting";
import { agentName } from "./history-model";

/** How long a thread's card must stay at least half in view before the viewer has looked at it. */
export const SEEN_AFTER_MS = 1000;

export type SidebarSections = { open: Thread[]; detached: Thread[]; resolved: Thread[]; numbers: Map<string, number>; file: string | null };

/** When a thread last changed: its newest comment, its start, or its resolve, whichever is latest. */
export function lastActivity(t: Thread): string {
  let at = t.created_at ?? "";
  for (const c of t.comments ?? []) if (c.created_at > at) at = c.created_at;
  if (t.resolved_at && t.resolved_at > at) at = t.resolved_at;
  return at;
}

/** `list` with the latest activity first; threads that changed at the same time keep their order. */
const latestFirst = (list: Thread[]) => [...list].sort((a, b) => (lastActivity(b) > lastActivity(a) ? 1 : lastActivity(b) < lastActivity(a) ? -1 : 0));

/** Open threads: those about the whole artifact (no pin, no number), those
 * on the page shown and found (numbered like the pins, in the order they
 * were made), and those on other pages the version holds; Detached: open
 * threads on the page shown and not found, and those on a page the version
 * does not hold (never one about the whole artifact, which is on no page);
 * then resolved threads. Each section lists the thread that changed last
 * first. `file` undefined means the index; null, a document that did not
 * greet. */
export function sidebarSections(threads: Thread[], resolved: Record<string, AnchorResult>, file: string | null | undefined, holds: (f: string) => boolean = () => true): SidebarSections {
  const page = file === undefined ? INDEX_FILE : file;
  const whole = threads.filter(t => t.status === "open" && t.anchor.kind === "artifact");
  const open = threads.filter(t => t.status === "open" && !whole.includes(t));
  const here = open.filter(t => t.anchor.file === page);
  const gone = open.filter(t => !holds(t.anchor.file));
  const detached = [...here.filter(t => resolved[t.id] && !resolved[t.id].found), ...gone.filter(t => !here.includes(t))];
  const attached = here.filter(t => !detached.includes(t));
  const elsewhere = open.filter(t => t.anchor.file !== page && !gone.includes(t));
  return {
    open: latestFirst([...whole, ...attached, ...elsewhere]),
    detached: latestFirst(detached),
    resolved: latestFirst(threads.filter(t => t.status === "resolved")),
    numbers: new Map(attached.map((t, i) => [t.id, i + 1])),
    file: page,
  };
}

/** Whether a label shows elapsed time, so the sidebar's clock must tick. */
export function needsTicking(threads: Thread[]): boolean {
  return threads.some(t => t.status === "open" && t.sent_to_agent && hasElapsedLabel(t.feedback_state));
}

/** A comment's author line (spec 2026-10-08 §11 "Display"): an agent's
 * reply as `<name> (agent, via <harness>)` when the agent's person is the
 * viewer (`person` null: the owner's agent, for the owner), else `<name> (<person>'s
 * agent, via <harness>)`; a reply from before agent names by its harness;
 * a person's comment by their name. Text, never markup. */
export function authorLabel(c: Comment, me?: Viewer | null): string {
  const a = c.agent, via = agentName(c.via_harness);
  return c.author_kind !== "agent" ? c.author_name : a ? `${a.name} (${(a.person === null ? me?.owner : a.person === me?.public_id) ? "" : `${whose(a.person_name)} `}agent, via ${via})` : via;
}

/** A page as people read it: percent-encoding decoded, or the page as
 * written when it is not valid percent-encoding. */
export function pageLabel(page: string): string {
  try { return decodeURIComponent(page); } catch { return page; }
}
