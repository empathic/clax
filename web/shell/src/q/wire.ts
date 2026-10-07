// The page-wide effects of the inbox: the count in the tab's title and icon,
// and the dot on the threads controls while a question about the shown
// artifact is open. They follow the feeds' listeners (not effects, whose
// root's runtime helper belongs to the shared chunk).
import { setBadge } from "./badge";
import { shared } from "./shared";

/** Keeps the title and icon on the unread count; the result stops it. */
export function followBadge(doc: Document): () => void {
  const { inbox } = shared();
  setBadge(doc, inbox.unread);
  return inbox.onCount(n => setBadge(doc, n));
}

/** Marks `<html data-questions>` while a question about `aid` is open (the
 * threads button and the phone's Threads tab show a dot); the result stops it. */
export function followDot(doc: Document, aid: string): () => void {
  const { questions } = shared();
  const show = () => doc.documentElement.toggleAttribute("data-questions", questions.byArtifact(aid).some(q => q.status === "open"));
  show();
  const off = questions.listen(show);
  return () => { off(); doc.documentElement.removeAttribute("data-questions"); };
}
