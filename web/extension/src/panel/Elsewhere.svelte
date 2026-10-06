<script lang="ts">
  // "Elsewhere on this site" (owner decision 2026-10-06): the threads of the
  // site's other pages, grouped under each page's path (a merged page under
  // its pattern) with its open, addressed and resolved counts, newest
  // activity first; each group collapsible. A card opens its thread on its
  // own page in this tab; "Pinned here" says its anchor was found on this
  // screen too. Everything shown is text.
  import type { AnchorResult } from "../../../bridge/src/protocol";
  import { relativeTime } from "../../../shell/src/format";
  import { type Thread, anchorLabel } from "../../../shell/src/threads";
  import MoveTo from "./MoveTo.svelte";
  import { type Group, type Status, lastActivity, statusOf } from "./site-model";

  type Target = { url: string; label: string };
  type Props = {
    groups: Group[]; resolved: Record<string, AnchorResult>; selected: string | null; collapsed: string[]; now?: Date;
    targets(t: Thread): Target[];
    onToggle(label: string, open: boolean): void;
    onOpen(t: Thread): void;
    onMove(t: Thread, url: string): void;
  };
  let { groups, resolved, selected, collapsed, now, targets, onToggle, onOpen, onMove }: Props = $props();
  let moving = $state<string | null>(null);
  const STATUS: Record<Status, string> = { open: "Open", addressed: "Addressed", resolved: "Resolved" };
  const total = $derived(groups.reduce((n, g) => n + g.threads.length, 0));
  const replies = (t: Thread) => t.comments.length - 1;
  const reveal = (el: HTMLElement, on: boolean) => {
    const go = (v: boolean) => { if (v) el.scrollIntoView?.({ block: "nearest" }); };
    go(on);
    return { update: go };
  };
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
        <article class="far" class:selected={t.id === selected} use:reveal={t.id === selected}>
          <button type="button" class="go" onclick={() => onOpen(t)} title={`Open on ${t.page_path ?? g.page.path}`}>
            <span class="top">
              <span class="anchor">{anchorLabel(t.anchor)}</span>
              <span class={`st ${st}`}>{STATUS[st]}</span>
            </span>
            <span class="body">{t.comments[0]?.body ?? ""}</span>
            <span class="meta">
              <span>{t.comments[0]?.author_name ?? ""}</span>
              {#if replies(t) > 0}<span>{replies(t)} {replies(t) === 1 ? "reply" : "replies"}</span>{/if}
              <span>{relativeTime(lastActivity(t), now)}</span>
              {#if g.page.merged && t.page_path}<span class="at" title={t.page_path}>at {t.page_path}</span>{/if}
              {#if resolved[t.id]?.found}<span class="here">Pinned here</span>{/if}
            </span>
          </button>
          {#if moving === t.id}
            <MoveTo targets={targets(t)} onMove={url => { moving = null; onMove(t, url); }} onCancel={() => (moving = null)} />
          {:else}
            <div class="tools"><button type="button" class="ghost" onclick={() => (moving = t.id)}>Move…</button></div>
          {/if}
        </article>
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
  .far { margin: 6px 0 8px; padding: 10px 12px; background: var(--card); border: 1px solid var(--border); border-radius: var(--radius); }
  .far.selected { border-color: var(--border-strong); box-shadow: var(--elev); }
  .go { display: grid; gap: 4px; width: 100%; min-height: 0; padding: 0; border: 0; background: none; text-align: left; white-space: normal; color: var(--fg); font: 400 13.5px/1.5 var(--font); }
  .go:not(:disabled):hover { background: none; }
  .top { display: flex; gap: 8px; align-items: center; min-width: 0; }
  .anchor { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--muted); font-size: 12.5px; }
  .st { flex: none; font: 500 11px/16px var(--font); padding: 0 6px; border-radius: var(--radius-xs); background: var(--hover); color: var(--muted); }
  .st.open { background: var(--comment-hl); color: var(--you-ink); }
  .st.addressed { background: var(--accent-tint); color: var(--agent-ink); }
  .body { display: -webkit-box; -webkit-line-clamp: 3; line-clamp: 3; -webkit-box-orient: vertical; overflow: hidden; overflow-wrap: anywhere; white-space: pre-wrap; }
  .meta { display: flex; flex-wrap: wrap; gap: 2px 10px; font-size: 12px; color: var(--muted); min-width: 0; }
  .meta > span { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; max-width: 100%; }
  .here { color: var(--you-ink); font-weight: 500; }
  .tools { display: flex; justify-content: flex-end; margin-top: 2px; }
  .tools button { min-height: 26px; padding: 2px 8px; font-size: 12.5px; color: var(--muted); }
</style>
