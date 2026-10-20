// Browser notifications for unread inbox items (spec
// 2026-10-06-agent-questions-and-inbox §9.7). The stream hub picks the tab
// that shows one (the most recently focused, when no Clax tab has focus);
// this shows it, if the person granted permission from **Notify me**. The
// tag is the page's or the question's, so a burst about one page replaces
// itself. A click focuses the tab, opens the item and marks it read; a
// notification closes once its item is read. A notification's text is the
// item's title and one-line text, made safe to show (`notificationText`).
import type { InboxItem } from "../api";
import { markInbox } from "./api";
import { itemText, itemTitle } from "./inbox-model";
import { loadMarkdown } from "../view/md-load.svelte";

/** The longest notification body, in characters, "…" included. */
export const NOTIFY_BODY = 180;

// C0 and C1 controls, the Arabic letter mark, LRM and RLM, the embeddings
// and overrides, and the isolates: none may steer a notification's text.
// oxlint-disable-next-line no-control-regex -- matching control characters is the point
const UNSAFE = /[\u0000-\u001f\u007f-\u009f\u061c\u200e\u200f\u202a-\u202e\u2066-\u2069]/g;

const clean = (s: string): string => s.replace(/\s+/g, " ").replace(UNSAFE, "").replace(/ {2,}/g, " ").trim();

/** A notification's title and body for `i`: control and bidirectional
 * characters removed, the body cut to [`NOTIFY_BODY`] characters with "…". */
export function notificationText(i: InboxItem, others: InboxItem[] = []): { title: string; body: string } {
  const body = [...clean(itemText(i))];
  return {
    title: clean(itemTitle(i, others)),
    body: body.length > NOTIFY_BODY ? `${body.slice(0, NOTIFY_BODY - 1).join("").trimEnd()}…` : body.join(""),
  };
}

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
    // A reply's or a question's notification shows its text without Markdown syntax once the module is here.
    void loadMarkdown();
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

  /** Closes every notification shown. */
  closeAll(): void {
    for (const id of [...this.shown.keys()]) this.close(id);
  }

  /** Closes item `id`'s notification (it was read). */
  close(id: string): void {
    const n = this.shown.get(id);
    if (!n) return;
    this.shown.delete(id);
    n.close();
  }
}
