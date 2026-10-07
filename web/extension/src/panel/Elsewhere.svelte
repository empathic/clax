<script lang="ts">
  // "Elsewhere on this site" (owner decisions 2026-10-06, 2026-10-07): the
  // threads of the site's other pages, grouped under each page's path (a
  // merged page under its pattern) with its open, addressed and resolved
  // counts, newest activity first; each group collapsible. A card is the
  // shell's thread card, folded to its summary: it opens in place, where the
  // thread is read, answered, resolved or reopened and sent to the agent
  // without the tab moving; "Go to page ↗" opens it on its own page in this
  // tab. An open card asks for its page's live agents (its Send picker, as on
  // that page) and versions (its comments' tags), and again when hovered or
  // focused once that read is 30 s old, so the picker follows agents that
  // came or went; a read that failed says so beside Send, with Retry. Any number of cards may be
  // open at once. "Pinned here" says its anchor was found on this screen
  // too. Everything shown is text.
  import type { AnchorResult } from "../../../bridge/src/protocol";
  import { relativeTime } from "../../../shell/src/format";
  import { type Thread, type Viewer, resolvedByLabel } from "../../../shell/src/threads";
  import SendButton from "../../../shell/src/ui/SendButton.svelte";
  import ThreadCard from "../../../shell/src/ui/ThreadCard.svelte";
  import { ticker } from "../../../shell/src/ui/ticker.svelte";
  import { agentName, historyOf } from "../../../shell/src/view/history-model";
  import { agentNames } from "../../../shell/src/view/working-model";
  import type { FarPage } from "../messages";
  import MoveTo from "./MoveTo.svelte";
  import { type Group, type Status, lastActivity, statusOf } from "./site-model";

  type Target = { url: string; label: string };
  type Props = {
    groups: Group[]; resolved: Record<string, AnchorResult>; selected: string | null; collapsed: string[];
    /** Fixed clock for tests; without it the times tick every 30 s. */
    now?: Date;
    /** The owner, to name it on threads it resolved. */
    me?: Viewer | null;
    /** The threads open in place, by ID: the panel keeps them, so a filter that empties this list does not fold them. */
    unfolded: string[];
    onFold(id: string, open: boolean): void;
    /** The pages of open cards, by artifact ID, as the worker answered. */
    pages: Record<string, FarPage>;
    /** The pages whose agents could not be read: Send says so, with Retry. */
    failed: Record<string, boolean>;
    /** An open card is hovered or focused: its page's agents may be read again. */
    onFreshen(t: Thread): void;
    /** The agent the person picked in a Send menu, which a Send names when it is live on the thread's page. */
    chosen: string | null;
    targets(t: Thread): Target[];
    onToggle(label: string, open: boolean): void;
    /** "Go to page ↗": the tab opens the thread's page. */
    onOpen(t: Thread): void;
    /** A card opened: its page's agents and versions are wanted. */
    onUnfold(t: Thread): void;
    onMove(t: Thread, url: string): void;
    onReply(t: Thread, body: string): void;
    /** Resolves an open thread, reopens a resolved one. */
    onResolve(t: Thread): void;
    /** Sends to the agent `to` names (null: none is live on its page). */
    onSend(t: Thread, to: string | null): void;
    onChoose(handle: string): void;
    /** An open card was looked at. */
    onSeen(t: Thread): void;
    clip(t: Thread): Promise<string | null>;
  };
  let { groups, resolved, selected, collapsed, now, me = null, unfolded, onFold, pages, failed, onFreshen, chosen, targets, onToggle, onOpen, onUnfold, onMove, onReply, onResolve, onSend, onChoose, onSeen, clip }: Props = $props();
  let moving = $state<string | null>(null);
  const clock = ticker(() => false, () => now, 30_000);
  const STATUS: Record<Status, string> = { open: "Open", addressed: "Addressed", resolved: "Resolved" };
  const total = $derived(groups.reduce((n, g) => n + g.threads.length, 0));
  const reveal = (el: HTMLElement, on: boolean) => {
    const go = (v: boolean) => { if (v) el.scrollIntoView?.({ block: "nearest" }); };
    go(on);
    return { update: go };
  };
  const fold = (t: Thread) => ({
    open: unfolded.includes(t.id),
    onToggle: (o: boolean) => {
      onFold(t.id, o);
      if (o) { asked.add(t.id); onUnfold(t); } else asked.delete(t.id);
    },
  });
  // A card open since before this list (kept open) asks for its page too.
  const asked = new Set<string>();
  $effect(() => {
    for (const g of groups) for (const t of g.threads) if (unfolded.includes(t.id) && !asked.has(t.id)) { asked.add(t.id); onUnfold(t); }
  });
  const names = (t: Thread) => (by: string) => by.startsWith("agent:") ? agentName(by.slice(6)) : resolvedByLabel(by, me, t.resolved_by_name);
  const versionsOf = (t: Thread) => pages[t.artifact_id]?.versions ?? [];
  const agentsOf = (t: Thread) => pages[t.artifact_id]?.agents ?? [];
  /** As on the thread's own page: the agent picked, when live there, else its first live agent. */
  const targetOf = (t: Thread) => {
    const live = agentsOf(t).filter(a => a.live);
    return live.some(a => a.handle === chosen) ? chosen : (live[0]?.handle ?? null);
  };
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
        <!-- Hovered or focused while open: its page's agents are read again when stale. -->
        <div class="far" use:reveal={t.id === selected} role="presentation"
          onpointerenter={() => { if (unfolded.includes(t.id)) onFreshen(t); }} onfocusin={() => { if (unfolded.includes(t.id)) onFreshen(t); }}>
          <ThreadCard {t} now={clock.now} {me} {selected} file={t.anchor.file} history={historyOf(t, versionsOf(t), names(t))} outdated={false} agent="agent"
            versions={versionsOf(t)} when={relativeTime(lastActivity(t), clock.now)} fold={fold(t)} {clip} {onSeen}
            onSelect={noop} onSend={x => onSend(x, targetOf(x))} {onResolve} {onReply}>
            {#snippet badge()}<span class={`state ${st}`}>{STATUS[st]}</span>{/snippet}
            {#snippet meta()}
              {#if g.page.merged && t.page_path}<span class="at" title={t.page_path}>at {t.page_path}</span>{/if}
              {#if resolved[t.id]?.found}<span class="here">Pinned here</span>{/if}
            {/snippet}
            {#snippet send(guard: (e: Event, act: () => void) => void)}
              {@const known = !!pages[t.artifact_id]}
              {@const by = agentNames([], agentsOf(t))}
              {@const to = targetOf(t)}
              <SendButton label={`Send to ${(to && by.get(to)) || "agent"}`} agents={agentsOf(t)} names={by} target={to} disabled={!known}
                onSend={e => guard(e, () => onSend(t, to))} {onChoose} />
              {#if !known && failed[t.artifact_id]}
                <span class="far-err" role="status">Could not load this page's agents.
                  <button type="button" class="ghost" onclick={() => onUnfold(t)}>Retry</button></span>
              {/if}
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
  .far-err { flex-basis: 100%; text-align: right; font-size: 12px; color: var(--muted); }
  .far :global(.actions button) { min-height: 26px; padding: 2px 8px; font-size: 12.5px; }
  .far :global(.actions button.ghost) { color: var(--muted); }
</style>
