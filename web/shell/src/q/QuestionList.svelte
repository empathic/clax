<svelte:options css="injected" />

<script lang="ts">
  // Question cards, keyed by question (a card's draft belongs to its
  // question), answered through the feed. A card that leaves while it holds
  // focus (its closed state over) hands focus to the card that takes its
  // place, else the one before, else the nearest region around the list
  // that still shows: focus never drops to the page. Its styles include
  // those of the blocks that always hold a list: the sidebar's
  // (SidebarQuestions) and the gallery's summary (GallerySummary).
  import type { QuestionView } from "../api";
  import type { QuestionFeed } from "./feed.svelte";
  import QuestionCard from "./QuestionCard.svelte";

  const p: { list: QuestionView[]; feed: QuestionFeed; here?: string | null; now?: Date } = $props();
  // The list's element, from its focus events (no `bind:this`: see
  // `QuestionCard`), and the card that last took focus in it, by question
  // and place; forgotten when focus moves out to somewhere else.
  let box: HTMLElement | null = null;
  let held: { id: string; at: number } | null = null;
  const onIn = (e: FocusEvent) => {
    box = e.currentTarget as HTMLElement;
    const cards = [...box.children] as HTMLElement[];
    const at = cards.findIndex(c => c.contains(e.target as Node));
    held = at >= 0 ? { id: cards[at].dataset.question ?? "", at } : null;
  };
  const onOut = (e: FocusEvent) => {
    const to = e.relatedTarget as Node | null;
    if (to && !box?.contains(to)) held = null;
  };
  // A card that held focus left, and focus went nowhere: the card now in
  // its place takes it, else the one before, else the nearest shown section,
  // sidebar or main around the list (made focusable).
  $effect(() => {
    const ids = p.list.map(q => q.id);
    if (!held || !box || ids.includes(held.id)) return;
    const at = held.at;
    held = null;
    const a = document.activeElement;
    if (a && a !== document.body) return;
    const cards = [...box.children] as HTMLElement[];
    let next = cards[Math.min(at, cards.length - 1)] ?? box.closest<HTMLElement>("section:not([hidden]), aside, main");
    if (next && !next.hasAttribute("tabindex")) next.setAttribute("tabindex", "-1");
    next?.focus();
  });
</script>

<div class="qlist" onfocusin={onIn} onfocusout={onOut}>
  {#each p.list as q (q.id)}
    <QuestionCard {q} others={p.list} here={p.here} now={p.now}
      onAnswer={b => p.feed.answer(q.id, b)} onDecline={() => p.feed.decline(q.id)}
      onRelease={q.source === "hook" ? () => p.feed.release(q.id) : undefined} />
  {/each}
</div>

<style>
  :global {
    .qlist { display: grid; gap: 10px; min-width: 0; }
    .qlist:empty { display: none; }
    .side-q { margin-bottom: 14px; padding-bottom: 12px; border-bottom: 1px solid var(--border); }
    .side-q:focus { outline: none; }
    .inbox-sum h2 .sw { border-radius: 0 8px 8px 0; background: var(--you); }
    .inbox-sum h2 a { color: inherit; text-decoration: none; }
    .inbox-sum h2 a:hover { text-decoration: underline; text-underline-offset: 3px; }
    .inbox-sum h2 .n { font-weight: 400; color: var(--muted); }
    .inbox-sum .qlist { margin-top: 12px; }
    .inbox-sum .rows { margin: 8px 0 0; padding: 0; }
    .inbox-sum .more { display: inline-block; margin-top: 10px; font: 600 13px var(--font); color: var(--accent-ink); }
    .inbox-sum:focus { outline: none; }
  }
</style>
