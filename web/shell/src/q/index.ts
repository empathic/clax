// The question and inbox module (spec 2026-10-06-agent-questions-and-inbox
// §9), loaded after the first paint (at once on `/inbox`), and only in a
// browser with an inbox (the owner's, or a connected person's): the feeds, the surfaces, the notifier and the tab's
// count. The surfaces show once the owner routes have answered. The page's
// stream comes from the host (the gallery's lazy module, the artifact view's
// stream), so this module does not import it: the stream's chunk stays
// shared with the code that loads it already.
import { mount, unmount } from "svelte";
import type { InboxItem } from "../api";
import type { EventStream } from "../stream";
import InboxLink from "./InboxLink.svelte";
import { setBadge } from "./badge";
import type { Notifier } from "./notify";
import { shared } from "./shared";
import SidebarQuestions from "./SidebarQuestions.svelte";
import ThreadQuestions from "./ThreadQuestions.svelte";
import { type Trail, useTrail } from "./trail";
import { followBadge, followDot } from "./wire";

export { default as GallerySummary } from "./GallerySummary.svelte";
export { default as InboxLink } from "./InboxLink.svelte";

/** Where a notification's click leads: in this tab on the gallery and the
 * inbox, which hold nothing unsaved; from an artifact view, in a new tab. */
const openUrl = (url: string) => {
  if (location.pathname === "/" || location.pathname === "/inbox") location.assign(url);
  else window.open(url, "_blank", "noopener");
};

let started = false;
// What `start` set up, undone by `stop`.
let undo: (() => void)[] = [];

/** Starts the page's feeds (once), the count in the title and icon, and
 * notifications. Only a browser with an inbox loads this module. */
export function start(stream: EventStream): void {
  if (started) return;
  started = true;
  const { inbox, questions } = shared();
  inbox.start(stream);
  questions.start(stream);
  // The notifier's code loads with the first notification asked for.
  let notifier: Notifier | undefined;
  undo.push(
    followBadge(document),
    stream.onNotify(d => {
      const i = d.item as InboxItem | undefined;
      if (i?.id) void import("./notify").then(m => { notifier ??= new m.Notifier(typeof Notification === "undefined" ? undefined : Notification, openUrl); notifier.show(i, inbox.latest); });
    }),
    inbox.listen(c => { if ("item" in c && c.item.read) notifier?.close(c.item.id); }),
    // A bulk mark says only what is left: none left, none shows.
    inbox.onCount(n => { if (!n) notifier?.closeAll(); }),
    () => setBadge(document, 0),
  );
}

/** Undoes `start`: the feeds stop following their topics, and the title and icon are as they were. */
export function stop(): void {
  const { inbox, questions } = shared();
  inbox.stop();
  questions.stop();
  for (const f of undo) f();
  undo = [];
  started = false;
}

/** The artifact view's surfaces for artifact `aid`, for the owner or a connected person: **Inbox**
 * in the top bar (before the island's controls), the questions about it at
 * the top of the sidebar (in the sidebar's `.questions-slot`, each time it
 * mounts), the questions asked in each thread inside that thread's card
 * (its `.thread-q-slot`), and the dot on the threads controls. `trail` is
 * the view's keyboard trail, which its cards follow. The result removes them and stops
 * the feeds (`stop`). */
export function artifact(aid: string, trail: Trail, stream: EventStream, doc: Document = document): () => void {
  useTrail(trail);
  let off: (() => void)[] = [];
  let side: { el: Element; c: Record<string, unknown> } | null = null;
  const fill = (el: Element | null) => {
    if (!el || side?.el === el) return;
    if (side) void unmount(side.c);
    side = { el, c: mount(SidebarQuestions, { target: el, props: { aid } }) };
  };
  const onSlot = (e: Event) => fill(e.target as Element);
  start(stream);
  // In the top bar before the controls; at phone width in the tab bar
  // instead, so the top bar keeps its title.
  for (const target of doc.querySelectorAll<HTMLElement>(".topbar, .phone-tabs")) {
    const link = mount(InboxLink, { target, anchor: target.querySelector(":scope > .island") ?? undefined, props: {} });
    off.push(() => { void unmount(link); });
  }
  doc.addEventListener("clax-questions-slot", onSlot);
  fill(doc.querySelector(".sidebar > .questions-slot"));
  // Each thread card's slot (`ThreadCard`'s `.thread-q-slot`) shows the
  // questions asked in that thread; cards come and go with the threads, so
  // a MutationObserver finds slots added and removed.
  const threads = new Map<HTMLElement, Record<string, unknown>>();
  const sweep = () => {
    for (const el of doc.querySelectorAll<HTMLElement>(".thread-q-slot[data-thread]")) {
      if (!threads.has(el)) threads.set(el, mount(ThreadQuestions, { target: el, props: { aid, tid: el.dataset.thread! } }));
    }
    for (const [el, c] of threads) if (!el.isConnected) { void unmount(c); threads.delete(el); }
  };
  // Only an element added or removed can add or remove a slot.
  const elements = (ns: NodeList) => [...ns].some(n => n.nodeType === Node.ELEMENT_NODE);
  const watch = new MutationObserver(rs => { if (rs.some(r => elements(r.addedNodes) || elements(r.removedNodes))) sweep(); });
  watch.observe(doc.body, { childList: true, subtree: true });
  sweep();
  off.push(() => {
    doc.removeEventListener("clax-questions-slot", onSlot);
    if (side) void unmount(side.c);
    side = null;
    watch.disconnect();
    for (const c of threads.values()) void unmount(c);
    threads.clear();
  }, followDot(doc, aid));
  return () => {
    for (const f of off) f();
    off = [];
    stop();
  };
}
