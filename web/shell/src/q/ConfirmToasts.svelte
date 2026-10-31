<svelte:options css="injected" />

<script lang="ts">
  // The toasts of the decisions waiting for the owner's click (docs/
  // contract.md, "Browser confirmations"), on every page of the owner's:
  // one card per open confirmation (`ConfirmationCard`), newest at the
  // bottom of a stack in the corner, so a call the CLI or an agent waits
  // on is seen wherever the owner is. A card answered here stays a
  // moment, saying how, then leaves; the inbox shows them too. At phone
  // width the stack spans the screen, inside its 16px gutter.
  import { after } from "../clock";
  import ConfirmationCard from "./ConfirmationCard.svelte";
  import { type ConfirmationFeed, type ConfirmationView, confirmations } from "./confirm.svelte";

  let { feed }: { feed?: ConfirmationFeed } = $props();
  const f = $derived(feed ?? confirmations());
  /** How long an answered card stays, saying how. */
  const LEAVE_MS = 2500;
  let answered = $state<ConfirmationView[]>([]);
  const shown = $derived([...f.open.filter(c => !answered.some(a => a.id === c.id)), ...answered]);
  const done = (c: ConfirmationView) => {
    answered = [...answered, c];
    after(LEAVE_MS, () => { answered = answered.filter(a => a.id !== c.id); f.take(c); });
  };
</script>

{#if shown.length}
  <section class="confirm-toasts" aria-label="Waiting for your confirmation">
    {#each shown as c (c.id)}<ConfirmationCard {c} onanswered={done} />{/each}
  </section>
{/if}

<style>
  :global {
    .confirm-toasts { position: fixed; right: 16px; bottom: 16px; z-index: 60; display: grid; gap: 8px; width: min(380px, calc(100vw - 32px)); max-height: calc(100vh - 32px); overflow-y: auto; }
    .confirm-toasts .cfcard { box-shadow: var(--shadow, 0 6px 24px rgb(0 0 0 / 0.18)); }
  }
</style>
