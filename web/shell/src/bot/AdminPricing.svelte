<svelte:options css="injected" />

<script lang="ts">
  // Admin › Pricing (spec 2026-10-21-claxbot §7.2, §7.4): what one credit
  // sells for, the free credits everyone gets each week, and every cost
  // line, with what it costs Clax, the margin on it (editable here), and the
  // credits one unit then spends (cost × (1 + margin) ÷ credit price, rounded
  // up; the harness fee is fixed). A change applies to new usage only.
  import { onMount } from "svelte";
  import { refusal } from "../people/refusal";
  import { type CostLine, type Pricing, pricing, setPricing } from "./api";
  import { UNITS, credits, money, perUnitMilli } from "./model";

  let p = $state<Pricing | null>(null);
  let err = $state<string | null>(null);
  let saved = $state("");
  let busy = $state(false);
  // The form: the credit price in dollars, the free tier in credits, each line's margin in percent.
  let price = $state("");
  let free = $state("");
  let margin = $state<Record<string, string>>({});
  function take(x: Pricing) {
    p = x;
    price = String(x.credit_price_micro_usd / 1e6);
    free = String(x.free_weekly_milli / 1000);
    margin = Object.fromEntries(x.lines.map(l => [l.id, String(l.margin_bp / 100)]));
  }
  const priceMicro = $derived(Math.round(Number(price) * 1e6));
  const marginBp = (l: CostLine) => { const n = Number(margin[l.id]); return Number.isFinite(n) ? Math.round(n * 100) : l.margin_bp; };
  const perUnit = (l: CostLine) => perUnitMilli({ ...l, margin_bp: marginBp(l) }, priceMicro);
  async function save() {
    if (!p || busy) return;
    if (!(priceMicro > 0)) { err = "A credit's price is more than $0."; return; }
    const f = Number(free);
    if (!Number.isFinite(f) || f < 0) { err = "The free tier is a number of credits, 0 or more."; return; }
    busy = true; err = null; saved = "";
    try {
      take(await setPricing({ credit_price_micro_usd: priceMicro, free_weekly_milli: Math.round(f * 1000), lines: p.lines.map(l => ({ ...l, margin_bp: marginBp(l) })) }));
      saved = "Saved. New usage is charged at these rates.";
    } catch (e) { err = refusal(e); } finally { busy = false; }
  }
  onMount(() => { void pricing().then(take, e => { err = refusal(e); }); });
</script>

{#if err}<p class="err" role="alert">{err}</p>{/if}
{#if p}
  <section class="bcard" aria-labelledby="pr-c">
    <h3 id="pr-c">Credits</h3>
    <p>One credit is the unit of spend, sold at a fixed price. Free credits are charged against the same cost lines, so the free tier's cost shows in Usage.</p>
    <div class="fields">
      <div class="field"><label for="pr-price">Credit price (USD)</label><input id="pr-price" type="number" min="0" step="0.0001" inputmode="decimal" bind:value={price} /></div>
      <div class="field"><label for="pr-free">Free credits each week</label><input id="pr-free" type="number" min="0" step="1" inputmode="numeric" bind:value={free} /></div>
    </div>
  </section>
  <section class="bcard" aria-labelledby="pr-l">
    <h3 id="pr-l">Cost lines</h3>
    <div class="scroll">
      <table class="pr">
        <thead><tr><th>Cost line</th><th>Unit</th><th class="num">Our cost</th><th class="num">Margin</th><th class="num">Credits per unit</th><th>Applies</th></tr></thead>
        <tbody>
          {#each p.lines as l (l.id)}
            <tr>
              <td><b>{l.label}</b>{#if l.source}<small class="src">{l.source}</small>{/if}</td>
              <td>{UNITS[l.unit] ?? l.unit}</td>
              <td class="num">{l.fixed_credits_milli != null ? "—" : money(l.cost_micro_usd)}</td>
              <td class="num">{#if l.fixed_credits_milli != null}—{:else}<label class="mg"><input type="number" step="1" min="0" inputmode="decimal" aria-label={`Margin on ${l.label}, percent`} bind:value={margin[l.id]} /><span aria-hidden="true">%</span></label>{/if}</td>
              <td class="num">{credits(perUnit(l))}</td>
              <td>{l.applies === "always" ? "every run" : "instance key"}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
    <p class="note">Credits per unit = our cost × (1 + margin) ÷ credit price, rounded up. Each line names where its cost comes from; a change applies to new usage only.</p>
  </section>
  <div class="acts">
    <button type="button" class="primary" aria-disabled={busy} onclick={save}>{busy ? "Saving…" : "Save"}</button>
    <span role="status">{#if saved}<span class="ok">{saved}</span>{/if}</span>
  </div>
{:else if !err}
  <p class="note" role="status">Loading the prices…</p>
{/if}

<style>
  :global {
    .pr { min-width: 680px; }
    .pr .src { display: block; font-size: 11.5px; color: var(--muted); }
    .pr .mg { display: inline-flex; align-items: center; gap: 4px; }
    .pr .mg input { width: 72px; text-align: end; }
  }
</style>
