<svelte:options css="injected" />

<script lang="ts">
  import type { Snippet } from "svelte";
  import { isSubmitKey } from "../view/composer-model";
  import { type Thread, type Viewer, anchorLabel } from "../threads";
  import type { Version } from "../api";
  import { type HistoryEvent, addressedNote } from "../view/history-model";
  import Clip from "./Clip.svelte";
  import { SEEN_AFTER_MS, authorLabel, pageLabel } from "../view/sidebar-model";
  import { after } from "../clock";
  import { guardedAction, keyboardTrail } from "../view/trail";
  import { waitingLabel } from "../waiting";
  import { clock } from "../view/working-model";

  type Props = {
    t: Thread; n?: number; now: Date; me?: Viewer | null; selected: string | null; file: string | null;
    /** The thread's version-tagged events, oldest first. */
    history: HistoryEvent[];
    /** The thread's element changed in a later version but is still there. */
    outdated: boolean;
    /** The publishing agent's name, for `Send to <agent>`. */
    agent: string;
    /** When the thread was opened, relative to now. */
    when: string;
    /** An agent working on the thread now: replaces the waiting line. */
    marker?: { text: string; since: string } | null;
    /** The artifact's versions: an agent's reply says which one addressed the thread. */
    versions?: Version[];
    /** The card has been at least half visible for a second (decided: Q4). */
    onSeen?(t: Thread): void;
    /** The card's box is ticked for a batch send (spec §8). */
    checked?: boolean;
    /** Ticks or unticks the card; `shift` asks for the range from the last one ticked. Without it the card has no box. */
    onToggle?(t: Thread, shift: boolean): void;
    /** The shared Send button, given the guarded send for its activation; without it, a plain Send to `agent`. */
    send?: Snippet<[(e: Event, act: () => void) => void]>;
    /** A thread of another page: folded to its summary until `fold.open`,
     * opened and folded in place by its head (Enter or Space), a click on
     * the folded card, or Escape inside it; a click never selects it. */
    fold?: { open: boolean; onToggle(open: boolean): void };
    /** Beside the folded card's anchor: its status, as the caller words it. */
    badge?: Snippet;
    /** More of the folded summary's line (where it was made, whether its pin is here). */
    meta?: Snippet;
    /** The card's own tools after its actions (Move…, Go to page ↗). */
    tools?: Snippet<[Thread]>;
    /** Fetches the clip's image instead of loading `t.clip_url` (the side panel's clips come through the worker). */
    clip?(t: Thread): Promise<string | null>;
    /** `onResolve` resolves an open thread and reopens a resolved one. */
    onSelect(t: Thread): void; onSend(t: Thread): void; onResolve(t: Thread): void; onReply(t: Thread, body: string): void; onHover?(t: Thread | null): void;
  };
  let { t, n, now, selected, file, history, outdated, agent, when, marker = null, versions = [], onSeen, checked = false, onToggle, send: sendButton, fold, badge, meta, tools, clip, onSelect, onSend, onResolve, onReply, onHover }: Props = $props();
  let reply = $state("");
  // Send, Resolve and Reply are consequential: on a tainted keyboard trail
  // they take only a pointer's click (`guardedAction`), and `hint` says so
  // until the trail clears.
  let hint: string | null = $state(null);
  $effect(() => keyboardTrail.onClear(() => { hint = null; }));
  let card: HTMLElement | undefined = $state();
  let head: HTMLButtonElement | undefined = $state();
  const send = () => { if (reply.trim()) { onReply(t, reply); reply = ""; } };
  // Enter or the submit shortcut in Reply, and any activation of the Reply button.
  const replyKey = (e: KeyboardEvent) => {
    if (!isSubmitKey(e) && !(e.key === "Enter" && !e.isComposing && !e.shiftKey)) return;
    e.preventDefault();
    hint = guardedAction(e, "reply", send);
  };
  const seen = (el: HTMLElement) => {
    if (!onSeen || typeof IntersectionObserver !== "function") return;
    let cancel = () => {};
    const io = new IntersectionObserver(([e]) => {
      cancel();
      if (e.intersectionRatio >= 0.5) cancel = after(SEEN_AFTER_MS, () => onSeen(t));
    }, { threshold: [0, 0.5] });
    io.observe(el);
    return () => { cancel(); io.disconnect(); };
  };
  // A sent thread has no Send: focus on the card moves to its head first, not to <body>.
  const guardSend = (e: Event, act: () => void) => {
    hint = guardedAction(e, "send", () => { if (card?.contains(document.activeElement)) head?.focus(); act(); });
  };
  const label = $derived(t.status === "open" && t.sent_to_agent ? waitingLabel(t.feedback_state, now) : null);
  const open = $derived(!fold || fold.open);
  const replies = $derived(t.comments.length - 1);
  /** Folds or opens the card; focus left inside what folds goes to its head. */
  const toggle = (to: boolean) => { if (!to && card?.contains(document.activeElement)) head?.focus(); fold?.onToggle(to); };
  const escape = (e: KeyboardEvent) => {
    if (e.key !== "Escape" || !fold?.open || e.defaultPrevented) return;
    e.preventDefault();
    toggle(false);
  };
</script>

<!-- The card's header button is its keyboard path; a click anywhere else on the card is a pointer shortcut to the same action. -->
<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
<article class={["thread-card", selected === t.id && "selected", fold && "far", !open && "folded"]} data-thread={t.id} bind:this={card}
  onclick={() => { if (!fold) onSelect(t); else if (!fold.open) toggle(true); }} onkeydown={escape} {@attach seen}
  onmouseenter={() => onHover?.(t)} onmouseleave={() => onHover?.(null)}>
  <header>
    {#if onToggle}<input type="checkbox" class="thread-check" {checked} aria-label={`Select thread ${n ?? ""} ${anchorLabel(t.anchor)}`.replace("  ", " ")}
      onclick={e => { e.stopPropagation(); onToggle(t, e.shiftKey); }} />{/if}
    {#if fold}
      <button type="button" class="card-head" bind:this={head} aria-expanded={fold.open} aria-label={`${fold.open ? "Fold" : "Show"} the thread on ${anchorLabel(t.anchor)}`}
        onclick={e => { e.stopPropagation(); toggle(!fold.open); }}
        ><span class="fold-caret" aria-hidden="true"></span><span class="anchor-label">{anchorLabel(t.anchor)}</span>{@render badge?.()}{#if t.anchor.file !== file}<span class="file-label muted small" title={pageLabel(t.anchor.file)}>on {pageLabel(t.anchor.file)}</span>{/if}<span class="muted small">{when}</span
      ></button>
    {:else}
      <button type="button" class="card-head" bind:this={head} aria-pressed={selected === t.id} onclick={e => { e.stopPropagation(); onSelect(t); }}
        >{#if n !== undefined}<span class="thread-num">{n}</span>{/if}<span class="anchor-label">{anchorLabel(t.anchor)}</span
        >{#if outdated}<span class="vt out">outdated</span>{/if}{#if t.anchor.file !== file}<span class="file-label muted small" title={pageLabel(t.anchor.file)}>on {pageLabel(t.anchor.file)}</span>{/if}<span class="muted small">{when}</span
      ></button>
    {/if}
  </header>
  {#if !open}
    <p class="fold-body">{t.comments[0]?.body ?? ""}</p>
    <p class="fold-meta"><span>{t.comments[0] ? authorLabel(t.comments[0]) : ""}</span>{#if replies > 0}<span>{replies} {replies === 1 ? "reply" : "replies"}</span>{/if}{@render meta?.()}</p>
  {:else}
    {#if t.clip_url}{#if clip}<Clip load={() => clip(t)} />{:else}<Clip src={t.clip_url} />{/if}{/if}
    {#each t.comments as c (c.id)}
      {@const note = addressedNote(t, c, versions)}
      <div class={["msg", c.author_kind === "agent" ? "agent" : "you"]}>
        <b class="author">{authorLabel(c)}{#if note}<span class="addressed muted">{` · addressed in v${note}`}</span>{/if}{#if c.via_page}<span class="via-page muted small">{" · via the page"}</span>{/if}</b>
        <p class="body">{c.body}</p>
      </div>
    {/each}
    {#if marker}<p class="st ag"><span class="tok a work sm" aria-hidden="true"></span>{marker.text}<small>{clock(marker.since, now)}</small></p>
    {:else if label}<p class="st waiting">{label}</p>{/if}
    {#if history.length}
      <ul class="hist" aria-label="History">
        {#each history as e, i (i)}{#if i && e.v !== null}<li class="br" aria-hidden="true"></li>{/if}<li class={["ev", e.agent && "agent", i && e.v !== null && "nl"]}>{#if i}{" "}<span class="sep">·</span>{" "}{/if}{#if e.v !== null}<span class="vt">v{e.v}</span>{" "}{/if}<b>{e.who}</b>{" "}{e.verb}</li>{/each}
      </ul>
    {/if}
    {#if !fold}{@render actions()}{/if}
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
    <form class="reply" onclick={e => e.stopPropagation()} onsubmit={e => e.preventDefault()}>
      <input aria-label="Reply" placeholder="Reply, or @name someone" bind:value={reply} onkeydown={replyKey} />
      <button type="button" onclick={e => { hint = guardedAction(e, "reply", send); }}>Reply</button>
    </form>
    {#if fold}{@render actions()}{/if}
    <!-- Said when a key asked for an action on a trail the page may have steered (`keyboardTrail`). -->
    <p class="act-hint" role="status">{hint ?? ""}</p>
  {/if}
  {#if !open && tools}
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <div class="actions" onclick={e => e.stopPropagation()}>{@render tools(t)}</div>
  {/if}
</article>

{#snippet actions()}
  <!-- Only stops a click on these controls from also selecting the card. -->
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="actions" onclick={e => e.stopPropagation()}>
    {#if t.status === "open"}
      <button onclick={e => { hint = guardedAction(e, "resolve", () => onResolve(t)); }}>Resolve</button>
      {#if !t.sent_to_agent}{#if sendButton}{@render sendButton(guardSend)}{:else}<button class="primary" onclick={e => { guardSend(e, () => onSend(t)); }}>Send to {agent}</button>{/if}{/if}
    {:else}
      <button onclick={e => { hint = guardedAction(e, "reopen", () => onResolve(t)); }}>Reopen</button>
    {/if}
    {@render tools?.(t)}
  </div>
{/snippet}

<!-- A card's styles (spec §8, "Thread sidebar") travel with it (injected on
     mount): the shell's lazy sidebar chunk and the side panel's lists. -->
<style>
  :global {
    .thread-card { background: var(--card); border: 1px solid var(--border); border-radius: var(--radius); padding: 11px 12px 10px; margin-bottom: 10px; cursor: pointer; transition: border-color var(--t), box-shadow var(--t); }
    .thread-card:hover { border-color: var(--border-hover); }
    .thread-card.selected { border-color: var(--border-strong); box-shadow: var(--elev); }
    .thread-card.far:not(.folded) { cursor: auto; }
    .thread-card header { display: flex; align-items: center; gap: 8px; margin-bottom: 8px; min-width: 0; }
    .thread-card .card-head { display: flex; gap: 8px; align-items: center; width: 100%; min-width: 0; min-height: 0; background: none; border: 0; padding: 0; color: var(--muted); text-align: left; font: 400 12.5px var(--font); }
    .thread-card .card-head:not(:disabled):hover { background-color: transparent; }
    .thread-card .anchor-label { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--muted); }
    .thread-card.far .anchor-label { color: var(--fg); font-weight: 500; }
    .fold-caret::before { content: "▸"; color: var(--muted); font-size: 11px; }
    .thread-card:not(.folded) .fold-caret::before { content: "▾"; }
    .fold-body { margin: 0; font-size: 13.5px; line-height: 1.5; display: -webkit-box; -webkit-line-clamp: 3; line-clamp: 3; -webkit-box-orient: vertical; overflow: hidden; overflow-wrap: anywhere; white-space: pre-wrap; }
    .fold-meta { display: flex; flex-wrap: wrap; gap: 2px 10px; margin: 4px 0 0; font-size: 12px; color: var(--muted); min-width: 0; }
    .fold-meta > span { min-width: 0; max-width: 100%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
    .vt { font: 500 11px/16px var(--font); padding: 0 5px; border-radius: var(--radius-xs); color: var(--fg); background: var(--hover); white-space: nowrap; }
    .vt.out { color: var(--muted); }
    .msg { display: grid; gap: 3px; }
    .msg + .msg { margin-top: 10px; }
    .msg .author { font: 600 13px/20px var(--font); }
    .msg .body { margin: 0; font-size: 13.5px; line-height: 1.55; white-space: pre-wrap; overflow-wrap: anywhere; }
    .msg.you { border-left: 2px solid var(--you); padding: 1px 0 1px 10px; }
    .msg.agent { border-right: 2px solid var(--agent); padding: 1px 10px 1px 0; margin-left: 26px; text-align: right; }
    .msg.agent .author { color: var(--agent-ink); }
    .msg.agent .body { text-align: left; }
    .thread-card .st { display: flex; align-items: center; gap: 8px; margin: 10px 0 0; padding-top: 8px; border-top: 1px solid var(--border); font-size: 12px; color: var(--muted); }
    .thread-card .st.ag { color: var(--agent-ink); font: 600 13px/1.25 var(--font); } .thread-card .st small { font: 400 11.5px var(--mono); font-variant-numeric: tabular-nums; color: var(--muted); margin-left: auto; }
    .hist { display: flex; flex-wrap: wrap; gap: 4px 6px; margin: 9px 0 0; padding: 8px 0 0; list-style: none; border-top: 1px solid var(--border); font-size: 12px; color: var(--muted); line-height: 1.6; }
    .hist .sep { margin-right: 2px; }
    .hist .ev { white-space: nowrap; }
    /* Each version's events start a line; the "·" stays in the text, read and copied. */
    .hist .br { flex-basis: 100%; height: 0; }
    .hist .nl .sep { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); }
    .hist .ev b { font-weight: 600; color: var(--fg); }
    .hist .ev.agent .vt { background: var(--accent-tint); color: var(--agent-ink); }
    .thread-card .actions { display: flex; flex-wrap: wrap; gap: 6px; justify-content: flex-end; margin-top: 8px; }
    .thread-card .actions button.primary { background: var(--accent); color: var(--on-accent); border-color: var(--accent); }
    .thread-card .actions button.primary:not(:disabled):hover { background: var(--accent-hover); border-color: var(--accent-hover); }
    .reply { display: flex; gap: 6px; margin-top: 8px; }
    .reply input { flex: 1; min-width: 0; }
    .thread-check { width: 18px; height: 18px; margin: 0; accent-color: var(--accent); flex: none; cursor: pointer; }
    .thread-card:has(.thread-check:checked) { border-color: var(--accent); box-shadow: inset 0 0 0 1px var(--accent); }
  }
</style>
