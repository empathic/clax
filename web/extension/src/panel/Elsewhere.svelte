<script lang="ts">
  // "Elsewhere on this site" (owner decisions 2026-10-06, 2026-10-07): the
  // threads of the site's other pages, grouped under each page's path (a
  // merged page under its pattern) with its open, addressed and resolved
  // counts, newest activity first; each group collapsible. A card is the
  // shell's thread card, folded to its summary: it opens in place, where the
  // thread is read, answered, resolved or reopened and sent to the agent
  // without the tab moving; "Go to page ↗" opens it on its own page in this
  // tab. Any number of cards may be open at once, kept by thread ID while
  // the listing changes. "Pinned here" says its anchor was found on this
  // screen too. Everything shown is text.
  import type { AnchorResult } from "../../../bridge/src/protocol";
  import { relativeTime } from "../../../shell/src/format";
  import { type Thread, type Viewer, resolvedByLabel } from "../../../shell/src/threads";
  import ThreadCard from "../../../shell/src/ui/ThreadCard.svelte";
  import { agentName, historyOf } from "../../../shell/src/view/history-model";
  import MoveTo from "./MoveTo.svelte";
  import { type Group, type Status, lastActivity, statusOf } from "./site-model";

  type Target = { url: string; label: string };
  type Props = {
    groups: Group[]; resolved: Record<string, AnchorResult>; selected: string | null; collapsed: string[]; now?: Date;
    /** The owner, to name it on threads it resolved. */
    me?: Viewer | null;
    targets(t: Thread): Target[];
    onToggle(label: string, open: boolean): void;
    /** "Go to page ↗": the tab opens the thread's page. */
    onOpen(t: Thread): void;
    onMove(t: Thread, url: string): void;
    onReply(t: Thread, body: string): void;
    /** Resolves an open thread, reopens a resolved one. */
    onResolve(t: Thread): void;
    onSend(t: Thread): void;
    clip(t: Thread): Promise<string | null>;
  };
  let { groups, resolved, selected, collapsed, now, me = null, targets, onToggle, onOpen, onMove, onReply, onResolve, onSend, clip }: Props = $props();
  let moving = $state<string | null>(null);
  let unfolded = $state<string[]>([]);
  const STATUS: Record<Status, string> = { open: "Open", addressed: "Addressed", resolved: "Resolved" };
  const total = $derived(groups.reduce((n, g) => n + g.threads.length, 0));
  const reveal = (el: HTMLElement, on: boolean) => {
    const go = (v: boolean) => { if (v) el.scrollIntoView?.({ block: "nearest" }); };
    go(on);
    return { update: go };
  };
  const fold = (t: Thread) => ({
    open: unfolded.includes(t.id),
    onToggle: (o: boolean) => { unfolded = o ? [...unfolded, t.id] : unfolded.filter(x => x !== t.id); },
  });
  const names = (t: Thread) => (by: string) => by.startsWith("agent:") ? agentName(by.slice(6)) : resolvedByLabel(by, me, t.resolved_by_name);
  // The panel holds only the tab's page's versions: an event of another
  // page is tagged with a version only where the thread names it (the
  // version it was made on, those that addressed it).
  const history = (t: Thread) => historyOf(t, [], names(t)).map(e => (e.verb === "commented" || e.agent ? e : { ...e, v: null }));
  const noop = () => {};
</script>

<section class="elsewhere" aria-label="Elsewhere on this site">
  <h2 class="eh"><span class="t">Elsewhere on this site</span> <span class="c">{total}</span></h2>
  {#each groups as g (g.page.artifact_id)}
    {@const open = !collapsed.includes(g.label) || g.threads.some(t => t.id === selected)}
    <details class="group" {open} ontoggle={e => { if (e.currentTarget.open !== open) onToggle(g.label, e.currentTarget.open); }}>
      <summary>
        <span class="caret" aria-hidden="true"></span>
        <span class="path" title={g.label}>{g.label}</span>
        {#if g.page.merged}<span class="tag">merged</span>{:else}<span></span>{/if}
        <span class="counts">
          {#each ["open", "addressed", "resolved"] as const as k (k)}
            {#if g.counts[k]}<span class={`n ${k}`}>{g.counts[k]} {k}</span>{/if}
          {/each}
        </span>
      </summary>
      {#each g.threads as t (t.id)}
        {@const st = statusOf(t)}
        <div class="far" use:reveal={t.id === selected}>
          <ThreadCard {t} now={now ?? new Date()} {me} {selected} file={t.anchor.file} history={history(t)} outdated={false} agent="agent"
            when={relativeTime(lastActivity(t), now)} fold={fold(t)} {clip}
            onSelect={noop} {onSend} {onResolve} {onReply}>
            {#snippet badge()}<span class={`state ${st}`}>{STATUS[st]}</span>{/snippet}
            {#snippet meta()}
              {#if g.page.merged && t.page_path}<span class="at" title={t.page_path}>at {t.page_path}</span>{/if}
              {#if resolved[t.id]?.found}<span class="here">Pinned here</span>{/if}
            {/snippet}
            {#snippet tools()}
              <button type="button" class="ghost" onclick={() => (moving = moving === t.id ? null : t.id)}>Move…</button>
              <button type="button" class="ghost" aria-label={`Go to page ${t.page_path ?? g.page.path}`} onclick={() => onOpen(t)}>Go to page ↗</button>
            {/snippet}
          </ThreadCard>
          {#if moving === t.id}
            <MoveTo targets={targets(t)} onMove={url => { moving = null; onMove(t, url); }} onCancel={() => (moving = null)} />
          {/if}
        </div>
      {/each}
    </details>
  {/each}
</section>

<style>
  .elsewhere { padding: 4px var(--gutter) 12px; border-top: 1px solid var(--border); }
  .eh { display: flex; align-items: center; gap: 8px; margin: 12px 2px 8px; font: 600 14px/1.2 var(--font); }
  .eh .t { flex: 1; } .eh .c, .counts { font: 400 12px var(--mono); color: var(--muted); font-variant-numeric: tabular-nums; }
  .group { margin-bottom: 6px; }
  summary { display: grid; grid-template-columns: 10px minmax(0, 1fr) auto; align-items: center; gap: 2px 8px; padding: 6px 8px; border-radius: var(--radius-sm); cursor: pointer; list-style: none; }
  summary::-webkit-details-marker { display: none; }
  summary:hover { background: var(--hover); }
  .caret::before { content: "▸"; color: var(--muted); font-size: 11px; }
  details[open] > summary .caret::before { content: "▾"; }
  .path { font: 500 12.5px/1.4 var(--mono); min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .tag { font: 500 11px/16px var(--font); padding: 0 6px; border-radius: var(--radius-xs); background: var(--accent-tint); color: var(--agent-ink); }
  .counts { grid-column: 2 / -1; display: flex; flex-wrap: wrap; gap: 4px 10px; font-size: 11.5px; }
  .n::before { content: ""; display: inline-block; width: 7px; height: 7px; margin-right: 5px; border-radius: 50%; vertical-align: 1px; }
  .n.open::before { background: var(--you); }
  .n.addressed::before { background: var(--agent); }
  .n.resolved::before { background: var(--border-strong); }
  .far { margin: 6px 0 8px; }
  .far :global(.thread-card) { margin-bottom: 0; }
  .state { flex: none; font: 500 11px/16px var(--font); padding: 0 6px; border-radius: var(--radius-xs); background: var(--hover); color: var(--muted); }
  .state.open { background: var(--comment-hl); color: var(--you-ink); }
  .state.addressed { background: var(--accent-tint); color: var(--agent-ink); }
  .here { color: var(--you-ink); font-weight: 500; }
  .far :global(.actions button) { min-height: 26px; padding: 2px 8px; font-size: 12.5px; }
  .far :global(.actions button.ghost) { color: var(--muted); }
</style>
