<script lang="ts">
  import { isSubmitKey } from "../view/composer-model";
  import { type Thread, type Viewer, anchorLabel } from "../threads";
  import type { Version } from "../api";
  import { type HistoryEvent, addressedNote } from "../view/history-model";
  import { authorLabel } from "../view/sidebar-model";
  import { guardedAction } from "../view/trail";
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
    onSelect(t: Thread): void; onSend(t: Thread): void; onResolve(t: Thread): void; onReply(t: Thread, body: string): void; onHover?(t: Thread | null): void;
  };
  let { t, n, now, selected, file, history, outdated, agent, when, marker = null, versions = [], onSeen, onSelect, onSend, onResolve, onReply, onHover }: Props = $props();
  let reply = $state("");
  let hint: string | null = $state(null);
  const send = () => { if (reply.trim()) { onReply(t, reply); reply = ""; } };
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
  const label = $derived(t.status === "open" && t.sent_to_agent ? waitingLabel(t.feedback_state, now) : null);
</script>

<!-- The card's header button is its keyboard path; a click anywhere else on the card is a pointer shortcut to the same action. -->
<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
<article class={`thread-card${selected === t.id ? " selected" : ""}`} data-thread={t.id} onclick={() => onSelect(t)} {@attach seen}
  onmouseenter={() => onHover?.(t)} onmouseleave={() => onHover?.(null)}>
  <header>
    <button type="button" class="card-head" aria-pressed={selected === t.id} onclick={e => { e.stopPropagation(); onSelect(t); }}
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
      {#each history as e, i (i)}<li class={["ev", e.agent && "agent"]}>{#if i}{" "}<span class="sep">·</span>{" "}{/if}{#if e.v !== null}<span class="vt">v{e.v}</span>{" "}{/if}<b>{e.who}</b>{" "}{e.verb}</li>{/each}
    </ul>
  {/if}
  {#if t.status === "open"}
    <!-- Only stops a click on these controls from also selecting the card. -->
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <div class="actions" onclick={e => e.stopPropagation()}>
      <button onclick={e => { hint = guardedAction(e, "resolve", () => onResolve(t)); }}>Resolve</button>
      {#if !t.sent_to_agent}<button class="primary" onclick={e => { hint = guardedAction(e, "send", () => onSend(t)); }}>Send to {agent}</button>{/if}
    </div>
    <!-- Said when the keyboard asked for an action on a trail the page may have steered (`keyboardTrail`). -->
    <p class="act-hint" role="status">{hint ?? ""}</p>
  {/if}
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
  <form class="reply" onclick={e => e.stopPropagation()} onsubmit={e => { e.preventDefault(); send(); }}>
    <input aria-label="Reply" placeholder="Reply, or @name someone" bind:value={reply}
      onkeydown={e => { if (isSubmitKey(e)) { e.preventDefault(); send(); } }} />
    <button type="submit">Reply</button>
  </form>
</article>
