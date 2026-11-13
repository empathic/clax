// A Settings screen is shown once it has its first data, as the gallery's
// other pages are (ui/Gallery.svelte keeps the skeleton until then). Each
// part of a screen that fetches what it first shows holds the reveal while
// it does (`holdReveal`, called as the part is set up) and lets go once
// its first answer, or its failure, is on screen; Settings (the `Reveal`
// it provides) reveals the screen once nothing holds it. A part set up
// outside Settings holds nothing.
import { getContext, setContext, tick } from "svelte";

const KEY = Symbol("clax.settings.reveal");

/** What a screen's parts hold, and the reveal once none does. */
export class Reveal {
  #held = 0;
  #armed = false;
  #done = false;
  constructor(private readonly ready: () => void) {}
  /** Holds the reveal; the answer lets go (once). */
  hold(): () => void {
    if (this.#done) return () => {};
    this.#held++;
    let let_go = false;
    return () => {
      if (let_go) return;
      let_go = true;
      this.#held--;
      this.#check();
    };
  }
  /** The screen is mounted: from now on, the reveal comes once nothing holds it. */
  arm(): void {
    this.#armed = true;
    this.#check();
  }
  // Checked after the DOM settles, so a part that a release mounts holds first.
  #check(): void {
    if (this.#done || !this.#armed || this.#held) return;
    void tick().then(() => {
      if (this.#done || this.#held) return;
      this.#done = true;
      this.ready();
    });
  }
}

/** Makes `r` the reveal the components set up under the caller hold. */
export function provideReveal(r: Reveal): void {
  setContext(KEY, r);
}

/** Holds the screen's reveal until the answer is called (a no-op outside Settings). */
export function holdReveal(): () => void {
  return (getContext(KEY) as Reveal | undefined)?.hold() ?? (() => {});
}
