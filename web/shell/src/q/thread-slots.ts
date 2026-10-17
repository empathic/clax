// The questions asked in each thread, inside that thread's card (spec
// 2026-10-16-ask-where-attention-is-design §2): each `ThreadCard`'s
// `.thread-q-slot` gets a `ThreadQuestions`. Thread cards come and go with
// the threads, so a MutationObserver finds slots added and removed. The
// question module loads this with the first question asked in an open
// thread of the artifact shown.
import { mount, unmount } from "svelte";
import ThreadQuestions from "./ThreadQuestions.svelte";

/** Fills `doc`'s thread slots for artifact `aid`, now and as they come; the result stops and empties them. */
export function followSlots(doc: Document, aid: string): () => void {
  const shown = new Map<HTMLElement, Record<string, unknown>>();
  const sweep = () => {
    for (const el of doc.querySelectorAll<HTMLElement>(".thread-q-slot[data-slot-thread]")) {
      if (!shown.has(el)) shown.set(el, mount(ThreadQuestions, { target: el, props: { aid, tid: el.dataset.slotThread! } }));
    }
    for (const [el, c] of shown) if (!el.isConnected) { void unmount(c); shown.delete(el); }
  };
  // Only an element added or removed can add or remove a slot.
  const elements = (ns: NodeList) => [...ns].some(n => n.nodeType === Node.ELEMENT_NODE);
  const watch = new MutationObserver(rs => { if (rs.some(r => elements(r.addedNodes) || elements(r.removedNodes))) sweep(); });
  watch.observe(doc.body, { childList: true, subtree: true });
  sweep();
  return () => {
    watch.disconnect();
    for (const c of shown.values()) void unmount(c);
    shown.clear();
  };
}
