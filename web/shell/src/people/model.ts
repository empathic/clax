// What People shows and sends, without the page (spec
// 2026-10-08-multi-agent-multi-person §7): the capabilities a narrowing
// turns off and an elevation adds, the presets (§7.3), and the daemon's
// comparison of rights (`Rights::is_narrower_or_equal`), so a narrowing
// built here is one the daemon takes as narrower.
import type { Access, PeopleAgent, PeopleGrant, PeopleInvite, Rights } from "../api";

/** The line beside per-agent controls: what narrowing one agent of a
 * connected machine does and does not bound. */
export const MACHINE_BOUNDARY =
  "Each agent has its own credential, so its limits hold against other agents' mistakes. Agents on one machine share its user account, though: the machine is the hard boundary.";

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

/** An elevation as single actions at single locations (`loc` null: an
 * access with no locations, as `clax-ask` is). */
type Raised = { type: string; action: string; loc: string | null; level?: string };
const raisedOf = (r: Rights | null): Raised[] =>
  (r ?? []).flatMap(x => x.actions.flatMap(action => (x.locations ?? [null]).map(loc => ({ type: x.type, action, loc, ...x.level && { level: x.level } }))));
const isCap = (c: Cap) => (p: Raised) => p.type === c.type && p.action === c.action;

/** Where agent `a`'s elevation raises capability `c`: `*` for every
 * artifact, else artifact IDs; empty when it does not. */
export const raisedAt = (a: PeopleAgent, c: Cap) => raisedOf(a.elevate).filter(isCap(c)).map(p => p.loc ?? "*");

/** Agent `a`'s capabilities on machine `g`: those of its person's rights
 * and those the owner raised it with. `machineOff` for those the machine's
 * narrowing turns off, which the agent's cannot turn on; `raised` where the
 * elevation adds one (`*`, or artifact IDs); `locked` when the agent's own
 * narrowing does not decide it: raised at every artifact, or beyond its
 * person's rights. `on` is the narrowing's, or the elevation's when locked. */
export const agentCaps = (person: Rights, g: PeopleGrant | null, a: PeopleAgent) => {
  const machine = offIn(person, g?.narrow ?? null);
  const own = offIn(person, a.narrow);
  return CAPS.filter(c => allows(person, c) || raisedAt(a, c).length > 0).map(c => {
    const raised = raisedAt(a, c);
    const locked = raised.includes("*") || !allows(person, c);
    return { ...c, raised, locked, machineOff: machine.has(c.key), on: locked || (!machine.has(c.key) && !own.has(c.key)) };
  });
};
export type AgentCap = ReturnType<typeof agentCaps>[number];

/** The narrowing that turns capability `key` of agent `a` on or off: its
 * person's rights less what its machine and it have off, so it lies within
 * both; null when the agent has nothing off beyond its machine. */
export function narrowAgent(person: Rights, g: PeopleGrant | null, a: PeopleAgent, key: string, on: boolean): Rights | null {
  const machine = offIn(person, g?.narrow ?? null);
  const own = toggled(offIn(person, a.narrow), key, on);
  if ([...own].every(k => machine.has(k))) return null;
  return without(person, new Set([...machine, ...own]));
}

/** Agent `a`'s rights without its elevation: its person's, within its
 * machine's narrowing and its own. */
const unraised = (person: Rights, g: PeopleGrant | null, a: PeopleAgent) =>
  without(person, new Set([...offIn(person, g?.narrow ?? null), ...offIn(person, a.narrow)]));
/** The access that raises capability `c` at `loc` (`*` or an artifact ID):
 * a db write at `interact`, an ask with no locations. */
const raiseOf = (c: Cap, loc: string): Access =>
  A(c.type, [c.action], c.type === "clax-ask" ? undefined : [loc], c.type === "clax-db" && c.action === "write" ? "interact" : undefined);
const placeOf = (c: Cap, loc: string) => (c.type === "clax-ask" ? null : loc);

/** Every capability for the owner's raising of agent `a` at `loc` (`*`,
 * every artifact, or one artifact's ID): `has` when the agent's rights
 * without its elevation already allow it there; `wider` when the
 * elevation raises it at every artifact and `loc` is one; `on` when it has
 * it there or it is raised there. Only a capability neither `has` nor
 * `wider` can be raised or dropped at `loc`. */
export const raiseCaps = (person: Rights, g: PeopleGrant | null, a: PeopleAgent, loc = "*") => {
  const base = unraised(person, g, a);
  const up = raisedOf(a.elevate);
  return CAPS.map(c => {
    const here = up.some(p => isCap(c)(p) && p.loc === placeOf(c, loc));
    const covered = narrowerOrEqual([raiseOf(c, loc)], a.elevate ?? []);
    const has = narrowerOrEqual([raiseOf(c, loc)], base);
    return { ...c, has, wider: covered && !here, on: has || covered };
  });
};

/** The elevation that raises agent `a` with capability `key` at `loc`
 * (`*` or an artifact ID), or drops it there, keeping the rest of its
 * elevation; raising at every artifact drops that capability's raises at
 * single artifacts. Null when nothing is left raised. */
export function elevation(a: PeopleAgent, key: string, on: boolean, loc = "*"): Rights | null {
  const c = CAPS.find(x => x.key === key);
  if (!c) return a.elevate;
  const at = placeOf(c, loc);
  let up = raisedOf(a.elevate).filter(p => !(isCap(c)(p) && (p.loc === at || (on && at === "*"))));
  if (on) up = [...up, { type: c.type, action: c.action, loc: at, ...raiseOf(c, loc).level && { level: "interact" } }];
  const order = (p: Raised) => { const i = CAPS.findIndex(x => isCap(x)(p)); return i < 0 ? CAPS.length : i; };
  const out = new Map<string, Access>();
  for (const p of [...up].sort((x, y) => order(x) - order(y))) {
    const k = `${p.type} ${p.loc} ${p.level ?? ""}`;
    const same = out.get(k);
    if (same) same.actions.push(p.action);
    else out.set(k, A(p.type, [p.action], p.loc === null ? undefined : [p.loc], p.level));
  }
  return out.size ? [...out.values()] : null;
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
