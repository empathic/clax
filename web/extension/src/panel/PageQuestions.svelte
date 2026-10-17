<script lang="ts">
  // The Page view's open questions about the tab's page (spec
  // 2026-10-06-agent-questions-and-inbox §9.6), above its threads: every
  // one a card, a question asked in a thread too, as no thread card in the
  // panel shows its questions; its card links to its thread (spec
  // 2026-10-16-ask-where-attention-is-design §2). A closed one says what
  // closed it for 4 s, then leaves; the block stays (hidden) while nothing
  // is open, so a card leaving with focus can hand it on. Its styles are
  // QuestionList's.
  import type { QuestionFeed } from "../../../shell/src/q/feed.svelte";
  import QuestionList from "../../../shell/src/q/QuestionList.svelte";

  const p: { aid: string; feed: QuestionFeed; now?: Date } = $props();
  const list = $derived(p.feed.byArtifact(p.aid));
  const open = $derived(list.filter(q => q.status === "open").length);
</script>

<section class="side-q" aria-labelledby="side-q-h" hidden={!list.length}>
  {#if list.length}<h2 class="gh ag" id="side-q-h"><span class="sw" aria-hidden="true"></span><span class="t">Questions for you</span> <span class="c">{open}</span></h2>{/if}
  <QuestionList {list} feed={p.feed} here={p.aid} now={p.now} />
</section>
