<script lang="ts">
  // The one modal the shell shows for a page: a capability's consent or a
  // download's confirmation. It opens with focus on the refusing button, and
  // "Allow" stays disabled for ALLOW_DELAY_MS, so a keystroke meant for the
  // page cannot grant consent. Escape dismisses it (neither allow nor deny).
  import { ALLOW_DELAY_MS, type Ask } from "../view/prompt-queue";

  let { ask }: { ask: Ask } = $props();
  let deny = $state<HTMLButtonElement>();
  let armed = $state(false);
  // Each ask (the next one in the queue reuses the dialog) refocuses the
  // refusing button and disarms "Allow" for the delay again.
  $effect(() => {
    const a = ask;
    armed = false;
    deny?.focus();
    const timer = setTimeout(() => { armed = true; }, ALLOW_DELAY_MS);
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") a.answer("dismiss"); };
    addEventListener("keydown", onKey);
    return () => { clearTimeout(timer); removeEventListener("keydown", onKey); };
  });
</script>

<div class="prompt-backdrop">
  <div class="prompt" role="dialog" aria-modal="true" aria-labelledby="prompt-title" aria-describedby="prompt-body">
    <h2 id="prompt-title">{ask.prompt.title}</h2>
    <p id="prompt-body">{ask.prompt.body}</p>
    <div class="actions">
      <button type="button" bind:this={deny} onclick={() => ask.answer("deny")}>{ask.prompt.deny}</button>
      <button type="button" class="primary" disabled={!armed} onclick={() => { if (armed) ask.answer("allow"); }}>{ask.prompt.allow}</button>
    </div>
  </div>
</div>
