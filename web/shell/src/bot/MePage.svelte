<svelte:options css="injected" />

<script lang="ts">
  // `/me` (spec 2026-10-21-claxbot D11, §7, §11.7): this person's ClaxBot
  // this week (credits against their allowance, by cost line), the model key
  // their runs use (their own Anthropic key, set, replaced or removed here,
  // write-only, and the switch to use it; off, the instance's), and their
  // runs, each linking to its artifact with its run panel open. Shown to
  // anyone the daemon answers `GET /api/me/bot` for; elsewhere it says
  // ClaxBot is not on this Clax.
  import { onMount } from "svelte";
  import { refusal } from "../people/refusal";
  import { type MeBot, type MeUsage, clearOwnKey, meUsage, setOwnKey, useOwnKey } from "./api";
  import { refresh } from "./me";
  import { UNITS, credits, meter, resetDay, runStatus, tokens } from "./model";
  import BotPage from "./BotPage.svelte";
  import KeyField from "./KeyField.svelte";
  import Meter from "./Meter.svelte";
  import Switch from "./Switch.svelte";

  let { now }: { now?: () => Date } = $props();
  let me = $state<MeBot | null>(null);
  let none = $state(false);
  let usage = $state<MeUsage | null>(null);
  let err = $state<string | null>(null);
  let busy = $state(false);
  const m = $derived(me ? meter(me.week) : null);
  const at = (iso: string) => new Date(iso).toLocaleString("en-US", { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
  const take = (x: MeBot | null) => { if (x) me = x; else none = true; };
  async function change(fn: () => Promise<MeBot>) {
    busy = true; err = null;
    try { await fn(); take(await refresh()); } catch (e) { err = refusal(e); } finally { busy = false; }
  }
  onMount(() => {
    document.title = "You · Clax";
    void refresh().then(take, e => { err = refusal(e); });
    void meUsage().then(u => { usage = u; }, () => {});
  });
</script>

<BotPage title="You" lede="Your ClaxBot this week and the key it uses for you. A credit is one unit of spend: script compute always, and model tokens on the instance's key.">
  {#if err}<p class="err" role="alert">{err}</p>{/if}
  {#if none}
    <p class="note">ClaxBot is not on this Clax.</p>
  {:else if me && m}
    <section class="bcard" aria-labelledby="me-w">
      <h3 id="me-w">This week</h3>
      {#if !me.enabled}<p>ClaxBot is not turned on for you. An admin can turn it on.</p>{/if}
      <div class="mew">
        <b>{credits(m.spent)} <span class="note">of {credits(m.total)} credits</span></b>
        <Meter {m} label="Your credits this week" />
        <span class="note">Resets {resetDay(me.week.end)}{me.week.lift_milli ? ` · includes a lift of ${credits(me.week.lift_milli)}` : ""}{me.unenforced ? " · not enforced" : ""}</span>
      </div>
      {#if usage?.totals.length}
        <div class="stats">
          {#each usage.totals as t (t.line)}<div class="stat"><b>{credits(t.credits_milli)}</b><span>{t.label ?? t.line} · {t.qty.toLocaleString("en-US")} {t.unit ? UNITS[t.unit as keyof typeof UNITS] ?? t.unit : ""}</span></div>{/each}
        </div>
      {/if}
    </section>
    <section class="bcard" aria-labelledby="me-k">
      <h3 id="me-k">Model key</h3>
      <p>Your own Anthropic key pays for your tokens; script compute still spends credits. Off, ClaxBot uses the instance's key and tokens spend credits too.</p>
      <div class="mek">
        <Switch checked={me.key.use_own} label="Use my own Anthropic key" disabled={!me.key.own?.set} busy={busy} describedby="me-k-why" onchange={on => change(() => useOwnKey(on))} />
        {#if !me.key.own?.set}<small id="me-k-why" class="note">Set a key first.</small>{/if}
        {#if me.key.own?.problem}<p class="note mek-problem" role="alert">{me.key.own.problem}</p>{/if}
        <KeyField info={me.key.own} label="Your Anthropic key" clearLabel="Remove" {now} onSave={k => change(() => setOwnKey(k))} onClear={() => change(clearOwnKey)} />
        <small class="note mek-host">Stored encrypted; ClaxBot sends it only to {#if me.provider_host}<b>{me.provider_host}</b>, the provider a key set now is bound to{:else}the provider's configured address{/if}.</small>
      </div>
    </section>
    <section class="bcard" aria-labelledby="me-r">
      <h3 id="me-r">Runs</h3>
      <div class="scroll">
        <table class="mer">
          <thead><tr><th>Run</th><th>Artifact</th><th>Status</th><th class="num">Scripts</th><th class="num">Compute</th><th class="num">Tokens</th><th>Key</th><th class="num">Credits</th></tr></thead>
          <tbody>
            {#each usage?.runs ?? [] as r (r.id)}
              <tr>
                <td>{at(r.started_at)}</td>
                <td>{#if r.artifact_id}<a href={`/a/${encodeURIComponent(r.artifact_id)}#run=${encodeURIComponent(r.id)}`}>{r.artifact_title ?? r.artifact_id}</a>{:else}—{/if}</td>
                <td>{runStatus({ status: r.status })}</td>
                <td class="num">{r.scripts}</td>
                <td class="num">{(r.cpu_gb_s ?? 0).toLocaleString("en-US", { maximumFractionDigits: 1 })} CPU·GB-s</td>
                <td class="num">{tokens(r.tokens)}</td>
                <td>{r.key_source === "own" ? "Yours" : "Instance"}</td>
                <td class="num">{credits(r.credits_milli)}</td>
              </tr>
            {:else}<tr><td colspan="8" class="note">{usage ? "No runs this week." : "Loading…"}</td></tr>{/each}
          </tbody>
        </table>
      </div>
      <p class="note"><a href="/audit?actor=claxbot">Everything ClaxBot did for you</a>, in the audit log.</p>
    </section>
  {:else if !err}
    <p class="note" role="status">Loading…</p>
  {/if}
</BotPage>

<style>
  :global {
    .mew { display: grid; gap: 6px; max-width: 520px; margin-bottom: 12px; }
    .mew b { font-size: 20px; font-variant-numeric: tabular-nums; }
    .mek { display: grid; gap: 10px; max-width: 560px; }
    .mer { min-width: 640px; }
    .bpg .mer a { text-decoration: underline; text-underline-offset: 2px; }
  }
</style>
