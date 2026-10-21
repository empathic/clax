// Tags (docs/contract.md "Tags") as the gallery, the inbox and the artifact
// view show and filter by them: which tags a filter names, the filter in a
// page's address (`?tag=a&tag=b`, match all), and the tags a list carries.
// A filter holds tag IDs (`canonical`): an address names each tag by its
// name, readable (`refsForUrl`), and may name one by ID, as the daemon reads
// both; names compare without case. Pure: no DOM. The gallery's entry loads it, so it stays small.
import type { Artifact, TagRef } from "../api";

/** The palette keys a tag's colour is one of, in the daemon's order. */
export const COLOURS = ["gray", "red", "orange", "yellow", "green", "teal", "blue", "purple", "pink", "brown"] as const;

const fold = (s: string) => s.trim().toLowerCase();

/** Whether `ref` (a tag's ID, or its name in any case) names `t`. */
export const names = (t: TagRef, ref: string): boolean => t.id === ref || fold(t.name) === fold(ref);

/** Whether filter `sel` holds tag `t`. */
export const holds = (sel: readonly string[], t: TagRef): boolean => sel.some(s => names(t, s));

/** The tags `search`'s `tag` parameters name, in order: trimmed, blanks
 * and repeats (in any case) left out. */
export function tagsFromUrl(search: string): string[] {
  const out: string[] = [];
  for (const v of new URLSearchParams(search).getAll("tag")) {
    const t = v.trim();
    if (t && !out.some(o => fold(o) === fold(t))) out.push(t);
  }
  return out;
}

/** `search` with its `tag` parameters replaced by `tags`, every other
 * parameter kept: "" when none is left, else `?…`. */
export function withTags(search: string, tags: readonly string[]): string {
  const u = new URLSearchParams(search);
  u.delete("tag");
  for (const t of tags) u.append("tag", t);
  const s = u.toString();
  return s ? `?${s}` : "";
}

/** Whether `a` carries every tag `sel` names (match all); with none named, it does. */
export const carriesAll = (a: Pick<Artifact, "tags">, sel: readonly string[]): boolean =>
  sel.every(s => (a.tags ?? []).some(t => names(t, s)));

/** `sel` with tag `t` added (by its ID), or every entry naming it taken out
 * when it holds it already (by its ID, or its name in any case). */
export const toggle = (sel: readonly string[], t: TagRef): string[] =>
  holds(sel, t) ? sel.filter(s => !names(t, s)) : [...sel, t.id];

/** A tag ID's shape: the daemon refuses it as a name, so it names a tag by ID alone. */
const ID = /^t_[0-9a-z]{10}$/i;

/** Filter `sel` in its canonical form: each entry naming a tag of `known`
 * (by ID or name) as that tag's ID, once; an ID the whole list `all` lacks
 * (the tag was deleted) left out; any other entry (a name no tag has) kept
 * as given, matching nothing, so it can be taken out. */
export function canonical(sel: readonly string[], known: readonly TagRef[], all?: readonly TagRef[] | null): string[] {
  const out: string[] = [];
  for (const s of sel) {
    const t = known.find(k => names(k, s)) ?? all?.find(k => names(k, s));
    const c = t ? t.id : s;
    if (!t && all && ID.test(s)) continue;
    if (!out.includes(c)) out.push(c);
  }
  return out;
}

/** Filter `sel` as an address names it: each tag by its name where `known`
 * has it (readable, and as the daemon reads it), else as given. */
export const refsForUrl = (sel: readonly string[], known: readonly TagRef[]): string[] =>
  sel.map(s => known.find(t => t.id === s)?.name ?? s);

/** Whether two filters are the same, entry for entry. */
export const same = (a: readonly string[], b: readonly string[]): boolean => a.length === b.length && a.every((s, i) => s === b[i]);

/** The tags `list` carries, each once, sorted by name. */
export function tagsInUse(list: readonly Pick<Artifact, "tags">[]): TagRef[] {
  const by = new Map<string, TagRef>();
  for (const a of list) for (const t of a.tags ?? []) by.set(t.id, t);
  return [...by.values()].sort((a, b) => a.name.localeCompare(b.name));
}

/** The tag bar's chips: the tags in use, then each tag the filter names
 * that none of them is (a tag no artifact carries, or a name none has),
 * shown so it can be taken out again: as the whole list `all` has it, else
 * in gray under the name given. */
export function barTags(inUse: readonly TagRef[], sel: readonly string[], all?: readonly TagRef[] | null): TagRef[] {
  const extra = sel.filter(s => !inUse.some(t => names(t, s))).map(s => all?.find(t => names(t, s)) ?? { id: `?${s}`, name: s, colour: "gray" });
  return [...inUse, ...extra];
}
