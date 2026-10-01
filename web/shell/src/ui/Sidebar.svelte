<script lang="ts">
  // Open threads: those on the page shown and found (numbered like the pins),
  // then those on other pages of the version (labelled "on <file>"); open
  // threads on the page shown and not found, or on a page the version does not
  // hold (Detached); then resolved threads.
  import type { Snippet } from "svelte";
  import type { AnchorResult } from "../../../bridge/src/protocol";
  import type { Thread, Viewer } from "../threads";
  import { needsTicking, sidebarSections } from "../view/sidebar-model";
  import ThreadCard from "./ThreadCard.svelte";
  import { ticker } from "./ticker.svelte";

  type Props = {
    threads: Thread[];
    resolved: Record<string, AnchorResult>;
    /** Fixed clock for tests; without it the sidebar ticks each second while a label shows elapsed time. */
    now?: Date;
    selected: string | null;
    onSelect(t: Thread): void;
    onSend(t: Thread): void;
    onResolve(t: Thread): void;
    onReply(t: Thread, body: string): void;
    /** Hears the thread whose card the pointer is over, and null when it leaves. */
    onHover?(t: Thread | null): void;
    /** This viewer, to show its own name on threads it resolved. */
    me?: Viewer | null;
    /** Rendered above the sections (the "Your name" field on narrow screens). */
    header?: Snippet;
    /** The page the frame shows (the index by default; null when the frame shows
     * a document that did not greet); threads on other pages are labelled with theirs. */
    file?: string | null;
    /** Whether the shown version holds a page; a thread on a page it lacks is detached. */
    holds?: (file: string) => boolean;
  };
  let p: Props = $props();
  const clock = ticker(() => needsTicking(p.threads), () => p.now);
  const s = $derived(sidebarSections(p.threads, p.resolved, p.file, p.holds));
</script>

{#snippet section(cls: string, title: string, list: Thread[])}
  <section class={cls}>
    <h2>{title} <span class="muted">{list.length}</span></h2>
    {#if list.length === 0}
      <p class="muted small">None.</p>
    {:else}
      {#each list as t (t.id)}
        <ThreadCard {t} n={s.numbers.get(t.id)} now={clock.now} me={p.me} selected={p.selected} file={s.file}
          onSelect={p.onSelect} onSend={p.onSend} onResolve={p.onResolve} onReply={p.onReply} onHover={p.onHover} />
      {/each}
    {/if}
  </section>
{/snippet}

<aside class="sidebar" aria-label="Comment threads">
  {@render p.header?.()}
  {@render section("section-open", "Open", s.open)}
  {@render section("section-detached", "Detached", s.detached)}
  {@render section("section-resolved", "Resolved", s.resolved)}
</aside>
