<script lang="ts">
  // The open questions about the artifact shown (spec
  // 2026-10-06-agent-questions-and-inbox §9.5), at the top of its sidebar,
  // above the threads; a closed one says what closed it for 4 s, then
  // leaves. The block is there (hidden) while nothing is open, so a card
  // leaving with focus can hand it on; its heading shows only with a card.
  // Its styles are QuestionList's.
  import type { QuestionFeed } from "./feed.svelte";
  import QuestionList from "./QuestionList.svelte";
  import { shared } from "./shared";

  const p: { aid: string; feed?: QuestionFeed; now?: Date } = $props();
  const feed = $derived(p.feed ?? shared().questions);
  const list = $derived(feed.byArtifact(p.aid));
  const open = $derived(list.filter(q => q.status === "open").length);
</script>

<section class="side-q" aria-labelledby="side-q-h" hidden={!list.length}>
  {#if list.length}<h2 class="gh ag" id="side-q-h"><span class="sw" aria-hidden="true"></span><span class="t">Questions for you</span> <span class="c">{open}</span></h2>{/if}
  <QuestionList {list} {feed} here={p.aid} now={p.now} />
</section>
