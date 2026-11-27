// The tab's title: the page's own name, after the unread count the inbox
// puts before it (`q/badge.ts`).

/** The unread count before a title, `(N) `. */
export const COUNT = /^\(\d+\) /;

/** Sets `doc`'s title to `base`, keeping the unread count it shows. */
export function setTitle(doc: Document, base: string): void {
  const title = (COUNT.exec(doc.title)?.[0] ?? "") + base;
  if (doc.title !== title) doc.title = title;
}
