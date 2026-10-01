<script lang="ts">
  import { isSubmitKey } from "../comments";
  import { type Thread, type Viewer, anchorLabel, resolvedByLabel } from "../threads";
  import { authorLabel } from "../view/sidebar-model";
  import { waitingLabel } from "../waiting";

  type Props = {
    t: Thread; n?: number; now: Date; me?: Viewer | null; selected: string | null; file: string | null;
    onSelect(t: Thread): void; onSend(t: Thread): void; onResolve(t: Thread): void; onReply(t: Thread, body: string): void; onHover?(t: Thread | null): void;
  };
  let { t, n, now, me, selected, file, onSelect, onSend, onResolve, onReply, onHover }: Props = $props();
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
      >{#if t.anchor.file !== file}<span class="file-label muted small">on {t.anchor.file}</span>{/if}<span class="muted small">v{t.version_n}</span
    ></button>
  </header>
  {#if t.clip_url}<img class="thumb" src={t.clip_url} alt="Screenshot of the commented region" loading="lazy" />{/if}
  {#each t.comments as c (c.id)}
    <div class={`comment ${c.author_kind === "agent" ? "agent" : "from-viewer"}`}>
      <div class="author">{authorLabel(c)}{#if c.via_page}<span class="via-page muted small">{" · via the page"}</span>{/if}</div>
      <div class="body">{c.body}</div>
    </div>
  {/each}
  {#if label}<p class="waiting">{label}</p>{/if}
  {#if t.status === "resolved" && t.resolved_by}<p class="resolved-by muted small">Resolved by {resolvedByLabel(t.resolved_by, me)}</p>{/if}
  {#if t.status === "open"}
    <!-- Only stops a click on these controls from also selecting the card. -->
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <div class="actions" onclick={e => e.stopPropagation()}>
      {#if !t.sent_to_agent}<button class="primary" onclick={() => onSend(t)}>Send to agent</button>{/if}
      <button onclick={() => onResolve(t)}>Resolve</button>
    </div>
  {/if}
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
  <form class="reply" onclick={e => e.stopPropagation()} onsubmit={e => { e.preventDefault(); send(); }}>
    <input aria-label="Reply" placeholder="Reply…" bind:value={reply}
      onkeydown={e => { if (isSubmitKey(e)) { e.preventDefault(); send(); } }} />
    <button type="submit">Reply</button>
  </form>
</article>
