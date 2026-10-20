// Tags (docs/contract.md "Tags") as the gallery, the inbox and the artifact
// view show and filter by them: which tags a filter names, the filter in a
// page's address (`?tag=a&tag=b`, match all), and the tags a list carries.
// A filter names tags by name (or ID), compared without case, as the daemon
// does. Pure: no DOM. The gallery's entry loads it, so it stays small.
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

/** `sel` with `name` added, or taken out when it holds it already (in any case). */
export const toggle = (sel: readonly string[], name: string): string[] =>
  sel.some(s => fold(s) === fold(name)) ? sel.filter(s => fold(s) !== fold(name)) : [...sel, name];

/** The tags `list` carries, each once, sorted by name. */
export function tagsInUse(list: readonly Pick<Artifact, "tags">[]): TagRef[] {
  const by = new Map<string, TagRef>();
  for (const a of list) for (const t of a.tags ?? []) by.set(t.id, t);
  return [...by.values()].sort((a, b) => a.name.localeCompare(b.name));
}

/** The tag bar's chips: the tags in use, then each tag the filter names
 * that none of them is (a tag no artifact carries, or a name none has),
 * shown so it can be taken out again. */
export function barTags(inUse: readonly TagRef[], sel: readonly string[]): TagRef[] {
  const extra = sel.filter(s => !inUse.some(t => names(t, s))).map(s => ({ id: `?${s}`, name: s, colour: "gray" }));
  return [...inUse, ...extra];
}
