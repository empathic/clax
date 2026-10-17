<script lang="ts">
  // The questions asked in one thread (spec
  // 2026-10-16-ask-where-attention-is-design §2), inside its card beneath
  // its last comment, with the same controls as anywhere. Hidden while none.
  import type { QuestionFeed } from "./feed.svelte";
  import QuestionList from "./QuestionList.svelte";
  import { shared } from "./shared";

  const p: { aid: string; tid: string; feed?: QuestionFeed; now?: Date } = $props();
  const feed = $derived(p.feed ?? shared().questions);
  const list = $derived(feed.byThread(p.tid));
</script>

<div class="thread-q" hidden={!list.length}><QuestionList {list} {feed} here={p.aid} now={p.now} /></div>
