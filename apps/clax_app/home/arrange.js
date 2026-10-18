// The home screen's arrangement: the `layout/main` document in the home
// app's own documents, and the rules over it. Nothing here touches the
// page or the store. Every function returns a new layout and keeps what
// it does not know, so a layout a later release wrote survives an earlier
// one's write (add fields, never change the meaning of one): unknown
// fields, at the top and in a group or an open count; a group's unknown
// look (shown as a folder); an order entry of an unknown kind (a string
// "<kind>:…" other than "g:…", or an object), in place and not shown; and
// a `v` above 1, never lowered. keepNewer puts back, before a write, the
// known fields of such a layout that this release cannot read.
//
//   {v: 1, starred: [appId], order: [appId | "g:<groupId>"],
//    groups: {groupId: {name, look: "folder" | "pile", apps: [appId]}},
//    seen: [appId], opens: {appId: {n, last, days: {"YYYY-MM-DD": n}}},
//    bigTiles: boolean (absent means true),
//    big: [appId] (the apps on 2×2 tiles when it was last written)}

export const LAYOUT_PATH = 'layout/main';
/** The days `opens[id].days` keeps, today included. */
export const OPEN_DAYS = 14;

const GROUP = 'g:';
export const groupKey = (gid) => GROUP + gid;
export const isGroupKey = (key) => typeof key === 'string' && key.startsWith(GROUP);
const gidOf = (key) => key.slice(GROUP.length);
const isId = (v) => typeof v === 'string' && v.length > 0 && v.length <= 64;
const obj = (v) => (v && typeof v === 'object' && !Array.isArray(v) ? v : null);
/** An order entry of a kind this release does not know: kept, not shown. */
const isOther = (v) => (typeof v === 'string' && /^[^:]+:/.test(v) && !isGroupKey(v)) || obj(v) !== null;
const isKey = (v) => isOther(v) || isId(v) || (isGroupKey(v) && isId(gidOf(v)));
const ids = (v, ok = isId) => (Array.isArray(v) ? [...new Set(v.filter(ok))] : []);
/** `list` (an order) with `key` put before its `index`-th shown entry,
 * clamped: an index counts only the entries the page shows. */
const insertShown = (list, key, index) => {
  const at = list.flatMap((k, i) => (isOther(k) ? [] : [i]));
  const n = Math.max(0, Math.min(Number.isFinite(index) ? Math.trunc(index) : at.length, at.length));
  const i = n < at.length ? at[n] : list.length;
  return [...list.slice(0, i), key, ...list.slice(i)];
};
const shownIndex = (list, key) => list.filter((k) => !isOther(k)).indexOf(key);
const DAY = /^\d{4}-\d{2}-\d{2}$/;
export const dayOf = (date) => date.toISOString().slice(0, 10);
const daysBefore = (date, n) => dayOf(new Date(date.getTime() - n * 86_400_000));

/** `body` (the stored document, or null) as a layout: every known field
 * there and well formed, every unknown field as it was. */
export function normalize(body) {
  const b = obj(body) ?? {};
  const groups = {};
  for (const [gid, g] of Object.entries(obj(b.groups) ?? {})) {
    if (!isId(gid) || !obj(g)) continue;
    groups[gid] = { ...g, name: typeof g.name === 'string' && g.name.trim() ? g.name : 'Group', look: typeof g.look === 'string' && g.look ? g.look : 'folder', apps: ids(g.apps) };
  }
  const opens = {};
  for (const [id, o] of Object.entries(obj(b.opens) ?? {})) {
    if (!isId(id) || !obj(o)) continue;
    const days = {};
    for (const [d, n] of Object.entries(obj(o.days) ?? {})) if (DAY.test(d) && Number.isInteger(n) && n > 0) days[d] = n;
    opens[id] = { ...o, n: Number.isInteger(o.n) && o.n > 0 ? o.n : 0, last: typeof o.last === 'string' ? o.last : null, days };
  }
  const order = Array.isArray(b.order) ? b.order.filter((k, i) => isKey(k) && (typeof k !== 'string' || b.order.indexOf(k) === i)) : [];
  const big = b.big === undefined ? {} : { big: ids(b.big) };
  return { ...b, v: Number.isFinite(b.v) && b.v >= 1 ? b.v : 1, starred: ids(b.starred), order, groups, seen: ids(b.seen), opens, ...big };
}

/** Whether a stored known field's value is one this release reads. */
const READS = {
  starred: Array.isArray,
  order: Array.isArray,
  seen: Array.isArray,
  groups: (v) => obj(v) !== null,
  opens: (v) => obj(v) !== null,
  bigTiles: (v) => typeof v === 'boolean',
  big: Array.isArray,
};

/** The body to write when `next` replaces `stored`: when `stored` is a
 * later release's layout (`v` above 1), its `v`, never lowered, and each
 * known field there whose value this release cannot read, as it was. */
export function keepNewer(stored, next) {
  const s = obj(stored);
  if (!s || !(Number.isFinite(s.v) && s.v > 1)) return next;
  const out = { ...next, v: Math.max(s.v, Number.isFinite(next?.v) ? next.v : 1) };
  for (const [k, reads] of Object.entries(READS)) if (s[k] !== undefined && !reads(s[k])) out[k] = s[k];
  return out;
}

const newestFirst = (apps) => [...apps].sort((a, b) => String(b.updatedAt || '').localeCompare(String(a.updatedAt || '')));

/** `layout` made to match `apps` (the `home` capability's list): an ID no
 * longer listed goes from everywhere; an app in two places keeps the
 * first of Starred, its group, All apps; a group left with one app gives
 * way to that app in its place, one left with none goes, and one no entry
 * names goes last in All apps; a listed app in no place goes first in All
 * apps, newest first. `first` (nothing stored yet) marks every listed app
 * seen, so nothing reads as new on the first load. */
export function reconcile(layout, apps, { first = false } = {}) {
  const l = normalize(layout);
  const listed = new Set(apps.map((a) => a.id));
  const placed = new Set();
  const take = (id) => {
    if (!listed.has(id) || placed.has(id)) return false;
    placed.add(id);
    return true;
  };
  const starred = l.starred.filter(take);
  const members = {};
  for (const [gid, g] of Object.entries(l.groups)) members[gid] = g.apps.filter(take);
  const order = [];
  const named = new Set();
  const placeGroup = (gid) => {
    named.add(gid);
    const m = members[gid];
    if (m.length >= 2) order.push(groupKey(gid));
    else if (m.length === 1) order.push(m[0]);
  };
  for (const key of l.order) {
    if (isOther(key)) order.push(key);
    else if (isGroupKey(key)) {
      const gid = gidOf(key);
      if (members[gid] && !named.has(gid)) placeGroup(gid);
    } else if (take(key)) order.push(key);
  }
  for (const gid of Object.keys(members)) if (!named.has(gid)) placeGroup(gid);
  const groups = {};
  for (const [gid, g] of Object.entries(l.groups)) if (members[gid].length >= 2) groups[gid] = { ...g, apps: members[gid] };
  const fresh = newestFirst(apps.filter((a) => !placed.has(a.id))).map((a) => a.id);
  const seen = first ? [...new Set([...l.seen.filter((id) => listed.has(id)), ...apps.map((a) => a.id)])] : l.seen.filter((id) => listed.has(id));
  const opens = Object.fromEntries(Object.entries(l.opens).filter(([id]) => listed.has(id)));
  return { ...l, starred, order: [...fresh, ...order], groups, seen, opens };
}

/** What the page shows: Starred's entries and All apps', in order, each
 * `{key, app}` or, for a group, `{key, group: {id, name, look}, apps}`. */
export function sections(layout, apps) {
  const byId = new Map(apps.map((a) => [a.id, a]));
  const appEntry = (id) => (byId.has(id) ? { key: id, app: byId.get(id) } : null);
  return {
    starred: layout.starred.map(appEntry).filter(Boolean),
    all: layout.order.map((key) => {
      if (isOther(key)) return null;
      if (!isGroupKey(key)) return appEntry(key);
      const gid = gidOf(key);
      const g = layout.groups[gid];
      const list = g ? g.apps.map((id) => byId.get(id)).filter(Boolean) : [];
      return list.length ? { key, group: { id: gid, name: g.name, look: g.look === 'pile' ? 'pile' : 'folder' }, apps: list } : null;
    }).filter(Boolean),
  };
}

/** The group app `id` is in, or null. */
export function groupOf(layout, id) {
  for (const [gid, g] of Object.entries(normalize(layout).groups)) if (g.apps.includes(id)) return gid;
  return null;
}

/** `key` (an app ID or a group's key) moved to `section` ("starred" or
 * "all") at `index` among its entries, clamped, out of wherever it was;
 * a group it leaves keeps its other apps. A group never goes to Starred:
 * that move changes nothing. */
export function moveTo(layout, key, section, index) {
  const l = normalize(layout);
  if (isGroupKey(key) && section === 'starred') return l;
  const out = (list) => list.filter((k) => k !== key);
  const groups = Object.fromEntries(Object.entries(l.groups).map(([gid, g]) => [gid, { ...g, apps: out(g.apps) }]));
  const into = (list) => {
    const i = Math.max(0, Math.min(Number.isFinite(index) ? Math.trunc(index) : list.length, list.length));
    return [...list.slice(0, i), key, ...list.slice(i)];
  };
  const starred = section === 'starred' ? into(out(l.starred)) : out(l.starred);
  const order = section === 'all' ? insertShown(out(l.order), key, index) : out(l.order);
  return { ...l, starred, order, groups };
}

/** App `id` last in Starred, out of any group. */
export const star = (layout, id) => moveTo(layout, id, 'starred', Infinity);
/** App `id` first in All apps. */
export const unstar = (layout, id) => moveTo(layout, id, 'all', 0);

/** App `id` opened from the home screen at `now`: seen, and counted (`n`
 * all told, `last`, and `days`, the opens per UTC day of the last
 * OPEN_DAYS days). */
export function opened(layout, id, now) {
  const l = normalize(layout);
  const o = l.opens[id] ?? { n: 0, last: null, days: {} };
  const since = daysBefore(now, OPEN_DAYS - 1);
  const today = dayOf(now);
  const days = Object.fromEntries(Object.entries(o.days).filter(([d]) => d >= since));
  days[today] = (days[today] ?? 0) + 1;
  return {
    ...l,
    seen: l.seen.includes(id) ? l.seen : [...l.seen, id],
    opens: { ...l.opens, [id]: { ...o, n: o.n + 1, last: now.toISOString(), days } },
  };
}

/** Whether app `id` has not been opened from the home screen yet. */
export const isNew = (layout, id) => !layout.seen.includes(id);

/** How many apps under All apps grow to a 2×2 tile, and the opens in the
 * last OPEN_DAYS days each needs. */
export const BIG_TOP = 3;
export const BIG_MIN = 5;
/** An app already big (in the layout's `big`) keeps its tile until its
 * opens fall BIG_KEEP below BIG_MIN, or another app has BIG_KEEP opens
 * more than it. */
export const BIG_KEEP = 2;

/** The apps under All apps (not in Starred or a group) that take a 2×2
 * cell at `now`: the BIG_TOP opened most from the home screen in the last
 * OPEN_DAYS UTC days, with at least BIG_MIN opens each, most first. An app
 * already big counts as having just under BIG_KEEP opens more than it has,
 * so it wins a tie and keeps its tile at BIG_MIN − BIG_KEEP + 1 opens and
 * against an app with one open more; past that, ties go to the one opened
 * later, then by ID. None when `bigTiles` is false. */
export function bigApps(layout, now) {
  const l = normalize(layout);
  if (l.bigTiles === false) return new Set();
  const since = daysBefore(now, OPEN_DAYS - 1);
  const loose = new Set(l.order.filter((k) => !isGroupKey(k)));
  const was = new Set(l.big ?? []);
  const ranked = Object.entries(l.opens)
    .filter(([id]) => loose.has(id))
    .map(([id, o]) => {
      const n = Object.entries(o.days).reduce((sum, [d, c]) => (d >= since ? sum + c : sum), 0);
      return { id, score: was.has(id) ? n + BIG_KEEP - 0.5 : n, last: o.last ?? '' };
    })
    .filter((r) => r.score >= BIG_MIN)
    .sort((a, b) => b.score - a.score || b.last.localeCompare(a.last) || a.id.localeCompare(b.id));
  return new Set(ranked.slice(0, BIG_TOP).map((r) => r.id));
}

/** `layout` with `big`, the apps `bigApps` puts on 2×2 tiles at `now`, so
 * the next ranking starts from them. */
export const keepBig = (layout, now) => {
  const l = normalize(layout);
  return { ...l, big: [...bigApps(l, now)] };
};

/** Big tiles on or off (`bigTiles`, true when absent). */
export const setBigTiles = (layout, on) => ({ ...normalize(layout), bigTiles: !!on });

/** The longest a group's name may be, in characters. */
const NAME_MAX = 40;

/** A new group's name: the icon word `a` and `b` share, capitalised, else "Group". */
export function groupNameOf(a, b) {
  const word = (x) => (typeof x?.icon?.word === 'string' ? x.icon.word.trim().toLowerCase() : '');
  const w = word(a);
  return w && w === word(b) ? w[0].toUpperCase() + w.slice(1) : 'Group';
}

/** App `draggedId` dropped on app `targetId`: a new folder `gid` named
 * `name`, holding the target then the dragged app, at the target's place
 * in All apps; a target in Starred leaves it (groups are not starred), and
 * the folder goes first in All apps, right under Starred. A group dropped
 * on anything, or on itself, makes nothing; so does a `gid` already in use. */
export function makeGroup(layout, targetId, draggedId, gid, name) {
  const l = normalize(layout);
  if (isGroupKey(targetId) || isGroupKey(draggedId) || targetId === draggedId || !isId(gid) || l.groups[gid]) return l;
  // The target's place among All apps' entries once the dragged app is
  // out; one not in All apps has none.
  const at = l.order.filter((k) => k !== draggedId).indexOf(targetId);
  const out = moveTo(moveTo(l, draggedId, 'all', 0), targetId, 'all', 0);
  const order = out.order.filter((k) => k !== targetId && k !== draggedId);
  return {
    ...out,
    order: at >= 0 ? [...order.slice(0, at), groupKey(gid), ...order.slice(at)] : insertShown(order, groupKey(gid), 0),
    groups: { ...out.groups, [gid]: { name, look: 'folder', apps: [targetId, draggedId] } },
  };
}

/** App `id` into group `gid` at `index` among its apps (clamped), out of
 * wherever it was. A group never goes in a group: that changes nothing. */
export function addToGroup(layout, gid, id, index = Infinity) {
  const l = normalize(layout);
  if (isGroupKey(id) || !l.groups[gid]) return l;
  const out = moveTo(l, id, 'all', 0);
  const order = out.order.filter((k) => k !== id);
  const apps = out.groups[gid].apps.filter((k) => k !== id);
  const i = Math.max(0, Math.min(Number.isFinite(index) ? Math.trunc(index) : apps.length, apps.length));
  return { ...out, order, groups: { ...out.groups, [gid]: { ...out.groups[gid], apps: [...apps.slice(0, i), id, ...apps.slice(i)] } } };
}

/** App `id` out of its group, into All apps right after the group; an app
 * in no group stays where it is. */
export function removeFromGroup(layout, id) {
  const l = normalize(layout);
  const gid = groupOf(l, id);
  if (!gid) return l;
  return moveTo(l, id, 'all', shownIndex(l.order, groupKey(gid)) + 1);
}

/** App `id` to `index` among group `gid`'s apps (clamped). */
export const moveInGroup = (layout, gid, id, index) => addToGroup(layout, gid, id, index);

/** Group `gid` named `name`, trimmed, at most NAME_MAX characters; an
 * empty name changes nothing. */
export function renameGroup(layout, gid, name) {
  const l = normalize(layout);
  const n = [...String(name ?? '').trim()].slice(0, NAME_MAX).join('');
  if (!n || !l.groups[gid]) return l;
  return { ...l, groups: { ...l.groups, [gid]: { ...l.groups[gid], name: n } } };
}

/** Group `gid` shown as a folder or as a pile; any other look changes nothing. */
export function setLook(layout, gid, look) {
  const l = normalize(layout);
  if (!l.groups[gid] || (look !== 'folder' && look !== 'pile')) return l;
  return { ...l, groups: { ...l.groups, [gid]: { ...l.groups[gid], look } } };
}

/** Group `gid` gone, its apps in its place in All apps, in its order. */
export function ungroup(layout, gid) {
  const l = normalize(layout);
  const g = l.groups[gid];
  if (!g) return l;
  const groups = { ...l.groups };
  delete groups[gid];
  const order = l.order.flatMap((k) => (k === groupKey(gid) ? g.apps : [k]));
  return { ...l, order, groups };
}
