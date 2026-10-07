// Browser notifications for unread inbox items (spec
// 2026-10-06-agent-questions-and-inbox §9.7). The stream hub picks the tab
// that shows one (the most recently focused, when no Clax tab has focus);
// this shows it, if the person granted permission from **Notify me**. The
// tag is the page's or the question's, so a burst about one page replaces
// itself. A click focuses the tab, opens the item and marks it read; a
// notification closes once its item is read.
import type { InboxItem } from "../api";
import { markInbox } from "./api";
import { notificationText } from "./inbox-model";

/** The Clax mark, the notifications' icon. */
export const NOTIFY_ICON = "/_clax/mark.svg";

/** A notification's tag: the question's, else the page's, else the item's. */
export const notifyTag = (i: InboxItem): string => `clax-inbox-${i.question?.id ?? i.artifact?.id ?? i.id}`;

export class Notifier {
  /** What each item's notification is, while it shows. */
  declare private shown: Map<string, Notification>;

  constructor(
    private readonly N: typeof Notification | undefined,
    private readonly open: (url: string) => void,
    private readonly mark: (id: string) => unknown = id => markInbox(id, true).catch(() => {}),
  ) {
    this.shown = new Map();
  }

  /** Shows `i` (an unread item); `others` are listed beside it, for naming its agent. */
  show(i: InboxItem, others: InboxItem[] = []): void {
    const N = this.N;
    if (!N || N.permission !== "granted" || i.read) return;
    const { title, body } = notificationText(i, others);
    const tag = notifyTag(i);
    let n: Notification;
    try { n = new N(title, { body, tag, icon: NOTIFY_ICON }); } catch { return; }
    // A notification of the same tag replaces the one before.
    for (const [id, m] of this.shown) if (m.tag === tag) this.shown.delete(id);
    this.shown.set(i.id, n);
    n.onclick = () => {
      globalThis.focus?.();
      this.open(i.url);
      this.mark(i.id);
      this.close(i.id);
    };
  }

  /** Closes item `id`'s notification (it was read). */
  close(id: string): void {
    const n = this.shown.get(id);
    if (!n) return;
    this.shown.delete(id);
    n.close();
  }
}
