<svelte:options css="injected" />

<script lang="ts">
  // Admin › Usage (spec 2026-10-21-claxbot §7, §10): one week's credits, the
  // revenue the paid ones stand for (credits × price), what they cost, what
  // the free tier cost, and the gross margin before and after the free
  // tier; then the totals by person and by cost line. The week is
  // `?week=<start>`, else this one; the arrows walk back and forward.
  import { onMount } from "svelte";
  import { refusal } from "../people/refusal";
  import { type AdminUsage, adminUsage } from "./api";
  import { BASE_UNITS, credits, margins, money, percent, rangeFrom, tokens } from "./model";

  let { now }: { now?: () => Date } = $props();
  let u = $state<AdminUsage | null>(null);
  let err = $state<string | null>(null);
  let week = $state(new URLSearchParams(location.search).get("week") ?? undefined);
  const m = $derived(u ? margins(u) : null);
  const day = 86_400_000;
  const load = () => { err = null; void adminUsage(week).then(x => { u = x; }, e => { err = refusal(e); }); };
  function walk(by: number) {
    const start = u?.week.start ?? rangeFrom("week", now?.() ?? new Date())!;
    week = new Date(new Date(start).getTime() + by * 7 * day).toISOString();
    const q = new URLSearchParams(location.search);
    q.set("week", week);
    history.replaceState(history.state, "", `${location.pathname}?${q}`);
    load();
  }
  const fmtDay = (iso: string) => new Date(iso).toLocaleDateString("en-US", { month: "short", day: "numeric", timeZone: "UTC" });
  onMount(load);
</script>

{#if err}<p class="err" role="alert">{err}</p>{/if}
{#if u}
  <div class="acts uw">
    <button type="button" class="icon" aria-label="The week before" onclick={() => walk(-1)}>‹</button>
    <b>Week of {fmtDay(u.week.start)} – {fmtDay(new Date(new Date(u.week.end).getTime() - day).toISOString())}</b>
    <button type="button" class="icon" aria-label="The week after" onclick={() => walk(1)}>›</button>
  </div>
  <div class="stats">
    <div class="stat"><b>{credits(u.paid_milli)}</b><span>Credits spent (paid) · {money(u.revenue_micro_usd)} revenue</span></div>
    <div class="stat"><b>{credits(u.free_milli)}</b><span>Credits spent (free tier) · {money(u.free_cost_micro_usd)} our cost</span></div>
    <div class="stat"><b>{money(u.cost_micro_usd)}</b><span>Our cost of the paid credits</span></div>
    <div class="stat"><b>{u.runs ?? 0}</b><span>Runs · {u.refused ?? 0} refused at the limit</span></div>
    <div class="stat"><b>{m?.before === null || !m ? "—" : percent(m.before)}</b><span>Gross margin · after the free tier: {m?.after === null || !m ? "—" : percent(m.after)}</span></div>
  </div>
  <section class="bcard" aria-labelledby="us-p">
    <h3 id="us-p">By person</h3>
    <div class="scroll">
      <table>
        <thead><tr><th>Person</th><th class="num">Runs</th><th class="num">Tokens</th><th class="num">Credits</th><th class="num">Of which free</th><th class="num">Our cost</th></tr></thead>
        <tbody>
          {#each u.by_person as r (r.person)}
            <tr><td>{r.name ?? r.person}</td><td class="num">{r.runs}</td><td class="num">{tokens(r.tokens)}</td><td class="num">{credits(r.credits_milli)}</td><td class="num">{credits(r.free_milli)}</td><td class="num">{money(r.cost_micro_usd)}</td></tr>
          {:else}<tr><td colspan="6" class="note">No one used ClaxBot this week.</td></tr>{/each}
        </tbody>
      </table>
    </div>
  </section>
  <section class="bcard" aria-labelledby="us-l">
    <h3 id="us-l">By cost line</h3>
    <div class="scroll">
      <table>
        <thead><tr><th>Cost line</th><th class="num">Quantity</th><th class="num">Credits</th><th class="num">Our cost</th></tr></thead>
        <tbody>
          {#each u.by_line as l (l.line)}
            <tr><td>{l.label}</td><td class="num">{l.qty.toLocaleString("en-US")} <small>{BASE_UNITS[l.unit] ?? l.unit}</small></td><td class="num">{credits(l.credits_milli)}</td><td class="num">{money(l.cost_micro_usd)}</td></tr>
          {:else}<tr><td colspan="4" class="note">Nothing was metered this week.</td></tr>{/each}
        </tbody>
      </table>
    </div>
  </section>
{:else if !err}
  <p class="note" role="status">Loading this week's usage…</p>
{/if}

<style>
  :global { .uw b { font-size: 14px; } }
</style>
