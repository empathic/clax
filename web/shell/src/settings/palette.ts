// Opening Settings' search from anywhere in the gallery shell: the Settings
// key (Cmd+K, or Ctrl+K), or Search in Settings' navigation. The dialog
// and the list of screens are a chunk of their own, loaded on first use.
import { mount, unmount } from "svelte";

/** The event Settings takes a chosen address by when it is the page shown:
 * it moves there in place and cancels the event. Otherwise the page loads. */
export const GO = "clax:settings-go";

/** Goes to a Settings address: in place when Settings is the page, else by loading it. */
export function go(href: string): void {
  if (dispatchEvent(new CustomEvent(GO, { detail: href, cancelable: true }))) location.assign(href);
}

/** Whether a keydown is the Settings key: Cmd+K or Ctrl+K, alone. */
export const isPaletteKey = (e: KeyboardEvent): boolean =>
  (e.metaKey || e.ctrlKey) && !e.altKey && !e.shiftKey && e.key.toLowerCase() === "k";

let shown = false;

/** Opens the search dialog, unless it or another modal dialog is open. */
export function openPalette(): void {
  if (shown || document.querySelector("[aria-modal='true']")) return;
  shown = true;
  void Promise.all([import("./Palette.svelte"), import("./context").then(m => m.context())]).then(([m, ctx]) => {
    const target = document.body.appendChild(document.createElement("div"));
    const view = mount(m.default, { target, props: { ctx, onGo: go, onClose: () => { void unmount(view); target.remove(); shown = false; } } });
  }, () => { shown = false; });
}
