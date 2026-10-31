// What the question module loads once the owner's inbox topic goes live
// (`index.ts`): the page's confirmation feed, following the topic through
// the inbox feed, and the toasts of the decisions waiting for the owner's
// confirmation, mounted on the page's body. Loaded on demand, it adds
// nothing to what the module loads first.
import { mount, unmount } from "svelte";
import ConfirmToasts from "./ConfirmToasts.svelte";
import { confirmations } from "./confirm.svelte";
import type { InboxFeed } from "./feed.svelte";

/** Follows `inbox`'s confirmations, fetching the open ones now, and shows
 * them as toasts; the result stops both. */
export function follow(inbox: Pick<InboxFeed, "hear">, doc: Document = document): () => void {
  const feed = confirmations();
  feed.start(inbox);
  void feed.refresh();
  const toasts = mount(ConfirmToasts, { target: doc.body, props: {} });
  return () => { feed.stop(); void unmount(toasts); };
}
