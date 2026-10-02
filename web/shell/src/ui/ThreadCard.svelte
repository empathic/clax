<script lang="ts">
  import { isSubmitKey } from "../view/composer-model";
  import { type Thread, type Viewer, anchorLabel } from "../threads";
  import type { HistoryEvent } from "../view/history-model";
  import { authorLabel } from "../view/sidebar-model";
  import { waitingLabel } from "../waiting";

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
    onSelect(t: Thread): void; onSend(t: Thread): void; onResolve(t: Thread): void; onReply(t: Thread, body: string): void; onHover?(t: Thread | null): void;
  };
  let { t, n, now, selected, file, history, outdated, agent, when, onSelect, onSend, onResolve, onReply, onHover }: Props = $props();
  let reply = $state("");
  const send = () => { if (reply.trim()) { onReply(t, reply); reply = ""; } };
  const label = $derived(t.status === "open" && t.sent_to_agent ? waitingLabel(t.feedback_state, now) : null);
</script>

<!-- The card's header button is its keyboard path; a click anywhere else on the card is a pointer shortcut to the same action. -->
<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
<article class={`thread-card${selected === t.id ? " selected" : ""}`} data-thread={t.id} onclick={() => onSelect(t)}
  onmouseenter={() => onHover?.(t)} onmouseleave={() => onHover?.(null)}>
  <header>
    <button type="button" class="card-head" aria-pressed={selected === t.id} onclick={e => { e.stopPropagation(); onSelect(t); }}
      >{#if n !== undefined}<span class="thread-num">{n}</span>{/if}<span class="anchor-label">{anchorLabel(t.anchor)}</span
      >{#if outdated}<span class="vt out">outdated</span>{/if}{#if t.anchor.file !== file}<span class="file-label muted small">on {t.anchor.file}</span>{/if}<span class="muted small">{when}</span
    ></button>
  </header>
  {#if t.clip_url}<img class="thumb" src={t.clip_url} alt="Screenshot of the commented region" loading="lazy" />{/if}
  {#each t.comments as c (c.id)}
    <div class={["msg", c.author_kind === "agent" ? "agent" : "you"]}>
      <b class="author">{authorLabel(c)}{#if c.via_page}<span class="via-page muted small">{" · via the page"}</span>{/if}</b>
      <p class="body">{c.body}</p>
    </div>
  {/each}
  {#if label}<p class="st waiting">{label}</p>{/if}
  {#if history.length}
    <p class="hist" aria-label="History">
      {#each history as e, i (i)}<span class={["ev", e.agent && "agent"]}>{#if e.v !== null}<span class="vt">v{e.v}</span>{/if}<b>{e.who}</b> {e.verb}</span>{/each}
    </p>
  {/if}
  {#if t.status === "open"}
    <!-- Only stops a click on these controls from also selecting the card. -->
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <div class="actions" onclick={e => e.stopPropagation()}>
      <button onclick={() => onResolve(t)}>Resolve</button>
      {#if !t.sent_to_agent}<button class="primary" onclick={() => onSend(t)}>Send to {agent}</button>{/if}
    </div>
  {/if}
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
  <form class="reply" onclick={e => e.stopPropagation()} onsubmit={e => { e.preventDefault(); send(); }}>
    <input aria-label="Reply" placeholder="Reply, or @name someone" bind:value={reply}
      onkeydown={e => { if (isSubmitKey(e)) { e.preventDefault(); send(); } }} />
    <button type="submit">Reply</button>
  </form>
</article>
