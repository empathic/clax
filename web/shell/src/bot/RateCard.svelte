<svelte:options css="injected" />

<script lang="ts">
  // A model's rate card editor (docs/contract.md "Pricing"): each token unit
  // charged at its cost plus a margin (US dollars per million tokens, a
  // percentage) or at fixed credits per million tokens, starting from the
  // rate it is charged at now (`from`). A unit that is not cost-based now
  // (new, fixed, or at the default rates) starts its margin at the token
  // margin (`marginBp`), and one at the default rates with no cost. Each
  // unit shows the credits a million tokens then charge at the saved credit
  // price (`priceMicro`). Save hands the four units over as `POST
  // /api/admin/pricing/models` takes them; a fixed unit keeps its cost and
  // margin, for information.
  import { onMount, untrack } from "svelte";
  import type { ModelPricing, TokenUnit, UnitPricing, UnitRate } from "./api";
  import { TOKEN_UNITS, credits, perUnitMilli } from "./model";

  let { id, model, from = null, priceMicro, marginBp, busy = false, saving = false, focus = false, saveLabel = "Save rates", onSave, onCancel }: {
    id: string; model: string; from?: ModelPricing | null; priceMicro: number; marginBp: number;
    /** `busy`: another write is in flight, so Save waits; `saving`: this card's is. */
    busy?: boolean; saving?: boolean; focus?: boolean; saveLabel?: string;
    onSave(units: Record<TokenUnit, UnitRate>): void; onCancel?(): void;
  } = $props();
  type Draft = { fixed: boolean; usd: string; margin: string; credits: string };
  const draft = (u: UnitPricing | undefined): Draft => {
    const costed = !!u?.source && u.fixed_credits_milli === null && u.margin_bp !== null;
    return {
      fixed: u?.fixed_credits_milli != null,
      usd: u?.cost_micro_usd != null && u.source !== "default" ? String(u.cost_micro_usd / 1e6) : "",
      margin: String((costed ? u!.margin_bp! : marginBp) / 100),
      credits: u?.credits_milli_per_mtok != null ? String(u.credits_milli_per_mtok / 1000) : "",
    };
  };
  let d = $state(untrack(() => Object.fromEntries(TOKEN_UNITS.map(u => [u.id, draft(from?.units[u.id])])) as Record<TokenUnit, Draft>));
  let err = $state<string | null>(null);
  let form: HTMLFormElement | undefined = $state();
  onMount(() => { if (focus) form?.querySelector("select")?.focus(); });

  // A number input's binding holds a number, or null when it is empty.
  const num = (s: string | number | null) => (s === null || String(s).trim() === "" ? NaN : Number(s));
  const ok = (n: number) => Number.isFinite(n) && n >= 0;
  /** The unit as the route takes it, or why it cannot be. */
  function rate(x: Draft, label: string): UnitRate | string {
    const [usd, margin, c] = [num(x.usd), num(x.margin), num(x.credits)];
    if (x.fixed) {
      if (!ok(c)) return `${label}: the credits per million tokens are a number, 0 or more.`;
      return { cost_micro_usd: ok(usd) ? Math.round(usd * 1e6) : 0, margin_bp: ok(margin) ? Math.round(margin * 100) : 0, fixed_credits_milli: Math.round(c * 1000) };
    }
    if (!ok(usd)) return `${label}: the cost is US dollars per million tokens, 0 or more.`;
    if (!ok(margin)) return `${label}: the margin is a percentage, 0 or more.`;
    return { cost_micro_usd: Math.round(usd * 1e6), margin_bp: Math.round(margin * 100), fixed_credits_milli: null };
  }
  const preview = (x: Draft) => { const r = rate(x, ""); return typeof r === "string" ? "—" : credits(perUnitMilli(r, priceMicro)); };
  function save(e: Event) {
    e.preventDefault();
    if (busy || saving) return;
    const out: Partial<Record<TokenUnit, UnitRate>> = {};
    for (const u of TOKEN_UNITS) {
      const r = rate(d[u.id], u.label);
      if (typeof r === "string") { err = r; return; }
      out[u.id] = r;
    }
    err = null;
    onSave(out as Record<TokenUnit, UnitRate>);
  }
</script>

<form class="rcard" bind:this={form} aria-label={`Rate card for ${model}`} onsubmit={save}>
  <div class="rcard-units">
    {#each TOKEN_UNITS as u (u.id)}
      {@const x = d[u.id]}
      <fieldset class="rcard-u">
        <legend>{u.label}</legend>
        <label for={`${id}-${u.id}-how`}>Charged at</label>
        <select id={`${id}-${u.id}-how`} value={x.fixed ? "fixed" : "cost"} onchange={e => { d[u.id].fixed = e.currentTarget.value === "fixed"; }}>
          <option value="cost">Cost + margin</option><option value="fixed">Fixed credits</option>
        </select>
        {#if x.fixed}
          <label for={`${id}-${u.id}-credits`}>Credits per million tokens</label>
          <input id={`${id}-${u.id}-credits`} type="number" min="0" step="any" inputmode="decimal" bind:value={d[u.id].credits} />
        {:else}
          <label for={`${id}-${u.id}-usd`}>Cost, USD per million tokens</label>
          <input id={`${id}-${u.id}-usd`} type="number" min="0" step="any" inputmode="decimal" bind:value={d[u.id].usd} />
          <label for={`${id}-${u.id}-margin`}>Margin, %</label>
          <input id={`${id}-${u.id}-margin`} type="number" min="0" step="any" inputmode="decimal" bind:value={d[u.id].margin} />
        {/if}
        <small class="rcard-eq">= <b>{preview(x)}</b> credits per million tokens</small>
      </fieldset>
    {/each}
  </div>
  {#if err}<p class="err" role="alert">{err}</p>{/if}
  <div class="acts">
    <button type="submit" class="primary" aria-disabled={busy || saving}>{saving ? "Saving…" : saveLabel}</button>
    {#if onCancel}<button type="button" onclick={onCancel}>Cancel</button>{/if}
  </div>
</form>

<style>
  :global {
    .rcard { display: grid; gap: 10px; min-width: 0; }
    .rcard-units { display: grid; grid-template-columns: repeat(auto-fill, minmax(200px, 1fr)); gap: 10px 14px; }
    .rcard-u { display: grid; gap: 4px; align-content: start; min-width: 0; margin: 0; padding: 8px 10px 10px; border: 1px solid var(--border); border-radius: var(--radius-sm); }
    .rcard-u legend { padding: 0 4px; font-weight: 600; font-size: 13px; }
    .rcard-u label { font-size: 12px; font-weight: 600; color: var(--muted); }
    .rcard-u input, .rcard-u select { width: 100%; min-width: 0; }
    .rcard-eq { font-size: 12px; color: var(--muted); }
  }
</style>
