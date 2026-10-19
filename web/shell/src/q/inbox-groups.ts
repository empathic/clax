// The inbox's groups by artifact (`GET /api/inbox?group=artifact`): one
// group per page an item is about, and one, "Other", for the items about
// none (connection requests, ended agents, questions asked outside a
// page). Groups are ordered by their newest item; a group's counts are the
// daemon's, over every match, not only the rows listed. Pure: no DOM.
import type { InboxGroup, InboxItem } from "../api";

/** The `artifact` filter, and the group key, of the items about no artifact. */
export const NO_ARTIFACT = "none";
/** Rows a group lists before **Show more**. */
export const GROUP_ROWS = 3;
/** Rows each **Show more** adds to a group. */
export const MORE_ROWS = 20;

/** One group as a section keeps it: `key` is the artifact's ID, or
 * `NO_ARTIFACT`; `next` the cursor of its rows not yet listed. */
export type Group = { key: string; artifact: InboxItem["artifact"]; total: number; unread: number; newestAt: string; items: InboxItem[]; next: string | null };

/** The key of the group `i` belongs in. */
export const groupKey = (i: Pick<InboxItem, "artifact">): string => i.artifact?.id ?? NO_ARTIFACT;

export const fromWire = (g: InboxGroup): Group => ({
  key: g.artifact?.id ?? NO_ARTIFACT, artifact: g.artifact, total: g.total, unread: g.unread,
  newestAt: g.newest_at, items: g.items, next: g.next_cursor,
});

/** A group's heading: its page's title, "a deleted page", or "Other". */
export const groupTitle = (g: Pick<Group, "key" | "artifact">): string =>
  (g.key === NO_ARTIFACT ? "Other" : g.artifact?.title ?? "a deleted page");

/** Where a group's heading leads: its page, while it has a title (not deleted). */
export const groupHref = (g: Pick<Group, "key" | "artifact">): string | null =>
  (g.key !== NO_ARTIFACT && g.artifact?.title ? `/a/${encodeURIComponent(g.key)}` : null);

/** A short age: "now", "2m", "1h", "3d", else the date. */
export function shortAge(iso: string, now: Date): string {
  const s = Math.max(0, Math.round((now.getTime() - new Date(iso).getTime()) / 1000));
  if (s < 60) return "now";
  if (s < 3600) return `${Math.round(s / 60)}m`;
  if (s < 86400) return `${Math.round(s / 3600)}h`;
  if (s < 7 * 86400) return `${Math.round(s / 86400)}d`;
  return iso.slice(0, 10);
}

/** Groups after another page of them: one already listed keeps its place and rows. */
export const appendGroups = (have: Group[], page: Group[]): Group[] =>
  [...have, ...page.filter(g => !have.some(h => h.key === g.key))];

/** `groups` with more rows of group `key` appended (repeats dropped) and its cursor moved. */
export const appendRows = (groups: Group[], key: string, items: InboxItem[], next: string | null): Group[] =>
  groups.map(g => (g.key === key ? { ...g, items: [...g.items, ...items.filter(i => !g.items.some(x => x.id === i.id))], next } : g));

/** `groups` with `item` as it is now: replaced where a group lists it, and
 * the unread count of its group moved by `delta` (an item known to match
 * that was read, or unread again, whether listed or not). */
export function takeItem(groups: Group[], item: InboxItem, delta: number): Group[] {
  const key = groupKey(item);
  return groups.map(g => {
    const items = g.items.some(x => x.id === item.id) ? g.items.map(x => (x.id === item.id ? item : x)) : g.items;
    const unread = g.key === key ? Math.max(0, g.unread + delta) : g.unread;
    return items === g.items && unread === g.unread ? g : { ...g, items, unread };
  });
}

/** Whether any group lists `id`. */
export const lists = (groups: Group[] | undefined, id: string): boolean => !!groups?.some(g => g.items.some(i => i.id === id));
