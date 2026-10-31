<script lang="ts">
  // Numbered pins over the frame at the top right of the resolved rectangle
  // (for an area thread, the drawn area) of each attached open thread on
  // `file`, the page the frame shows (the index by default; none when it is
  // null, a document that did not greet). `onHover` hears the thread whose pin
  // the pointer is over, and null when it leaves.
  import { type AnchorResult, INDEX_FILE } from "../../../bridge/src/protocol";
  // The UI kit's pin (web/sdk/ui), by its file: the kit's barrel would bring
  // every kit component into the artifact page.
  import Pin from "../../../sdk/ui/Pin.svelte";
  import type { Thread } from "../threads";
  import { pinPlaces } from "../view/pins-model";
  import { ALLOW_DELAY_MS } from "../view/prompt-queue";

  type Props = {
    threads: Thread[];
    resolved: Record<string, AnchorResult>;
    onSelect(t: Thread): void;
    onHover?(t: Thread | null): void;
    /** A fixed stage width (tests); without it the stage is measured. */
    width?: number;
    file?: string | null;
    /** Threads an agent is working on: their pins are split, person and agent. */
    onit?: Set<string>;
    /** Threads the latest version addressed, by thread ID, with that version:
     * white pins with a green ring and a `vN` flag. */
    addressed?: Map<string, number>;
  };
  let { threads, resolved, onSelect, onHover, width, file = INDEX_FILE, onit, addressed }: Props = $props();
  let measured = $state(0);
  // A pin's title is its first comment as text, Markdown syntax taken
  // away (`view/markdown.ts`, loaded when the pointer first comes to a
  // pin; until then, as typed).
  let plain = $state<((s: string) => string) | null>(null);
  const titled = () => { if (!plain) void import("../view/markdown").then(m => { plain = m.plain; }, () => {}); };
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
  // A pin that appeared or moved less than ALLOW_DELAY_MS ago takes no press
  // (`.settling`): a page that places its pin under the resting pointer just
  // before the viewer's click cannot take that click, which reaches the page.
  const seen = new Map<string, { left: number; top: number; at: number }>();
  let tick = $state(0);
  const settling = $derived.by(() => {
    void tick;
    const now = performance.now();
    const out = new Set<string>();
    for (const p of places) {
      const was = seen.get(p.thread.id);
      if (!was || was.left !== p.left || was.top !== p.top) seen.set(p.thread.id, { left: p.left, top: p.top, at: now });
      if (now - seen.get(p.thread.id)!.at < ALLOW_DELAY_MS) out.add(p.thread.id);
    }
    // A pin that goes and comes back has appeared again.
    for (const id of seen.keys()) if (!places.some(p => p.thread.id === id)) seen.delete(id);
    return out;
  });
  $effect(() => {
    if (!settling.size) return;
    const t = setTimeout(() => { tick++; }, ALLOW_DELAY_MS);
    return () => clearTimeout(t);
  });
</script>

<div class="pins" {@attach measure}>
  {#each places as p (p.thread.id)}
    <Pin class="thread-pin" n={p.n} label={`Thread ${p.n}`} left={p.left} top={p.top}
      title={(b => (plain ? plain(b) : b))(p.thread.comments[0]?.body ?? "")}
      settling={settling.has(p.thread.id)} onit={onit?.has(p.thread.id)} addressed={addressed?.has(p.thread.id)}
      flag={addressed?.has(p.thread.id) ? `v${addressed.get(p.thread.id)}` : undefined}
      onclick={() => onSelect(p.thread)} onenter={() => { titled(); onHover?.(p.thread); }} onleave={() => onHover?.(null)} />
  {/each}
</div>
