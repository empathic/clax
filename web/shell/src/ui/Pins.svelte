<script lang="ts">
  // Numbered pins over the frame at the top right of the resolved rectangle
  // (for an area thread, the drawn area) of each attached open thread on
  // `file`, the page the frame shows (the index by default; none when it is
  // null, a document that did not greet). `onHover` hears the thread whose pin
  // the pointer is over, and null when it leaves.
  import { type AnchorResult, INDEX_FILE } from "../../../bridge/src/protocol";
  import type { Thread } from "../threads";
  import { pinPlaces } from "../view/pins-model";

  type Props = {
    threads: Thread[];
    resolved: Record<string, AnchorResult>;
    onSelect(t: Thread): void;
    onHover?(t: Thread | null): void;
    /** A fixed stage width (tests); without it the stage is measured. */
    width?: number;
    file?: string | null;
  };
  let { threads, resolved, onSelect, onHover, width, file = INDEX_FILE }: Props = $props();
  let measured = $state(0);
  // The stage's width, followed while no fixed `width` is given.
  const measure = (el: HTMLElement) => {
    if (width !== undefined) return;
    const read = () => { measured = el.clientWidth; };
    read();
    if (typeof ResizeObserver === "function") {
      const ro = new ResizeObserver(read);
      ro.observe(el);
      return () => ro.disconnect();
    }
    addEventListener("resize", read);
    return () => removeEventListener("resize", read);
  };
  // A region scrolled wholly above the frame gets no pin; one below it is
  // clipped by `.pins`.
  const places = $derived(pinPlaces(threads, resolved, file, width ?? measured));
</script>

<div class="pins" {@attach measure}>
  {#each places as p (p.thread.id)}
    <button class="thread-pin" title={p.thread.comments[0]?.body ?? ""} aria-label={`Thread ${p.n}`} style:left={`${p.left}px`} style:top={`${p.top}px`}
      onclick={() => onSelect(p.thread)} onmouseenter={() => onHover?.(p.thread)} onmouseleave={() => onHover?.(null)}>{p.n}</button>
  {/each}
</div>
