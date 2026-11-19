// The browser tab's title on an artifact's view: "<title> · Clax", as the
// other pages name themselves ("Tags · Clax", "Inbox · Clax"), and "Clax"
// while no artifact is shown. The unread count's prefix (`q/badge.ts`,
// "(3) ") is kept: the badge puts it back only when the count changes.

const PREFIX = /^\(\d+\) /;

/** The tab's title for an artifact titled `title` (null: none shown). */
export const tabTitle = (title: string | null): string => (title ? `${title} · Clax` : "Clax");

/** Sets `doc`'s title for `title` (`tabTitle`), after the unread count's
 * prefix it carries; leaves it alone when it is that already. */
export function setTabTitle(doc: Document, title: string | null): void {
  const next = (PREFIX.exec(doc.title)?.[0] ?? "") + tabTitle(title);
  if (doc.title !== next) doc.title = next;
}
