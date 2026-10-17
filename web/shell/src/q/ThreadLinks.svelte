<svelte:options css="injected" />

<script lang="ts">
  // The sidebar's links to the questions asked in open threads (spec
  // 2026-10-16-ask-where-attention-is-design §2): who asks and the
  // questions' headers. A link to a thread whose card is on this page goes
  // there: a folded card (a thread of another page) opens first, and once
  // its question shows, the card scrolls into view and the question takes
  // focus. With no card on the page it navigates to `?thread=`, which the
  // view opens.
  import type { QuestionView } from "../api";
  import { after } from "../clock";
  import { agentLabel, cutHeader } from "./model";
  import { terminalLine } from "./terminal";

  /** How long a link waits for an opened card to show its question. */
  const OPEN_MS = 3000;
  const p: { list: QuestionView[] } = $props();
  let stop = () => {};
  $effect(() => () => stop());
  const go = (e: MouseEvent, tid: string) => {
    const card = [...document.querySelectorAll<HTMLElement>(".thread-card")].find(c => c.dataset.thread === tid);
    if (!card) return;
    e.preventDefault();
    stop();
    const show = (): boolean => {
      const slot = card.querySelector<HTMLElement>(".thread-q-slot");
      const q = slot?.querySelector<HTMLElement>(".qcard");
      if (!q) return false;
      slot!.scrollIntoView({ block: "nearest" });
      q.focus();
      return true;
    };
    if (card.classList.contains("folded")) card.querySelector<HTMLButtonElement>(".card-head")?.click();
    if (show()) return;
    const watch = new MutationObserver(() => { if (show()) stop(); });
    const cancel = after(OPEN_MS, () => stop());
    stop = () => { watch.disconnect(); cancel(); stop = () => {}; };
    watch.observe(card, { childList: true, subtree: true });
  };
</script>

{#each p.list as q (q.id)}
  <a class="q-thread" href={`?thread=${encodeURIComponent(q.thread!.id)}`} onclick={e => go(e, q.thread!.id)}>{q.in_terminal ? terminalLine(q, []) : `${agentLabel(q.agent, [])} asks in a thread`}: {q.questions.map(s => cutHeader(s.header)).join(", ")}</a>
{/each}

<style>
  :global { .side-q .q-thread { display: block; margin: 0 0 8px; font: 500 13px/18px var(--font); color: var(--accent-ink); overflow-wrap: anywhere; text-underline-offset: 3px; } }
</style>
