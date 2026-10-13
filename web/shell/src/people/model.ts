// What People shows and sends, without the page (spec
// 2026-10-08-multi-agent-multi-person §7): the capabilities a narrowing
// turns off and an elevation adds, the presets (§7.3), and the daemon's
// comparison of rights (`Rights::is_narrower_or_equal`), so a narrowing
// built here is one the daemon takes as narrower.
import type { Access, PeopleAgent, PeopleGrant, PeopleInvite, Rights } from "../api";

/** The line beside per-agent controls: what narrowing one agent of a
 * connected machine does and does not bound. */
export const MACHINE_BOUNDARY = "Agents on the same machine share its access: the machine is the boundary.";

/** One action of one type, as People offers it. */
export type Cap = { key: string; type: string; action: string; label: string };
const cap = (type: string, action: string, label: string): Cap => ({ key: `${type} ${action}`, type, action, label });
/** Every action an agent may be given, in the order People lists them. */
export const CAPS: Cap[] = [
  cap("clax-artifacts", "read", "Read artifacts"),
  cap("clax-artifacts", "create", "Publish new artifacts"),
  cap("clax-artifacts", "update", "Publish new versions"),
  cap("clax-artifacts", "delete", "Delete artifacts"),
  cap("clax-comments", "read", "Read comments"),
  cap("clax-comments", "write", "Comment"),
  cap("clax-comments", "reply", "Reply"),
  cap("clax-comments", "resolve", "Resolve threads"),
  cap("clax-comments", "send", "Send threads to agents"),
  cap("clax-comments", "watch", "Watch for comments"),
  cap("clax-db", "read", "Read page data"),
  cap("clax-db", "write", "Write page data"),
  cap("clax-ask", "ask", "Ask questions"),
];

const A = (type: string, actions: string[], locations?: string[], level?: string): Access => ({ type, actions, ...locations && { locations }, ...level && { level } });
const READER: Rights = [
  A("clax-artifacts", ["read"], ["*"]), A("clax-comments", ["read", "reply", "watch"], ["*"]),
  A("clax-db", ["read"], ["*"], "view"), A("clax-ask", ["ask"]),
];
/** The presets (§7.3), by name; a person's rights are set by name. */
export const PRESETS: Record<string, Rights> = {
  "clax:reader": READER,
  "clax:collaborator": [
    ...READER, A("clax-artifacts", ["create"], ["*"]), A("clax-artifacts", ["update", "delete"], ["own"]),
    A("clax-comments", ["write", "resolve", "send"], ["*"]),
    A("clax-db", ["read", "write"], ["*"], "interact"), A("clax-db", ["read", "write"], ["own"], "admin"),
  ],
  "clax:full": [
    A("clax-artifacts", ["read", "create", "update", "delete"], ["*"]),
    A("clax-comments", ["read", "write", "reply", "resolve", "send", "watch"], ["*"]),
    A("clax-db", ["read", "write"], ["*"], "admin"), A("clax-ask", ["ask"]),
  ],
};
export const PRESET_NAMES: Record<string, string> = { "clax:reader": "Reader", "clax:collaborator": "Collaborator", "clax:full": "Full" };

const LEVEL: Record<string, number> = { view: 1, interact: 2, admin: 3 };
const rank = (l?: string) => (l ? LEVEL[l] ?? 0 : 0);
/** Whether location `l` lies within `m` wherever `l` applies. */
const within = (l: string, m: string) => m === "*" || l === m || (l.startsWith("own:") && (m === "own" || m === l.slice(4)));

/** Whether `b` allows everything `a` does, wherever `a` does, at least at
 * `a`'s db level: the daemon's test of a narrowing. */
export function narrowerOrEqual(a: Rights, b: Rights): boolean {
  return a.every(x => (x.locations ?? ["*"]).every(l => x.actions.every(act =>
    b.some(y => y.type === x.type && y.actions.includes(act) && rank(y.level) >= rank(x.level) && (y.locations ?? ["*"]).some(m => within(l, m))))));
}

/** The preset `r` is, or null for rights of their own. */
export const presetOf = (r: Rights): string | null =>
  Object.keys(PRESETS).find(n => narrowerOrEqual(r, PRESETS[n]) && narrowerOrEqual(PRESETS[n], r)) ?? null;

const allows = (r: Rights, c: Cap) => r.some(a => a.type === c.type && a.actions.includes(c.action));
/** The capabilities `ceiling` allows that `r` (null: no narrowing) does not. */
const offIn = (ceiling: Rights, r: Rights | null) => new Set(r ? CAPS.filter(c => allows(ceiling, c) && !allows(r, c)).map(c => c.key) : []);
/** `ceiling` without the actions `off` names: a narrowing of it. */
const without = (ceiling: Rights, off: Set<string>): Rights =>
  ceiling.map(a => ({ ...a, actions: a.actions.filter(x => !off.has(`${a.type} ${x}`)) })).filter(a => a.actions.length > 0);
const toggled = (off: Set<string>, key: string, on: boolean) => { const s = new Set(off); if (on) s.delete(key); else s.add(key); return s; };

export type CapRow = Cap & { on: boolean };

/** Machine `g`'s capabilities within its person's rights `person`. */
export const machineCaps = (person: Rights, g: PeopleGrant): CapRow[] => {
  const off = offIn(person, g.narrow);
  return CAPS.filter(c => allows(person, c)).map(c => ({ ...c, on: !off.has(c.key) }));
};

/** The narrowing that turns capability `key` of machine `g` on or off;
 * null when nothing is off (no narrowing). */
export function narrowGrant(person: Rights, g: PeopleGrant, key: string, on: boolean): Rights | null {
  const off = toggled(offIn(person, g.narrow), key, on);
  return off.size ? without(person, off) : null;
}

/** Agent `a`'s capabilities on machine `g`; `machineOff` for those the
 * machine's narrowing turns off, which the agent's cannot turn on. */
export const agentCaps = (person: Rights, g: PeopleGrant | null, a: PeopleAgent): (CapRow & { machineOff: boolean })[] => {
  const machine = offIn(person, g?.narrow ?? null);
  const own = offIn(person, a.narrow);
  return CAPS.filter(c => allows(person, c)).map(c => ({ ...c, on: !machine.has(c.key) && !own.has(c.key), machineOff: machine.has(c.key) }));
};

/** The narrowing that turns capability `key` of agent `a` on or off: its
 * person's rights less what its machine and it have off, so it lies within
 * both; null when the agent has nothing off beyond its machine. */
export function narrowAgent(person: Rights, g: PeopleGrant | null, a: PeopleAgent, key: string, on: boolean): Rights | null {
  const machine = offIn(person, g?.narrow ?? null);
  const own = toggled(offIn(person, a.narrow), key, on);
  if ([...own].every(k => machine.has(k))) return null;
  return without(person, new Set([...machine, ...own]));
}

/** Every capability for the owner's raising of agent `a`: `has` when its
 * person's rights within its narrowings give it already, `on` when it has
 * it or it is raised. */
export const raiseCaps = (person: Rights, g: PeopleGrant | null, a: PeopleAgent): (CapRow & { has: boolean })[] => {
  const base = new Set(agentCaps(person, g, a).filter(r => r.on).map(r => r.key));
  return CAPS.map(c => ({ ...c, has: base.has(c.key), on: base.has(c.key) || (!!a.elevate && allows(a.elevate, c)) }));
};

/** The elevation that raises agent `a` with capability `key` or drops it,
 * at every artifact; null when nothing is raised. */
export function elevation(person: Rights, g: PeopleGrant | null, a: PeopleAgent, key: string, on: boolean): Rights | null {
  const raised = new Set(raiseCaps(person, g, a).filter(r => r.on && !r.has).map(r => r.key));
  if (on) raised.add(key); else raised.delete(key);
  const out: Rights = [];
  for (const c of CAPS.filter(x => raised.has(x.key))) {
    const same = out.find(x => x.type === c.type);
    if (same) same.actions.push(c.action);
    else out.push(A(c.type, [c.action], c.type === "clax-ask" ? undefined : ["*"]));
  }
  for (const x of out) if (x.type === "clax-db" && x.actions.includes("write")) x.level = "interact";
  return out.length ? out : null;
}

/** The invites that can still be used. */
export const liveInvites = (all: PeopleInvite[], now: Date) =>
  all.filter(i => !i.used_at && !i.revoked_at && Date.parse(i.expires_at) > now.getTime());

/** How long until `at`: "24 h", "40 min", "now". */
export function expiresIn(at: string, now: Date): string {
  const min = Math.round((Date.parse(at) - now.getTime()) / 60_000);
  if (min <= 0) return "now";
  return min >= 60 ? `${Math.round(min / 60)} h` : `${min} min`;
}
