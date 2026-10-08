<script lang="ts">
  // The gallery's unread summary (spec 2026-10-06-agent-questions-and-inbox
  // §9.3), above "Needs your eyes": "Inbox · N unread", the two oldest open
  // questions as cards (full width, previews beside the options) and "N more
  // questions in the inbox" (the inbox's questions), the five newest other
  // unread items as rows, then "N more in the inbox". Nothing shows
  // while nothing is unread and no card is closing, or for anyone but the
  // owner. Its styles are QuestionList's.
  import type { InboxItem } from "../api";
  import type { InboxFeed, QuestionFeed } from "./feed.svelte";
  import InboxRow from "./InboxRow.svelte";
  import { openItem } from "./open";
  import QuestionList from "./QuestionList.svelte";
  import { shared } from "./shared";

  const p: { inbox?: InboxFeed; questions?: QuestionFeed; go?: (url: string) => void; now?: Date } = $props();
  const inbox = $derived(p.inbox ?? shared().inbox);
  const questions = $derived(p.questions ?? shared().questions);
  const cards = $derived(questions.cards.slice(0, 2));
  const moreQ = $derived(questions.cards.length - cards.length);
  const rows = $derived(inbox.latest);
  // Open questions are unread items too.
  const more = $derived(Math.max(0, inbox.unread - questions.open.length - rows.length));
  const shown = $derived(inbox.owner && (inbox.unread > 0 || cards.length > 0));
  const open = (i: InboxItem) => void openItem(inbox, i, p.go);
  const toggle = (i: InboxItem) => void inbox.mark(i.id, !i.read).catch(() => {});
</script>

<section class="grp inbox-sum" aria-labelledby="inbox-sum-h" hidden={!shown}>
  <h2 id="inbox-sum-h"><span class="sw" aria-hidden="true"></span><a href="/inbox">Inbox</a> <span class="n">· {inbox.unread} unread</span></h2>
  <QuestionList list={cards} feed={questions} now={p.now} />
  {#if moreQ > 0}<a class="more" href="/inbox?kind=question">{moreQ} more {moreQ === 1 ? "question" : "questions"} in the inbox</a>{/if}
  {#if rows.length}
    <ul class="rows">{#each rows as i (i.id)}<InboxRow item={i} others={rows} now={p.now} onOpen={open} onToggle={toggle} />{/each}</ul>
  {/if}
  {#if more > 0}<a class="more" href="/inbox">{more} more in the inbox</a>{/if}
</section>
