<svelte:options css="injected" />

<script lang="ts">
  // Admin › Pricing (spec 2026-10-21-claxbot §7.2, §7.4; docs/contract.md
  // "Pricing"): what one credit sells for and the free credits everyone gets
  // each week; each model's token rates as charged now and where each comes
  // from (its rate card set here, tokenulator.dev's price catalog, or the
  // default rates, an estimate), with a flag where a set cost drifted from
  // the catalog's; a model's rate card edited in place, added, or priced from
  // tokenulator.dev, and removed so its tokens resolve past it; the default
  // rates for unpriced models and whether a model needs an explicit price;
  // the catalog's age, refreshed on demand; and every other cost line with
  // its margin. A rate card applies on its own; Save applies the credit
  // price, the free tier, the default rates and the margins. A change
  // applies to new usage only; a value set in `config.toml` or the
  // environment is read-only here.
  import { onMount } from "svelte";
  import { refusal } from "../people/refusal";
  import { type CostLine, type ModelPricing, type Pricing, type PricingPatch, type TokenUnit, type UnitRate, deleteModelRates, priceFromTokenulator, pricing, refreshCatalog, setModelRates, setPricing } from "./api";
  import { RATE_SOURCES, TOKEN_UNITS, UNITS, credits, money, percent, perUnitMilli, readOnlySource } from "./model";
  import RateCard from "./RateCard.svelte";
  import SourceBadge from "./SourceBadge.svelte";

  type Note = { ok: boolean; text: string } | null;
  let p = $state<Pricing | null>(null);
  let err = $state<string | null>(null);
  let saved = $state("");
  let busy = $state(false);
  // The form: the credit price in dollars, the free tier and the default
  // rates in credits, each non-token line's margin in percent.
  let price = $state("");
  let free = $state("");
  let rates = $state<Record<string, string>>({});
  let strict = $state(false);
  let margin = $state<Record<string, string>>({});
  const isToken = (l: CostLine) => l.unit.startsWith("mtok_");
  const margins = (x: Pricing) => Object.fromEntries(x.lines.filter(l => !isToken(l)).map(l => [l.id, String(l.margin_bp / 100)]));
  /** Takes the pricing; `all` resets the form to it, else only lines new to the form join it. */
  function take(x: Pricing, all = true) {
    p = x;
    if (!all) { margin = { ...margins(x), ...margin }; return; }
    price = String(x.credit_price_micro_usd / 1e6);
    free = String(x.free_weekly_milli / 1000);
    rates = Object.fromEntries(TOKEN_UNITS.map(u => [u.id, String(x.default_rates[u.id] / 1000)]));
    strict = x.require_explicit_price;
    margin = margins(x);
  }
  const ro = (field: string) => readOnlySource(p?.sources[field]);
  const priceMicro = $derived(Math.round(Number(price) * 1e6));
  const marginBp = (l: CostLine) => { const n = Number(margin[l.id]); return Number.isFinite(n) ? Math.round(n * 100) : l.margin_bp; };
  const perUnit = (l: CostLine) => perUnitMilli({ ...l, margin_bp: marginBp(l) }, priceMicro);
  const others = $derived(p?.lines.filter(l => !isToken(l)) ?? []);
  async function save() {
    if (!p || busy) return;
    if (!(priceMicro > 0)) { err = "A credit's price is more than $0."; return; }
    const f = Number(free);
    if (!Number.isFinite(f) || f < 0) { err = "The free tier is a number of credits, 0 or more."; return; }
    // Token lines go back as they are: their rate cards change on their own.
    const patch: PricingPatch = { credit_price_micro_usd: priceMicro, free_weekly_milli: Math.round(f * 1000), lines: p.lines.map(l => (isToken(l) ? l : { ...l, margin_bp: marginBp(l) })) };
    const dr: Partial<Record<TokenUnit, number>> = {};
    for (const u of TOKEN_UNITS) {
      if (ro(`default_rates.${u.id}`)) continue;
      const v = rates[u.id] as string | number | null;
      const n = Number(v);
      if (v === null || String(v).trim() === "" || !Number.isFinite(n) || n < 0) { err = `The default ${u.label.toLowerCase()} rate is credits per million tokens, 0 or more.`; return; }
      dr[u.id] = Math.round(n * 1000);
    }
    if (Object.keys(dr).length) patch.default_rates = dr;
    if (!ro("require_explicit_price")) patch.require_explicit_price = strict;
    busy = true; err = null; saved = "";
    try {
      take(await setPricing(patch));
      saved = "Saved. New usage is charged at these rates.";
    } catch (e) { err = refusal(e); } finally { busy = false; }
  }

  // ---- models ----
  let editing = $state<string | null>(null);
  let mBusy = $state(false);
  let mNote = $state<Note>(null);
  const hasLine = (m: ModelPricing) => TOKEN_UNITS.some(u => m.units[u.id].source === "line");
  // The name a model's lines are labelled with ("Claude Sonnet 5.5 input tokens"), kept when its card changes.
  function nameOf(m: ModelPricing): string | undefined {
    const l = p?.lines.find(x => x.model === m.model && x.unit === "mtok_input");
    return l?.label.endsWith(" input tokens") ? l.label.slice(0, -" input tokens".length) : m.listing?.name || undefined;
  }
  /** Where `model`'s input tokens resolve now, in words. */
  function resolvesTo(x: Pricing, model: string): string {
    const s = x.models.find(m => m.model === model)?.units.input.source;
    return s === "catalog" ? "tokenulator.dev's price" : s === "default" ? "the default rates (estimated)" : "no price: runs on it are refused";
  }
  async function act(fn: () => Promise<Pricing>, ok: (x: Pricing) => string, failed?: (e: unknown) => string | null): Promise<boolean> {
    if (mBusy) return false;
    mBusy = true; mNote = null;
    try { const x = await fn(); take(x, false); mNote = { ok: true, text: ok(x) }; return true; }
    catch (e) { mNote = { ok: false, text: failed?.(e) ?? refusal(e) }; return false; }
    finally { mBusy = false; }
  }
  async function saveCard(m: ModelPricing, units: Record<TokenUnit, UnitRate>) {
    const name = nameOf(m);
    if (await act(() => setModelRates({ model: m.model, ...(name ? { name } : {}), ...units }), () => `Saved the rates of ${m.model}.`)) editing = null;
  }
  const useCatalog = (m: ModelPricing) => act(() => priceFromTokenulator(m.model, m.listing?.provider_id), () => `Priced ${m.model} at tokenulator.dev's price.`);
  const remove = (m: ModelPricing) => act(
    () => deleteModelRates(m.model),
    x => `Removed the rates set for ${m.model}: its tokens are charged at ${resolvesTo(x, m.model)}.`,
    e => (e as { code?: unknown }).code === "cost_line_in_use" ? `A run on ${m.model} has not ended yet: remove its rates once it has.` : null,
  );
  const refresh = () => act(refreshCatalog, () => "Fetched tokenulator.dev's prices.");
  const when = (iso: string) => `${new Date(iso).toLocaleString("en-US", { month: "short", day: "numeric", year: "numeric", hour: "2-digit", minute: "2-digit", hourCycle: "h23", timeZone: "UTC" })} UTC`;
  const drifted = (m: ModelPricing) => TOKEN_UNITS.filter(u => m.units[u.id].drift && m.units[u.id].catalog_micro_usd !== null)
    .map(u => `${u.label.toLowerCase()} ${money(m.units[u.id].catalog_micro_usd!)}`).join(", ");

  // ---- add a model ----
  let aModel = $state("");
  let aName = $state("");
  let aHow = $state<"catalog" | "card">("catalog");
  let aProvider = $state("");
  let aNote = $state<Note>(null);
  let aKey = $state(0);
  async function add(fn: (model: string) => Promise<Pricing>, ok: string) {
    const model = aModel.trim();
    if (!model) { aNote = { ok: false, text: "Give the model's ID." }; return; }
    if (mBusy) return;
    mBusy = true; aNote = null;
    try { take(await fn(model), false); aNote = { ok: true, text: ok.replace("{}", model) }; aModel = ""; aName = ""; aProvider = ""; aKey++; }
    catch (e) { aNote = { ok: false, text: refusal(e) }; }
    finally { mBusy = false; }
  }
  const lookUp = () => add(model => priceFromTokenulator(model, aProvider.trim() || undefined), "Priced {} from tokenulator.dev.");
  const addCard = (units: Record<TokenUnit, UnitRate>) => add(model => setModelRates({ model, ...(aName.trim() ? { name: aName.trim() } : {}), ...units }), "Added {} with the rates set.");

  onMount(() => { void pricing().then(x => take(x), e => { err = refusal(e); }); });
</script>

{#if err}<p class="err" role="alert">{err}</p>{/if}
{#if p}
  <section class="bcard" aria-labelledby="pr-c">
    <h3 id="pr-c">Credits</h3>
    <p>One credit is the unit of spend, sold at a fixed price. Free credits are charged against the same cost lines, so the free tier's cost shows in Usage.</p>
    <div class="fields">
      <div class="field"><label for="pr-price">Credit price (USD) <SourceBadge source={p.sources.credit_price_micro_usd} /></label><input id="pr-price" type="number" min="0" step="0.0001" inputmode="decimal" bind:value={price} readonly={ro("credit_price_micro_usd")} /></div>
      <div class="field"><label for="pr-free">Free credits each week <SourceBadge source={p.sources.free_weekly_milli} /></label><input id="pr-free" type="number" min="0" step="1" inputmode="numeric" bind:value={free} readonly={ro("free_weekly_milli")} /></div>
    </div>
  </section>
  <section class="bcard" aria-labelledby="pr-m">
    <h3 id="pr-m">Models</h3>
    <p>Each model's credits per million tokens as charged now, and where each rate comes from: set here, <a href="https://tokenulator.dev" target="_blank" rel="noreferrer">tokenulator.dev</a>'s price for the provider in effect, or the default rates (an estimate). A rate card applies when it is saved.</p>
    <div class="acts pr-cat">
      <span class="note">{#if p.catalog.fetched_at}tokenulator.dev prices as of {when(p.catalog.fetched_at)}{:else}No tokenulator.dev prices fetched yet{/if} ({p.catalog.provider ? `provider ${p.catalog.provider}` : "the base URL names no tokenulator.dev provider"})</span>
      <button type="button" aria-disabled={mBusy} onclick={refresh}>Refresh</button>
    </div>
    <div class="scroll">
      <table class="pr pr-m">
        <thead><tr><th>Model</th>{#each TOKEN_UNITS as u (u.id)}<th class="num">{u.label}</th>{/each}<th><span class="bot-sr">Actions</span></th></tr></thead>
        <tbody>
          {#each p.models as m (m.model)}
            <tr data-model={m.model}>
              <td>
                <b>{m.model}</b>{#each m.tiers as t (t)} <span class="pr-tier">{t}</span>{/each}
                {#if m.drift && m.listing}
                  <p class="pr-drift">tokenulator.dev lists another price: {drifted(m)} per million tokens.
                    <button type="button" class="link" aria-disabled={mBusy} onclick={() => useCatalog(m)}>Use tokenulator.dev's price</button></p>
                {/if}
              </td>
              {#each TOKEN_UNITS as u (u.id)}
                {@const r = m.units[u.id]}
                <td class="num" data-unit={u.id} class:drift={r.drift}>
                  {#if r.source && r.credits_milli_per_mtok !== null}
                    <b>{credits(r.credits_milli_per_mtok)}</b>
                    <small class="pr-how">{r.fixed_credits_milli !== null ? "fixed" : `${money(r.cost_micro_usd ?? 0)} + ${percent(r.margin_bp ?? 0)}`}</small>
                    <span class="bsrc">{RATE_SOURCES[r.source]}</span>
                  {:else}—<small class="pr-how">no price</small>{/if}
                </td>
              {/each}
              <td class="pr-acts">
                <button type="button" aria-expanded={editing === m.model} aria-label={`Edit the rates of ${m.model}`} onclick={() => { editing = editing === m.model ? null : m.model; }}>Edit</button>
                {#if hasLine(m)}<button type="button" aria-disabled={mBusy} aria-label={`Remove the rates set for ${m.model}`} onclick={() => remove(m)}>Remove</button>{/if}
              </td>
            </tr>
            {#if editing === m.model}
              <tr class="pr-edit"><td colspan={TOKEN_UNITS.length + 2}>
                <RateCard id={`pr-e-${m.model.replace(/[^\w-]/g, "_")}`} model={m.model} from={m} {priceMicro} busy={mBusy} focus onSave={u => saveCard(m, u)} onCancel={() => { editing = null; }} />
              </td></tr>
            {/if}
          {/each}
        </tbody>
      </table>
    </div>
    <p class="note" role="status">{#if mNote}<span class={mNote.ok ? "ok" : "err"}>{mNote.text}</span>{/if}</p>
  </section>
  <section class="bcard" aria-labelledby="pr-a">
    <h3 id="pr-a">Add a model</h3>
    <div class="fields">
      <div class="field"><label for="pr-a-model">Model ID</label><input id="pr-a-model" type="text" spellcheck="false" autocomplete="off" placeholder="gemini-3.5-flash" bind:value={aModel} /></div>
    </div>
    <fieldset class="pr-pick">
      <legend>Price it</legend>
      <label><input type="radio" name="pr-a-how" value="catalog" bind:group={aHow} /> From tokenulator.dev</label>
      <label><input type="radio" name="pr-a-how" value="card" bind:group={aHow} /> With a rate card</label>
    </fieldset>
    {#if aHow === "catalog"}
      <p>Sets its four token lines at the standard rates tokenulator.dev lists, at the token margin, as the provider the API and base URL name lists them, unless you give another of its provider IDs (<code>google</code>, <code>mistral</code>, <code>zai</code>, …).</p>
      <div class="fields">
        <div class="field"><label for="pr-a-provider">tokenulator.dev provider ID (optional)</label><input id="pr-a-provider" type="text" spellcheck="false" autocomplete="off" placeholder={p.catalog.provider ?? ""} bind:value={aProvider} /></div>
      </div>
      <div class="acts pr-go"><button type="button" aria-disabled={mBusy} onclick={lookUp}>{mBusy ? "Looking up…" : "Look up and price"}</button></div>
    {:else}
      <div class="fields pr-go">
        <div class="field"><label for="pr-a-name">Name (optional)</label><input id="pr-a-name" type="text" placeholder="Gemini 3.5 Flash" bind:value={aName} /></div>
      </div>
      {#key aKey}<RateCard id="pr-a" model={aModel.trim() || "a new model"} {priceMicro} busy={mBusy} saveLabel="Add the model" onSave={addCard} />{/key}
    {/if}
    <p class="note" role="status">{#if aNote}<span class={aNote.ok ? "ok" : "err"}>{aNote.text}</span>{/if}</p>
  </section>
  <section class="bcard" aria-labelledby="pr-d">
    <h3 id="pr-d">Default rates for unpriced models</h3>
    <p>A model with no rate card and no tokenulator.dev price is charged these, in credits per million tokens: an estimate, and usage shows it as one.</p>
    <div class="fields">
      {#each TOKEN_UNITS as u (u.id)}
        <div class="field"><label for={`pr-d-${u.id}`}>{u.label} <SourceBadge source={p.sources[`default_rates.${u.id}`]} /></label><input id={`pr-d-${u.id}`} type="number" min="0" step="any" inputmode="decimal" bind:value={rates[u.id]} readonly={ro(`default_rates.${u.id}`)} /></div>
      {/each}
    </div>
    <div class="pr-strict">
      <input id="pr-strict" type="checkbox" bind:checked={strict} disabled={ro("require_explicit_price")} />
      <label for="pr-strict">Require an explicit price: refuse runs on a model without a rate card</label>
      <SourceBadge source={p.sources.require_explicit_price} />
    </div>
  </section>
  <section class="bcard" aria-labelledby="pr-l">
    <h3 id="pr-l">Other cost lines</h3>
    <div class="scroll">
      <table class="pr">
        <thead><tr><th>Cost line</th><th>Unit</th><th class="num">Our cost</th><th class="num">Margin</th><th class="num">Credits per unit</th><th>Applies</th></tr></thead>
        <tbody>
          {#each others as l (l.id)}
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
    .pr-m td.num b { display: block; }
    .pr-m .pr-how { display: block; font-size: 11.5px; color: var(--muted); }
    .pr-m td.drift b { color: var(--danger); }
    .pr-tier { display: inline-block; font-size: 11px; font-weight: 600; color: var(--accent-ink); border: 1px solid currentColor; border-radius: var(--radius-xs); padding: 0 5px; vertical-align: middle; }
    .pr-drift { margin: 6px 0 0; font-size: 12.5px; color: var(--danger); }
    .pr-drift .link { background: none; border: 0; padding: 0; color: var(--accent-ink); text-decoration: underline; font: inherit; cursor: pointer; }
    .pr-acts { white-space: nowrap; text-align: end; }
    .pr-acts button + button { margin-inline-start: 4px; }
    .pr-edit > td { background: var(--hover); }
    .pr-cat { margin-bottom: 8px; }
    .pr-pick { display: flex; flex-wrap: wrap; gap: 6px 16px; margin: 12px 0 10px; padding: 0; border: 0; font-size: 13px; }
    .pr-pick legend { float: left; margin-inline-end: 8px; font-weight: 600; color: var(--muted); font-size: 12.5px; }
    .pr-go { margin: 10px 0; }
    .pr-strict { display: flex; flex-wrap: wrap; align-items: center; gap: 6px; margin-top: 12px; font-size: 13px; }
  }
</style>
