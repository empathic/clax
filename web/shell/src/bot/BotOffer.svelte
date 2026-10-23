<svelte:options css="injected" />

<script lang="ts">
  // ClaxBot's offer on a thread sent to the agent that no agent is running
  // to take (`agent_ended`), shown to a viewer ClaxBot is available to: it
  // asks before spending their credits. "Send to ClaxBot" sends this thread;
  // "Always here" also chooses ClaxBot for the artifact's Sends, so this
  // viewer's later `@agent` threads that no agent takes go to ClaxBot
  // without asking (ShellSidebar).
  import { credits } from "./model";
  import { botView } from "./send.svelte";

  let { onSend, onAlways }: { onSend(): void; onAlways(): void } = $props();
  const cost = $derived(botView.me?.estimate_milli ? `, usually ${credits(botView.me.estimate_milli.low)}–${credits(botView.me.estimate_milli.high)} credits` : "");
</script>

<div class="bot-offer" role="group" aria-label="Send to ClaxBot">
  <p>No agent is running to take this. ClaxBot can work on it here, using your credits{cost}.</p>
  <div class="acts">
    <button type="button" class="primary" onclick={e => { e.stopPropagation(); onSend(); }}>Send to ClaxBot</button>
    <button type="button" class="ghost" onclick={e => { e.stopPropagation(); onAlways(); }}>Always here</button>
  </div>
</div>

<style>
  .bot-offer { display: grid; gap: 6px; margin: 0 0 8px; padding: 8px 10px; border: 1px solid var(--border); border-radius: var(--radius-sm); background: var(--accent-tint); }
  .bot-offer p { margin: 0; font-size: 12.5px; line-height: 1.5; }
  .acts { display: flex; flex-wrap: wrap; gap: 6px; }
  @media (pointer: coarse) { .acts button { min-height: 44px; } }
</style>
