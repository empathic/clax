<svelte:options css="injected" />
<script lang="ts">
  // Agents drafting live (hosted spec §16.4): "<agent> is drafting…" with
  // Show draft for each agent drafting on the artifact; while a draft is
  // shown, whose it is, how many changed elements its page tinted, and Back
  // to the version the viewer was showing. Each agent is named as the
  // working signal names it. Loaded with the drafts' module (view/drafts.ts),
  // off the artifact entry.
  import type { ViewState } from "../view/artifact-controller";
  import { bannerText } from "../view/draft-model";
  import { agentNames } from "../view/working-model";

  let { s, shown, onShow, onBack }: { s: ViewState; shown: number; onShow(agent: string): void; onBack(): void } = $props();
  const text = $derived.by(() => {
    const names = agentNames(s.working, s.agents);
    return bannerText(s.drafts, s.showDraft, s.draftChanged, shown, (h, harness) => names.get(h) ?? harness);
  });
</script>

{#if text}
<div class="banner draft" class:showing={text.kind === "showing"} role="status" aria-live="polite">
  {#if text.kind === "showing"}
    <span class="what"><span class="pulse" aria-hidden="true"></span>Showing <b>{text.who}</b>’s draft over v{text.base} <span class="muted">· {text.detail}</span></span>
    {#each text.others as o (o.agent)}
      <button type="button" class="who" onclick={() => onShow(o.agent)} aria-label={`Show ${o.who}’s draft`}>{o.who}</button>
    {/each}
    <button type="button" class="primary" onclick={onBack}>{text.back}</button>
  {:else if text.kind === "drafting"}
    <span class="what"><span class="pulse" aria-hidden="true"></span><b>{text.who}</b> is drafting…{#if text.note}<span class="muted note">{text.note}</span>{/if}</span>
    <button type="button" class="primary" onclick={() => onShow(text.agent)}>Show draft</button>
  {:else}
    <span class="what"><span class="pulse" aria-hidden="true"></span><b>{text.who}</b> are drafting…</span>
    {#each text.agents as o (o.agent)}
      <button type="button" class="who" onclick={() => onShow(o.agent)} aria-label={`Show ${o.who}’s draft`}>{o.who}</button>
    {/each}
  {/if}
</div>
{/if}

<style>
  /* In the stage's foot (StageIsland, theme.css .stage-foot): a page opens
     on its heading, and a draft page scrolls its first change to the middle. */
  .draft { width: max-content; font-size: 13px; flex-wrap: wrap; row-gap: 6px; background: var(--raised); }
  .draft.showing { border-color: var(--agent); }
  .pulse { display: inline-block; width: 8px; height: 8px; margin-right: 8px; vertical-align: 1px; border-radius: 50%; background: var(--agent); animation: pulse 1.6s ease-in-out infinite; }
  @keyframes pulse { 0%, 100% { opacity: 1; } 50% { opacity: .35; } }
  @media (prefers-reduced-motion: reduce) { .pulse { animation: none; } }
  .what { flex: 1 1 auto; min-width: 0; overflow-wrap: anywhere; color: var(--fg); }
  .what b { color: var(--agent-ink); font-weight: 600; }
  .note::before { content: " · "; }
  .muted { color: var(--muted); }
  .who { flex: none; color: var(--agent-ink); border-color: var(--agent); background: var(--card); }
  button { flex: none; }
</style>
