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
import { need } from "./later.svelte";
import { inThread } from "./model";
import { shared } from "./shared";
import SidebarQuestions from "./SidebarQuestions.svelte";
import { type Trail, useTrail } from "./trail";
import { followBadge, followDot } from "./wire";

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

/** Starts the page's feeds (once), the count in the title and icon,
 * notifications, and the toasts of the decisions waiting for the owner's
 * confirmation (`confirm-live`, loaded once the inbox topic goes live).
 * Only a browser with an inbox loads this module. */
export function start(stream: EventStream): void {
  if (started) return;
  started = true;
  const { inbox, questions } = shared();
  inbox.start(stream);
  questions.start(stream);
  // The confirmations' feed and toasts load once the inbox topic goes live
  // (`confirm-live`), which a browser that is not the owner's never fills.
  let following = false;
  const follow = (e: { type: string }) => {
    if (following || !["ready", "resync", "confirmation"].includes(e.type)) return;
    following = true;
    void import("./confirm-live").then(m => { if (started) undo.push(m.follow(inbox)); }, () => { following = false; });
  };
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
    inbox.hear(follow),
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

/** The app's question sheet's slot (`app/host.ts` makes it, in app mode only). */
const APP_SLOT = ".app-questions > .questions-slot";

/** What the artifact view's question surfaces tell the page whenever the
 * questions about the artifact change (a `clax-questions` event on the
 * document), before their cards are rendered: the IDs of the `open` ones,
 * and of those `closed` moments ago (their cards still say what closed
 * them), oldest first. App mode's host tells the app how many are open,
 * shows the sheet for a question the app asked for, and closes it when no
 * card is left. */
export type QuestionsChange = { open: string[]; closed: string[] };

/** The artifact view's surfaces for artifact `aid`, for the owner or a connected person: **Inbox**
 * in the top bar (before the island's controls), the questions about it at
 * the top of the sidebar (in the sidebar's `.questions-slot`, each time it
 * mounts), the questions asked in each thread inside that thread's card
 * (its `.thread-q-slot`), and the dot on the threads controls. In app mode
 * (`html.app`) the sidebar's questions go in the app's sheet
 * (`.app-questions`, its slot announced the same way) instead of the hidden
 * sidebar, every question as a card there, those asked in a thread too:
 * the thread cards are in the hidden sidebar, so none is mounted in them. Each change of the questions about `aid` is told as a
 * `clax-questions` event (`QuestionsChange`). `trail` is the view's keyboard
 * trail, which its cards follow. The result removes them and stops the
 * feeds (`stop`). */
export function artifact(aid: string, trail: Trail, stream: EventStream, doc: Document = document): () => void {
  useTrail(trail);
  let off: (() => void)[] = [];
  let side: { el: Element; c: Record<string, unknown> } | null = null;
  const fill = (el: Element | null) => {
    if (!el || side?.el === el) return;
    if (side) void unmount(side.c);
    side = { el, c: mount(SidebarQuestions, { target: el, props: { aid, cardsOnly: el.matches(APP_SLOT) } }) };
  };
  const appMode = () => doc.documentElement.classList.contains("app");
  // In app mode the sheet's slot, whichever slot announced itself; else the one that did.
  const slot = (from: Element | null) => (appMode() ? doc.querySelector(APP_SLOT) : null) ?? from;
  const onSlot = (e: Event) => fill(slot(e.target as Element));
  const { questions } = shared();
  const tell = (c: QuestionsChange) => doc.dispatchEvent(new CustomEvent<QuestionsChange>("clax-questions", { detail: c }));
  const count = () => {
    const list = questions.byArtifact(aid);
    tell({ open: list.filter(q => q.status === "open").map(q => q.id), closed: list.filter(q => q.status !== "open").map(q => q.id) });
  };
  start(stream);
  // In the top bar before the controls; at phone width in the tab bar
  // instead, so the top bar keeps its title.
  for (const target of doc.querySelectorAll<HTMLElement>(".topbar, .phone-tabs")) {
    const link = mount(InboxLink, { target, anchor: target.querySelector(":scope > .island") ?? undefined, props: {} });
    off.push(() => { void unmount(link); });
  }
  doc.addEventListener("clax-questions-slot", onSlot);
  fill(slot(doc.querySelector(".sidebar > .questions-slot")));
  // The questions asked in each thread show inside its card
  // (`thread-slots`), loaded (`later`) with the first asked in an open
  // thread here.
  let slots: (() => void) | null = null;
  let live = true;
  const check = () => {
    if (slots || appMode() || !questions.byArtifact(aid).some(inThread)) return;
    slots = () => {};
    void need().then(m => { if (live) slots = m ? m.followSlots(doc, aid) : null; });
  };
  const unlisten = questions.listen(check);
  check();
  off.push(questions.listen(count), () => {
    doc.removeEventListener("clax-questions-slot", onSlot);
    if (side) void unmount(side.c);
    side = null;
    live = false;
    unlisten();
    slots?.();
    tell({ open: [], closed: [] });
  }, followDot(doc, aid));
  return () => {
    for (const f of off) f();
    off = [];
    stop();
  };
}
