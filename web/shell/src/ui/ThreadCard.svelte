<script lang="ts">
  import type { Snippet } from "svelte";
  import { isSubmitKey } from "../view/composer-model";
  import { type Thread, type Viewer, anchorLabel } from "../threads";
  import type { Version } from "../api";
  import { type HistoryEvent, addressedNote } from "../view/history-model";
  import { authorLabel } from "../view/sidebar-model";
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
    onSelect(t: Thread): void; onSend(t: Thread): void; onResolve(t: Thread): void; onReply(t: Thread, body: string): void; onHover?(t: Thread | null): void;
  };
  let { t, n, now, selected, file, history, outdated, agent, when, marker = null, versions = [], onSeen, checked = false, onToggle, send: sendButton, onSelect, onSend, onResolve, onReply, onHover }: Props = $props();
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
    let timer: ReturnType<typeof setTimeout> | undefined;
    const io = new IntersectionObserver(([e]) => {
      clearTimeout(timer);
      if (e.intersectionRatio >= 0.5) timer = setTimeout(() => onSeen(t), 1000);
    }, { threshold: [0, 0.5] });
    io.observe(el);
    return () => { clearTimeout(timer); io.disconnect(); };
  };
  // A sent thread has no Send: focus on the card moves to its head first, not to <body>.
  const guardSend = (e: Event, act: () => void) => {
    hint = guardedAction(e, "send", () => { if (card?.contains(document.activeElement)) head?.focus(); act(); });
  };
  const label = $derived(t.status === "open" && t.sent_to_agent ? waitingLabel(t.feedback_state, now) : null);
</script>

<!-- The card's header button is its keyboard path; a click anywhere else on the card is a pointer shortcut to the same action. -->
<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
<article class={`thread-card${selected === t.id ? " selected" : ""}`} data-thread={t.id} bind:this={card} onclick={() => onSelect(t)} {@attach seen}
  onmouseenter={() => onHover?.(t)} onmouseleave={() => onHover?.(null)}>
  <header>
    {#if onToggle}<input type="checkbox" class="thread-check" {checked} aria-label={`Select thread ${n ?? ""} ${anchorLabel(t.anchor)}`.replace("  ", " ")}
      onclick={e => { e.stopPropagation(); onToggle(t, e.shiftKey); }} />{/if}
    <button type="button" class="card-head" bind:this={head} aria-pressed={selected === t.id} onclick={e => { e.stopPropagation(); onSelect(t); }}
      >{#if n !== undefined}<span class="thread-num">{n}</span>{/if}<span class="anchor-label">{anchorLabel(t.anchor)}</span
      >{#if outdated}<span class="vt out">outdated</span>{/if}{#if t.anchor.file !== file}<span class="file-label muted small">on {t.anchor.file}</span>{/if}<span class="muted small">{when}</span
    ></button>
  </header>
  {#if t.clip_url}<img class="thumb" src={t.clip_url} alt="Screenshot of the commented region" loading="lazy" />{/if}
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
  {#if t.status === "open"}
    <!-- Only stops a click on these controls from also selecting the card. -->
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <div class="actions" onclick={e => e.stopPropagation()}>
      <button onclick={e => { hint = guardedAction(e, "resolve", () => onResolve(t)); }}>Resolve</button>
      {#if !t.sent_to_agent}{#if sendButton}{@render sendButton(guardSend)}{:else}<button class="primary" onclick={e => { guardSend(e, () => onSend(t)); }}>Send to {agent}</button>{/if}{/if}
    </div>
  {/if}
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
  <form class="reply" onclick={e => e.stopPropagation()} onsubmit={e => e.preventDefault()}>
    <input aria-label="Reply" placeholder="Reply, or @name someone" bind:value={reply} onkeydown={replyKey} />
    <button type="button" onclick={e => { hint = guardedAction(e, "reply", send); }}>Reply</button>
  </form>
  <!-- Said when a key asked for an action on a trail the page may have steered (`keyboardTrail`). -->
  <p class="act-hint" role="status">{hint ?? ""}</p>
</article>
