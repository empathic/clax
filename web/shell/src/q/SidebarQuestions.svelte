<script lang="ts">
  // The open questions about the artifact shown (spec
  // 2026-10-06-agent-questions-and-inbox §9.5; 2026-10-16 §2), at the top of
  // its sidebar, above the threads: as cards, except those asked in an open
  // thread, which show in that thread's card and are counted and linked
  // here. A closed one says what closed it for 4 s, then leaves. The block
  // is there (hidden) while nothing is open, so a card leaving with focus
  // can hand it on; its heading shows only with a card or a link. Its
  // styles are QuestionList's.
  import type { QuestionFeed } from "./feed.svelte";
  import QuestionList from "./QuestionList.svelte";
  import { agentLabel, cutHeader, inThread, terminalLine } from "./model";
  import { shared } from "./shared";

  const p: { aid: string; feed?: QuestionFeed; now?: Date } = $props();
  const feed = $derived(p.feed ?? shared().questions);
  const all = $derived(feed.byArtifact(p.aid));
  const cards = $derived(all.filter(q => !inThread(q)));
  const threaded = $derived(all.filter(q => inThread(q) && q.status === "open"));
  const open = $derived(all.filter(q => q.status === "open").length);
  const shown = $derived(cards.length + threaded.length);
  // Scrolls to the thread's card when it is on this page; else the link navigates.
  const go = (e: MouseEvent, tid: string) => {
    const slot = document.querySelector(`.thread-q-slot[data-thread="${CSS.escape(tid)}"]`);
    if (!slot) return;
    e.preventDefault();
    slot.scrollIntoView({ block: "nearest" });
    slot.querySelector<HTMLElement>(".qcard")?.focus();
  };
</script>

<section class="side-q" aria-labelledby="side-q-h" hidden={!shown}>
  {#if shown}<h2 class="gh ag" id="side-q-h"><span class="sw" aria-hidden="true"></span><span class="t">Questions for you</span> <span class="c">{open}</span></h2>{/if}
  {#each threaded as q (q.id)}
    <a class="q-thread" href={`?thread=${encodeURIComponent(q.thread!.id)}`} onclick={e => go(e, q.thread!.id)}>{q.in_terminal ? terminalLine(q, []) : `${agentLabel(q.agent, [])} asks in a thread`}: {q.questions.map(s => cutHeader(s.header)).join(", ")}</a>
  {/each}
  <QuestionList list={cards} {feed} here={p.aid} now={p.now} />
</section>
