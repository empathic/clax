// A token's queue access as Settings' Queues control shows and sets it
// (docs/contract.md, "Rights": `clax-queues`, zero by default and granted
// on a token): which queues (every queue the granter holds, or some by ID)
// and which actions there. Each action includes Read, so Read cannot be
// left out while another is on, and turning Read off turns every action
// off. Nothing chosen is no grant: the control removes it.

/** The actions on a report queue, in the order the control lists them. */
export const QUEUE_ACTIONS = ["read", "claim", "reply", "fix", "resolve"] as const;
export type QueueAction = (typeof QUEUE_ACTIONS)[number];

/** What each action lets the token's agent do, as the control labels it. */
export const ACTION_LABELS: Record<QueueAction, string> = {
  read: "Read",
  claim: "Claim",
  reply: "Reply",
  fix: "Fix",
  resolve: "Resolve",
};
export const ACTION_HINTS: Record<QueueAction, string> = {
  read: "List and read reports, their screenshots and diagnostics, and be woken by new ones",
  claim: "Claim a report, renew and release the claim",
  reply: "Reply to the reporter",
  fix: "Propose a fix",
  resolve: "Resolve a report",
};

/** One `clax-queues` access object. */
export type QueueAccessObject = { type: "clax-queues"; actions: string[]; locations?: string[] };

/** What the control holds: every queue the granter holds (`all`), or the
 * queues named, and the actions there. */
export type QueueChoice = { all: boolean; queues: string[]; actions: QueueAction[] };

export const NO_ACCESS: QueueChoice = { all: false, queues: [], actions: [] };

const isAction = (a: string): a is QueueAction => (QUEUE_ACTIONS as readonly string[]).includes(a);

/** The choice an access array (a granter's grant on a token) reads as: the
 * union of its `clax-queues` objects' actions and locations. */
export function fromAccess(access: unknown): QueueChoice {
  if (!Array.isArray(access)) return NO_ACCESS;
  const actions = new Set<QueueAction>();
  const queues = new Set<string>();
  let all = false;
  for (const o of access as QueueAccessObject[]) {
    if (o?.type !== "clax-queues") continue;
    for (const a of o.actions ?? []) if (isAction(a)) actions.add(a);
    for (const l of o.locations ?? ["*"]) {
      if (l === "*") all = true;
      else queues.add(l);
    }
  }
  if (actions.size) actions.add("read");
  return { all, queues: all ? [] : [...queues].sort(), actions: QUEUE_ACTIONS.filter(a => actions.has(a)) };
}

/** The grant a choice sends, or null for none (no queue, or no action). */
export function toAccess(c: QueueChoice): QueueAccessObject[] | null {
  if (!c.actions.length || (!c.all && !c.queues.length)) return null;
  return [{ type: "clax-queues", actions: [...c.actions], locations: c.all ? ["*"] : [...c.queues] }];
}

/** `c` with action `a` turned `on` or off: on adds Read; Read off clears every action. */
export function withAction(c: QueueChoice, a: QueueAction, on: boolean): QueueChoice {
  const set = new Set(c.actions);
  if (on) { set.add(a); set.add("read"); }
  else if (a === "read") set.clear();
  else set.delete(a);
  return { ...c, actions: QUEUE_ACTIONS.filter(x => set.has(x)) };
}

/** `c` with queue `id` chosen or not (`"*"`: every queue the granter holds). */
export function withQueue(c: QueueChoice, id: string, on: boolean): QueueChoice {
  if (id === "*") return { ...c, all: on, queues: on ? [] : c.queues };
  const set = new Set(c.queues);
  if (on) set.add(id);
  else set.delete(id);
  return { ...c, all: false, queues: [...set].sort() };
}

/** One line for a choice: `No queue access`, or the actions and where. */
export function summary(c: QueueChoice, names: Record<string, string>): string {
  if (!toAccess(c)) return "No queue access";
  const what = c.actions.map(a => ACTION_LABELS[a]).join(", ");
  const where = c.all ? "all my queues" : c.queues.map(q => names[q] ?? q).join(", ");
  return `${what} on ${where}`;
}

/** Whether two choices grant the same. */
export function sameChoice(a: QueueChoice, b: QueueChoice): boolean {
  return JSON.stringify(toAccess(a)) === JSON.stringify(toAccess(b));
}
