<svelte:options css="injected" />

<script lang="ts">
  // An in-terminal question (spec 2026-10-16-ask-where-attention-is-design
  // §3): an agent is asking the person in the terminal, or in chat. It says
  // who and when, and shows the questions' headers only; it takes no
  // answer. **Ask me here** sends the agent word to ask it in Clax instead
  // (following the keyboard trail rule, `guardedAction`); for Claude Code's
  // terminal dialog it says the dialog stays until Esc. Closed, it says what
  // closed it, for the list's usual moment. Its styles travel with the
  // question module's chunk and carry the card's (`.qcard`) frame
  // themselves, as QuestionCard's styles inject only once a QuestionCard
  // mounts.
  import type { QuestionView } from "../api";
  import { relativeTime } from "../format";
  import { closedLabel, cutHeader } from "./model";
  import { TERMINAL_ESC, terminalLine } from "./terminal";
  import { trail } from "./trail";

  const p: {
    q: QuestionView;
    /** The questions listed beside it: an agent is named by harness alone unless another of its harness is among them. */
    others?: QuestionView[];
    now?: Date;
    onPull(): Promise<void>;
  } = $props();
  let busy = $state(false);
  let hint = $state<string | null>(null);
  let failed = $state(false);
  $effect(() => trail().keyboardTrail.onClear(() => { hint = null; }));
  const open = $derived(p.q.status === "open");
  const who = $derived(terminalLine(p.q, (p.others ?? [p.q]).map(o => o.agent)));
  const pull = (e: Event) => {
    if (busy) return;
    hint = trail().guardedAction(e, "ask here", () => {
      busy = true;
      failed = false;
      p.onPull().then(() => { busy = false; }, () => { busy = false; failed = true; });
    });
  };
</script>

<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<article class="qcard in-terminal" data-question={p.q.id} data-closed={open ? undefined : ""} aria-label={who} tabindex="-1">
  <header class="qhead">
    <strong class="asker">{who}</strong>
    {#if p.q.agent?.project}<span class="proj">in {p.q.agent.project}</span>{/if}
    <time datetime={p.q.created_at}>{relativeTime(p.q.created_at, p.now ?? new Date())}</time>
  </header>
  <ul class="hdrs" aria-label="Questions">{#each p.q.questions as s (s.question)}<li title={s.header}>{cutHeader(s.header)}</li>{/each}</ul>
  {#if open}
    {#if p.q.source === "hook"}<p class="esc small muted">{TERMINAL_ESC}</p>{/if}
    <div class="acts">
      <button type="button" class="primary" disabled={busy} onclick={pull}>Ask me here</button>
      {#if failed}<span class="err small">Not sent: try again</span>{/if}
    </div>
    <p class="act-hint" role="status">{hint ?? ""}</p>
  {:else}
    <p class="closed" role="status">{closedLabel(p.q)}</p>
  {/if}
</article>

<style>
  :global {
    .qcard.in-terminal { position: relative; background: var(--card); border: 1px solid var(--border); border-left: 3px solid var(--agent-ink); border-radius: var(--radius); padding: 11px 12px 10px 13px; min-width: 0; }
    .qcard.in-terminal[data-closed] { border-left-color: var(--border-strong); }
    .qcard.in-terminal:focus { outline: none; }
    .qcard.in-terminal:focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; }
    .qcard.in-terminal .qhead { display: flex; flex-wrap: wrap; align-items: baseline; gap: 2px 8px; font-size: 12.5px; color: var(--muted); margin-bottom: 8px; min-width: 0; }
    .qcard.in-terminal .asker { font: 600 13px/20px var(--font); color: var(--agent-ink); overflow-wrap: anywhere; }
    .qcard.in-terminal time { margin-left: auto; white-space: nowrap; }
    .qcard.in-terminal .hdrs { display: flex; flex-wrap: wrap; gap: 6px; margin: 0 0 8px; padding: 0; list-style: none; }
    .qcard.in-terminal .hdrs li { padding: 2px 8px; border: 1px solid var(--border-strong); border-radius: var(--radius-sm); font: 500 12px/16px var(--font); color: var(--muted); white-space: nowrap; }
    .qcard.in-terminal .esc { margin: 0 0 8px; }
    .qcard.in-terminal .acts { display: flex; flex-wrap: wrap; align-items: center; justify-content: flex-end; gap: 8px; margin-top: 10px; }
    .qcard.in-terminal .err { color: var(--danger); }
    .qcard.in-terminal .closed { margin: 0; font: 600 13px/20px var(--font); color: var(--fg); }
  }
</style>
