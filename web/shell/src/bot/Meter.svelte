<svelte:options css="injected" />

<script lang="ts">
  // A week's credits as a bar (`role="meter"`): spent of the allowance,
  // amber from 80 %, red once used up. Its name is `label`, its value text
  // "310 of 2,000 credits".
  import { credits, type Meter } from "./model";

  let { m, label, small = false }: { m: Meter; label: string; small?: boolean } = $props();
  const text = $derived(`${credits(m.spent)} of ${credits(m.total)} credits`);
</script>

<div class="bmeter" class:small class:warn={m.warn} class:out={m.exhausted} role="meter" aria-label={label} aria-valuemin={0} aria-valuemax={m.total / 1000} aria-valuenow={Math.min(m.spent, m.total) / 1000} aria-valuetext={text}>
  <span style:width={`${Math.round(m.ratio * 100)}%`}></span>
</div>

<style>
  :global {
    .bmeter { height: 8px; border-radius: 999px; background: var(--hover); border: 1px solid var(--border-strong); overflow: hidden; min-width: 60px; }
    .bmeter.small { height: 6px; }
    .bmeter > span { display: block; height: 100%; background: var(--accent); border-radius: inherit; }
    .bmeter.warn > span { background: var(--tag-yellow-fg); }
    .bmeter.out > span { background: var(--danger); }
  }
</style>
