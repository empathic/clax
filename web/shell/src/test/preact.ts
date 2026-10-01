// Mounts a Preact component for a unit test; test/svelte.ts has the same
// exports, so a test changes one import when its component is ported.
import { type ComponentType, h, render } from "preact";
import { act } from "preact/test-utils";

export type Mounted<P> = {
  root: HTMLElement;
  /** Re-renders the same instance, keeping its local state, with `props` as
   * its whole props (a key absent from them is undefined), and runs its effects. */
  update(props: P): void;
  /** Removes the component and runs its cleanups. */
  unmount(): void;
};

function attach(): HTMLElement {
  const root = document.createElement("div");
  document.body.appendChild(root);
  return root;
}

/** Renders `C` with `props` into `root` (a new div in the body by default),
 * running its effects before returning. */
export function mount<P extends object>(C: ComponentType<P>, props: P, root: HTMLElement = attach()): Mounted<P> {
  act(() => { render(h(C, props), root); });
  return {
    root,
    update: next => act(() => { render(h(C, next), root); }),
    unmount: () => act(() => { render(null, root); }),
  };
}

/** Runs `fn` (a timer advance, a click) and applies every update it caused. */
export function flush(fn?: () => void): void {
  act(() => { fn?.(); });
}
