// Opening an inbox item: mark it read, then go where it leads.
import type { InboxItem } from "../api";
import type { InboxFeed } from "./feed.svelte";

/** Marks `i` read (when unread) and then goes to its `url` with `go`; a
 * failed mark does not keep the person from the item. */
export function openItem(feed: InboxFeed, i: InboxItem, go: (url: string) => void = url => location.assign(url)): Promise<void> {
  const marked = i.read ? Promise.resolve() : feed.mark(i.id, true).then(() => {}, () => {});
  return marked.then(() => go(i.url));
}
