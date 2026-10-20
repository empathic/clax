// The artifact's tag editor as a popover under its Edit tags button
// (`view/title-tags.ts`): its own chunk, loaded on the first press. One is
// open at a time; the button toggles it, and a press outside it, focus
// leaving it, or the view ending closes it. Escape closes it with focus
// back on the button.
import { mount, unmount } from "svelte";
import type { ArtifactController } from "../view/artifact-controller";
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

/** Opens the tag editor of `ctl`'s artifact under `button`, or closes it when it is open there. */
export function toggleTagEditor(button: HTMLButtonElement, ctl: ArtifactController): void {
  const was = open?.button;
  open?.close();
  if (was === button) return;
  const pop = document.createElement("div");
  pop.className = "tag-pop";
  pop.setAttribute("role", "dialog");
  pop.setAttribute("aria-label", "Edit tags");
  place(pop, button);
  document.body.append(pop);
  const close = (refocus: boolean) => {
    if (open?.button !== button) return;
    open = null;
    removeEventListener("pointerdown", outside, true);
    removeEventListener("resize", replace);
    offState();
    void unmount(view);
    pop.remove();
    button.setAttribute("aria-expanded", "false");
    if (refocus && button.isConnected) button.focus();
  };
  const outside = (e: PointerEvent) => { if (!pop.contains(e.target as Node) && !button.contains(e.target as Node)) close(false); };
  const replace = () => place(pop, button);
  open = { button, close: () => close(false) };
  const view = mount(TagEditor, { target: pop, props: { ctl, onClose: () => close(true) } });
  // The view ended, or the button went (the viewer may no longer tag).
  const offState = ctl.state.subscribe(() => { if (ctl.disposed || !button.isConnected) queueMicrotask(() => close(false)); });
  pop.addEventListener("focusout", e => {
    const to = e.relatedTarget as Node | null;
    if (to && !pop.contains(to) && to !== button) close(false);
  });
  button.setAttribute("aria-expanded", "true");
  addEventListener("pointerdown", outside, true);
  addEventListener("resize", replace);
}
