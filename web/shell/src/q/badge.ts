// The tab's unread count (spec 2026-10-06-agent-questions-and-inbox §9.7):
// the title takes the prefix `(N) ` and the icon becomes the mark with a
// red-orange dot; at 0 both are as they were.

const PREFIX = /^\(\d+\) /;

/** The mark with a red-orange dot, top right, served beside the mark. */
const DOTTED = "/_clax/mark-dot.svg";

/** Shows `n` unread items in `doc`'s title and icon. */
export function setBadge(doc: Document, n: number): void {
  const base = doc.title.replace(PREFIX, "");
  const title = n > 0 ? `(${n}) ${base}` : base;
  if (doc.title !== title) doc.title = title;
  const link = doc.querySelector<HTMLLinkElement>('link[rel~="icon"]');
  if (!link) return;
  // The icon as the page had it, kept on the link while it is swapped.
  const plain = link.dataset.plain ?? link.getAttribute("href") ?? "";
  if (n > 0) {
    link.dataset.plain = plain;
    if (link.getAttribute("href") !== DOTTED) link.setAttribute("href", DOTTED);
  } else if (link.dataset.plain !== undefined) {
    link.setAttribute("href", plain);
    delete link.dataset.plain;
  }
}
