// Mounts a Svelte component for a unit test, with the same exports as
// test/preact.ts. Updates are applied synchronously (flushSync), so a test
// reads the DOM right after an action, as it did under Preact's act().
import { render } from "@testing-library/svelte";
import { type Component, flushSync } from "svelte";

export type Mounted<P> = {
  root: HTMLElement;
  /** Updates the same instance's props in place, keeping its local state,
   * with `props` as its whole props, and runs its effects. */
  update(props: P): void;
  /** Removes the component and runs its teardowns. */
  unmount(): void;
};

function attach(): HTMLElement {
  const root = document.createElement("div");
  document.body.appendChild(root);
  return root;
}

/** Mounts `C` with `props` into `root` (a new div in the body by default),
 * running its effects before returning. */
export function mount<P extends Record<string, unknown>>(C: Component<P>, props: P, root: HTMLElement = attach()): Mounted<P> {
  const r = render(C, { props, target: root });
  flushSync();
  return {
    root,
    update: next => { void r.rerender(next); flushSync(); },
    unmount: () => { r.unmount(); flushSync(); },
  };
}

/** Runs `fn` (a timer advance, a click) and applies every update it caused. */
export function flush(fn?: () => void): void {
  fn?.();
  flushSync();
}
