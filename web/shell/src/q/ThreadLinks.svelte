<svelte:options css="injected" />

<script lang="ts">
  // The sidebar's links to the questions asked in open threads (spec
  // 2026-10-16-ask-where-attention-is-design §2): who asks and the
  // questions' headers. A link scrolls to the thread's card and focuses
  // its question when the card is on this page; else it navigates to the
  // thread.
  import type { QuestionView } from "../api";
  import { agentLabel, cutHeader } from "./model";
  import { terminalLine } from "./terminal";

  const p: { list: QuestionView[] } = $props();
  const go = (e: MouseEvent, tid: string) => {
    const slot = document.querySelector(`.thread-q-slot[data-thread="${CSS.escape(tid)}"]`);
    if (!slot) return;
    e.preventDefault();
    slot.scrollIntoView({ block: "nearest" });
    slot.querySelector<HTMLElement>(".qcard")?.focus();
  };
</script>

{#each p.list as q (q.id)}
  <a class="q-thread" href={`?thread=${encodeURIComponent(q.thread!.id)}`} onclick={e => go(e, q.thread!.id)}>{q.in_terminal ? terminalLine(q, []) : `${agentLabel(q.agent, [])} asks in a thread`}: {q.questions.map(s => cutHeader(s.header)).join(", ")}</a>
{/each}

<style>
  :global { .side-q .q-thread { display: block; margin: 0 0 8px; font: 500 13px/18px var(--font); color: var(--accent-ink); overflow-wrap: anywhere; text-underline-offset: 3px; } }
</style>
