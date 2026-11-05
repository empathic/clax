<svelte:options css="injected" />

<script lang="ts">
  // A model field (spec 2026-10-21-claxbot D15): a selector of the models
  // the provider lists for the effective key and URL, with ↻ to list them
  // again and "Custom model ID…" to type one it does not list. A value the
  // list lacks shows as its own option. Until the list loads (no key, a bad
  // URL) the selector is disabled and says why (`why`); `readOnly` (a value
  // from the file or the environment) disables it too. Each listed model
  // shows its price (input / output credits per million tokens and where
  // they come from); a note under the selected one says when no admin set
  // its price (tokenulator.dev's or the default rates: an estimate) or it
  // has none.
  import type { ProviderModels } from "./api";
  import { priceSummary } from "./model";

  let { id, label, hideLabel = false, value, models, why, readOnly = false, onRefresh, onChange }: {
    id: string; label: string; hideLabel?: boolean; value: string; models: ProviderModels["models"] | null; why?: string | null; readOnly?: boolean;
    onRefresh(): void; onChange(v: string): void;
  } = $props();
  const CUSTOM = "\u0000custom";
  let custom = $state(false);
  let typed = $state("");
  let input: HTMLInputElement | undefined = $state();
  // The select shows `value` (set here: the listed options come after it).
  let sel: HTMLSelectElement | undefined = $state();
  $effect(() => { void listed; if (sel) sel.value = value; });
  $effect(() => { input?.focus(); });
  const listed = $derived(models ?? []);
  const off = $derived(readOnly || models === null);
  const priced = $derived(listed.find(m => m.id === value)?.pricing);
  const note = $derived(!priced ? null
    : !priced.source ? "No price set: runs on it are refused until it is priced under Pricing."
    : !priced.estimated ? null
    : `Estimated: ${priced.source === "mixed" ? "only part of its price is set. The rest is" : "no price set. Runs are"} charged at ${priced.source === "default" ? "the default rates" : priced.source === "catalog" ? "tokenulator.dev's price" : "tokenulator.dev's price or the default rates"} until it is priced under Pricing.`);
  const name = (m: ProviderModels["models"][number]) => {
    const n = m.display_name && m.display_name !== m.id ? `${m.display_name} (${m.id})` : m.id;
    return m.pricing ? `${n} — ${priceSummary(m.pricing)}` : n;
  };
  const described = $derived([why ? `${id}-why` : "", note ? `${id}-price` : ""].filter(Boolean).join(" ") || undefined);
  const pick = (v: string) => { if (v === CUSTOM) { custom = true; typed = value; } else onChange(v); };
  const done = (e: Event) => { e.preventDefault(); const t = typed.trim(); if (t) onChange(t); custom = false; };
</script>

<div class="bmodel">
  <label for={id} class:bot-sr={hideLabel}>{label}</label>
  {#if custom}
    <form class="bmodel-row" onsubmit={done}>
      <input {id} type="text" spellcheck="false" autocomplete="off" placeholder="claude-…" bind:value={typed} bind:this={input} />
      <button type="submit">Use</button>
      <button type="button" class="ghost" onclick={() => { custom = false; }}>Cancel</button>
    </form>
  {:else}
    <div class="bmodel-row">
      <select {id} bind:this={sel} disabled={off} aria-describedby={described} onchange={e => pick(e.currentTarget.value)}>
        {#if !listed.some(m => m.id === value)}<option value={value} selected>{value || "None"}</option>{/if}
        {#each listed as m (m.id)}<option value={m.id} selected={m.id === value}>{name(m)}</option>{/each}
        <option disabled>──────────</option>
        <option value={CUSTOM}>Custom model ID…</option>
      </select>
      <button type="button" class="icon" aria-label={`List the models again for ${label}`} title="List the models again" disabled={readOnly} onclick={onRefresh}>↻</button>
    </div>
  {/if}
  {#if why}<p class="bmodel-why" id={`${id}-why`}>{why}</p>{/if}
  {#if note && !custom}<p class="bmodel-why" id={`${id}-price`} role="status">{note}</p>{/if}
</div>

<style>
  :global {
    .bmodel { display: grid; gap: 4px; min-width: 0; }
    .bmodel label { font-size: 12.5px; font-weight: 600; color: var(--muted); }
    .bmodel-row { display: flex; gap: 6px; min-width: 0; }
    .bmodel-row select, .bmodel-row input { flex: 1 1 auto; min-width: 0; }
    .bmodel-why { margin: 0; font-size: 12.5px; color: var(--muted); }
  }
</style>
