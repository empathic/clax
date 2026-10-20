// The popovers under the artifact's tag buttons (`view/title-tags.ts`): the
// tag editor under Edit tags, and the list of every tag it carries under
// "+N" or "N tags". Their own chunk, loaded on the first press. One is open
// at a time; its button toggles it, and a press outside it, focus leaving
// it, or the view ending (or its button going) closes it. Escape closes it
// with focus back on the button.
import { mount, unmount } from "svelte";
import type { ArtifactController } from "../view/artifact-controller";
import { tagChip, tagsOf } from "../view/title-tags";
import TagEditor from "./TagEditor.svelte";

let open: { button: HTMLButtonElement; close(): void } | null = null;

/** Places `pop` under `button`, inside the window: 8 px from its edges at most. */
function place(pop: HTMLElement, button: HTMLElement): void {
  const r = button.getBoundingClientRect();
  const width = Math.min(280, innerWidth - 16);
  pop.style.width = `${width}px`;
  pop.style.top = `${r.bottom + 6}px`;
  pop.style.left = `${Math.max(8, Math.min(r.left, innerWidth - width - 8))}px`;
}

/** Opens a popover named `label` under `button`, filled by `fill` (whose
 * result empties it, given how to close it), or closes it when it is open
 * there. */
function toggle(button: HTMLButtonElement, ctl: ArtifactController, label: string, fill: (pop: HTMLElement, close: (refocus: boolean) => void) => () => void): void {
  const was = open?.button;
  open?.close();
  if (was === button) return;
  const pop = document.createElement("div");
  pop.className = "tag-pop";
  pop.setAttribute("role", "dialog");
  pop.setAttribute("aria-label", label);
  place(pop, button);
  document.body.append(pop);
  let empty = () => {};
  let offState = () => {};
  const close = (refocus: boolean) => {
    if (open?.button !== button) return;
    open = null;
    removeEventListener("pointerdown", outside, true);
    removeEventListener("resize", replace);
    offState();
    empty();
    pop.remove();
    button.setAttribute("aria-expanded", "false");
    if (refocus && button.isConnected) button.focus();
  };
  const outside = (e: PointerEvent) => { if (!pop.contains(e.target as Node) && !button.contains(e.target as Node)) close(false); };
  const replace = () => place(pop, button);
  open = { button, close: () => close(false) };
  empty = fill(pop, close);
  pop.addEventListener("keydown", e => { if (e.key === "Escape" && open?.button === button) { e.preventDefault(); e.stopPropagation(); close(true); } });
  pop.addEventListener("focusout", e => {
    const to = e.relatedTarget as Node | null;
    if (to && !pop.contains(to) && to !== button) close(false);
  });
  // The view ended, or the button went (the viewer may no longer tag).
  offState = ctl.state.subscribe(() => { if (ctl.disposed || !button.isConnected) queueMicrotask(() => close(false)); });
  button.setAttribute("aria-expanded", "true");
  addEventListener("pointerdown", outside, true);
  addEventListener("resize", replace);
}

/** Opens the tag editor of `ctl`'s artifact under `button`, or closes it when it is open there. */
export function toggleTagEditor(button: HTMLButtonElement, ctl: ArtifactController): void {
  toggle(button, ctl, "Edit tags", (pop, close) => {
    const view = mount(TagEditor, { target: pop, props: { ctl, onClose: () => close(true) } });
    return () => { void unmount(view); };
  });
}

/** Opens the list of every tag `ctl`'s artifact carries under `button`
 * (each a link to the gallery filtered by it when `reach`), following them
 * as they change, or closes it when it is open there. */
export function toggleTagList(button: HTMLButtonElement, ctl: ArtifactController, reach: boolean): void {
  toggle(button, ctl, "Tags", pop => {
    const list = document.createElement("div");
    list.className = "tlist";
    pop.append(list);
    let key = "";
    const off = ctl.state.subscribe(s => {
      const tags = tagsOf(s).tags;
      if (JSON.stringify(tags) === key) return;
      key = JSON.stringify(tags);
      list.replaceChildren(...tags.map(t => tagChip(document, t, reach)));
    });
    // Focus goes in, so the keyboard reaches the links and Escape.
    pop.tabIndex = -1;
    queueMicrotask(() => (list.querySelector<HTMLElement>("a") ?? pop).focus());
    return off;
  });
}
