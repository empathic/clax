// The question and inbox module (spec 2026-10-06-agent-questions-and-inbox
// §9), loaded after the first paint (at once on `/inbox`): the feeds, the
// surfaces, the notifier and the tab's count. `start` asks the owner routes
// first; for anyone else nothing subscribes and nothing shows. The page's
// stream comes from the host (the gallery's lazy module, the artifact view's
// stream), so this module does not import it: the stream's chunk stays
// shared with the code that loads it already.
import { mount, unmount } from "svelte";
import type { InboxItem } from "../api";
import type { EventStream } from "../stream";
import InboxLink from "./InboxLink.svelte";
import { setBadge } from "./badge";
import { Notifier } from "./notify";
import { shared } from "./shared";
import SidebarQuestions from "./SidebarQuestions.svelte";
import { type Trail, useTrail } from "./trail";
import { followBadge, followDot } from "./wire";

export { default as GallerySummary } from "./GallerySummary.svelte";
export { default as InboxLink } from "./InboxLink.svelte";
export { default as InboxPage } from "./InboxPage.svelte";

/** A notification's click opens its item in a new tab, so nothing this tab holds is lost. */
const openUrl = (url: string) => void window.open(url, "_blank", "noopener");

let started: Promise<boolean> | null = null;
let starts = 0;
// What `start` set up, undone by `stop`.
let undo: (() => void)[] = [];

/** Starts the page's feeds (once): for the owner, the count in the title and
 * icon, and notifications. Resolves whether the caller is the owner. */
export function start(stream: EventStream): Promise<boolean> {
  return (started ??= (async () => {
    const { inbox, questions } = shared();
    const mine = ++starts;
    if (!(await inbox.start(stream)) || starts !== mine) return false;
    void questions.start(stream);
    const notifier = new Notifier(typeof Notification === "undefined" ? undefined : Notification, openUrl);
    undo.push(
      followBadge(document),
      stream.onNotify(d => { const i = d.item as InboxItem | undefined; if (i?.id) notifier.show(i, inbox.latest); }),
      inbox.listen(c => { if ("item" in c && c.item.read) notifier.close(c.item.id); }),
      () => setBadge(document, 0),
    );
    return true;
  })());
}

/** Undoes `start`: the feeds stop following their topics, and the title and icon are as they were. */
export function stop(): void {
  const { inbox, questions } = shared();
  inbox.stop();
  questions.stop();
  for (const f of undo) f();
  undo = [];
  started = null;
  starts++;
}

/** The artifact view's surfaces for artifact `aid`, for the owner: **Inbox**
 * in the top bar (before the island's controls), the questions about it at
 * the top of the sidebar (in the sidebar's `.questions-slot`, each time it
 * mounts), and the dot on the threads controls. `trail` is the view's
 * keyboard trail, which its cards follow. The result removes them and stops
 * the feeds (`stop`). */
export function artifact(aid: string, trail: Trail, stream: EventStream, doc: Document = document): () => void {
  useTrail(trail);
  let stopped = false;
  let off: (() => void)[] = [];
  let side: { el: Element; c: Record<string, unknown> } | null = null;
  const fill = (el: Element | null) => {
    if (!el || side?.el === el) return;
    if (side) void unmount(side.c);
    side = { el, c: mount(SidebarQuestions, { target: el, props: { aid } }) };
  };
  const onSlot = (e: Event) => fill(e.target as Element);
  void start(stream).then(owner => {
    if (!owner || stopped) return;
    const bar = doc.querySelector<HTMLElement>(".topbar");
    if (bar) {
      const link = mount(InboxLink, { target: bar, anchor: bar.querySelector(":scope > .island") ?? undefined, props: {} });
      off.push(() => { void unmount(link); });
    }
    doc.addEventListener("clax-questions-slot", onSlot);
    fill(doc.querySelector(".sidebar > .questions-slot"));
    off.push(() => {
      doc.removeEventListener("clax-questions-slot", onSlot);
      if (side) void unmount(side.c);
      side = null;
    }, followDot(doc, aid));
  });
  return () => {
    stopped = true;
    for (const f of off) f();
    off = [];
    stop();
  };
}
