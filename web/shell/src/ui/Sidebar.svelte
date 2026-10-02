<svelte:options css="injected" />

<script lang="ts">
  // Open threads: those on the page shown and found (numbered like the pins),
  // then those on other pages of the version (labelled "on <file>"). Below,
  // collapsed into a tail: open threads on the page shown and not found, or on
  // a page the version does not hold (Detached); then resolved threads.
  import type { Snippet } from "svelte";
  import type { AnchorResult } from "../../../bridge/src/protocol";
  import type { Version } from "../api";
  import { relativeTime } from "../format";
  import { type Thread, type Viewer, resolvedByLabel } from "../threads";
  import { agentName, historyOf, isOutdated } from "../view/history-model";
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
    /** The artifact's versions, which tag each event in a thread's history. */
    versions: Version[];
    /** The version shown, against which a thread may be outdated. */
    shown: number;
    /** The publishing agent's name. */
    agent: string;
  };
  let p: Props = $props();
  const clock = ticker(() => needsTicking(p.threads), () => p.now);
  const s = $derived(sidebarSections(p.threads, p.resolved, p.file, p.holds));
  const names = (by: string) => by.startsWith("agent:") ? agentName(by.slice(6)) : resolvedByLabel(by, p.me);
</script>

{#snippet cards(list: Thread[])}
  {#each list as t (t.id)}
    <ThreadCard {t} n={s.numbers.get(t.id)} now={clock.now} me={p.me} selected={p.selected} file={s.file}
      history={historyOf(t, p.versions, names)} outdated={isOutdated(t, p.resolved[t.id], p.shown)} agent={p.agent} when={relativeTime(t.created_at, clock.now)}
      onSelect={p.onSelect} onSend={p.onSend} onResolve={p.onResolve} onReply={p.onReply} onHover={p.onHover} />
  {/each}
{/snippet}

<aside class="sidebar" aria-label="Comment threads">
  {@render p.header?.()}
  <section class="section-open">
    <h2 class="gh you"><span class="sw" aria-hidden="true"></span><span class="t">Open</span> <span class="c">{s.open.length}</span></h2>
    {#if s.open.length === 0}<p class="muted small empty-open">Nothing open. Press C and click anything to comment on it.</p>{:else}{@render cards(s.open)}{/if}
  </section>
  <div class="tail">
    <details class="section-detached">
      <summary class="gh oth"><span class="sw" aria-hidden="true"></span><span class="t">Detached</span> <span class="c">{s.detached.length}</span></summary>
      {@render cards(s.detached)}
    </details>
    <details class="section-resolved">
      <summary class="gh set"><span class="sw" aria-hidden="true"></span><span class="t">Resolved</span> <span class="c">{s.resolved.length}</span></summary>
      {@render cards(s.resolved)}
    </details>
  </div>
</aside>

<!-- The sidebar's styles (spec §8, "Thread sidebar") travel with its lazy
     chunk (injected on mount), so the artifact entry's eager CSS does not
     carry them. The thread number's rule is the theme's, shared with the pins. -->
<style>
  :global {
    .sidebar { width: 392px; flex: none; overflow: auto; border-left: 1px solid var(--border); background: var(--bg); padding: 12px 14px 18px; display: flex; flex-direction: column; gap: 10px; }
    .gh { display: flex; align-items: center; gap: 8px; margin: 6px 2px 8px; font: 600 19px/1.1 var(--grot); list-style: none; cursor: default; }
    summary.gh { cursor: pointer; font-size: 16px; color: var(--muted); }
    summary.gh::-webkit-details-marker { display: none; }
    .gh .t { flex: 1; } .gh .c { font: 400 12px var(--mono); color: var(--muted); }
    .gh .sw { width: 7px; height: 16px; flex: none; }
    .gh.you .sw { border-radius: 0 8px 8px 0; background: var(--you); }
    .gh.ag .sw { border-radius: 8px 0 0 8px; background: var(--agent); } .gh.ag .t { color: var(--agent-ink); }
    .gh.oth .sw { border-radius: 0 8px 8px 0; box-shadow: inset 0 0 0 1.5px var(--you); }
    .gh.set .sw { border-radius: 50%; width: 10px; height: 10px; background: var(--border-strong); }
    .tail { margin-top: 6px; border-top: 1px solid var(--border); padding-top: 8px; }
    .thread-card { background: var(--card); border: 1px solid var(--border); padding: 11px 12px 10px; margin-bottom: 10px; cursor: pointer; transition: background-color var(--t); }
    .thread-card:hover { border-color: var(--border-strong); }
    .thread-card.selected { box-shadow: inset 3px 0 0 var(--accent); background: var(--comment-hl); }
    .thread-card header { margin-bottom: 8px; min-width: 0; }
    .thread-card .card-head { display: flex; gap: 8px; align-items: center; width: 100%; min-width: 0; min-height: 0; background: none; border: 0; padding: 0; color: var(--muted); text-align: left; font: 400 12px var(--mono); }
    .thread-card .anchor-label { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--fg); }
    .thread-card .thumb { display: block; max-width: 100%; max-height: 120px; min-height: 24px; margin-bottom: 6px; object-fit: cover; object-position: left top; border: 1px solid var(--border); background: var(--bg); }
    .vt { font: 600 11px/15px var(--grot); padding: 0 4px; border: 1px solid var(--border-strong); color: var(--fg); background: var(--bg); white-space: nowrap; }
    .vt.out { color: var(--muted); }
    .msg { display: grid; gap: 3px; }
    .msg + .msg { margin-top: 10px; }
    .msg .author { font: 600 14px/20px var(--grot); }
    .msg .body { margin: 0; font-size: 13.5px; line-height: 1.55; white-space: pre-wrap; overflow-wrap: anywhere; }
    .msg.you { border-left: 3px solid var(--you); padding: 1px 0 1px 10px; }
    .msg.agent { border-right: 3px solid var(--agent); padding: 1px 10px 1px 0; margin-left: 26px; text-align: right; }
    .msg.agent .author { color: var(--agent-ink); }
    .msg.agent .body { text-align: left; }
    .st { display: flex; align-items: center; gap: 8px; margin: 10px 0 0; padding-top: 8px; border-top: 1px dashed var(--border); font-size: 12px; color: var(--muted); }
    .hist { display: flex; flex-wrap: wrap; gap: 4px 10px; margin: 9px 0 0; padding-top: 8px; border-top: 1px dashed var(--border); font-size: 11.5px; color: var(--muted); line-height: 1.6; }
    .hist .ev { display: inline-flex; align-items: center; gap: 4px; white-space: nowrap; }
    .hist .ev b { font-weight: 600; color: var(--fg); }
    .hist .ev.agent .vt { border-color: var(--agent); color: var(--agent-ink); }
    .thread-card .actions { display: flex; gap: 6px; justify-content: flex-end; margin-top: 8px; }
    .reply { display: flex; gap: 6px; margin-top: 8px; }
    .reply input { flex: 1; min-width: 0; }
    @media (max-width: 700px) { .sidebar { position: absolute; inset: 0; width: auto; z-index: 10; border-left: 0; } }
  }
</style>
