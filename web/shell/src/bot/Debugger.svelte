<svelte:options css="injected" />

<script lang="ts">
  // The Debugger (BRIEF-stint-2026-11-02-clax-debugger): the owner's one
  // place in /settings to see how this Clax is running. Each view reads a
  // route that exists today; what another seat is still building is a
  // "Not built yet — <seat>" row, never a dead control. A view whose route
  // refuses says why in its own place, and the others still show.
  import { onMount } from "svelte";
  import { refusal } from "../people/refusal";
  import {
    type AuditEvent, type ExtensionStatus, type Health, type JournalStatus, type LiveSite, type PeopleMe, type Provider,
    audit, extensionStatus, health, journalStatus, liveSites, peopleMe, provider, testProvider,
  } from "./api";

  type Load<T> = { v: T | null; err: string | null };
  const empty = <T,>(): Load<T> => ({ v: null, err: null });
  let hz = $state(empty<Health>());
  let jr = $state(empty<JournalStatus>());
  let ext = $state(empty<ExtensionStatus>());
  let me = $state(empty<PeopleMe>());
  let sites = $state(empty<{ sites: LiveSite[] }>());
  let prov = $state(empty<Provider>());
  let runs = $state(empty<{ events: AuditEvent[] }>());
  let sent = $state(empty<{ events: AuditEvent[] }>());
  let test = $state<{ ok: boolean; text: string } | null>(null);

  const load = <T,>(p: Promise<T>, set: (l: Load<T>) => void) => { void p.then(v => set({ v, err: null }), e => set({ v: null, err: refusal(e) })); };
  const since = (at: string) => {
    const s = Math.max(0, Math.round((Date.now() - Date.parse(at)) / 1000));
    if (Number.isNaN(s)) return at;
    const d = Math.floor(s / 86400), h = Math.floor((s % 86400) / 3600), m = Math.floor((s % 3600) / 60);
    return d ? `${d}d ${h}h` : h ? `${h}h ${m}m` : `${m}m`;
  };
  const who = (e: AuditEvent) => {
    const a = e.actor as unknown as { display_name?: string; name?: string; key?: string } | string | null;
    return typeof a === "string" ? a : a?.display_name ?? a?.name ?? a?.key ?? "unknown";
  };
  async function runTest() {
    test = { ok: true, text: "Testing…" };
    try {
      const r = await testProvider();
      test = r.ok ? { ok: true, text: `Connected in ${r.latency_ms} ms.` } : { ok: false, text: `The call failed${r.error ? `: ${r.error}` : "."}` };
    } catch (e) { test = { ok: false, text: refusal(e) }; }
  }
  onMount(() => {
    load(health(), l => { hz = l; });
    load(journalStatus(), l => { jr = l; });
    load(extensionStatus(), l => { ext = l; });
    load(peopleMe(), l => { me = l; });
    load(liveSites(), l => { sites = l; });
    load(provider(), l => { prov = l; });
    load(audit({ group: "bot" }), l => { runs = l; });
    load(audit({ q: "agent_sent" }), l => { sent = l; });
  });
  const grants = $derived((me.v?.grants ?? []).filter(g => !g.ended_at));
  const runEvents = $derived((runs.v?.events ?? []).filter(e => e.run).slice(0, 20));
</script>

{#snippet pending(seat: string, what: string)}
  <p class="nyb">Not built yet — {seat}: {what}</p>
{/snippet}
{#snippet failed(err: string | null)}
  {#if err}<p class="err" role="alert">{err}</p>{/if}
{/snippet}

<section class="bcard dbg" aria-labelledby="dbg-h">
  <h3 id="dbg-h">Debugger</h3>
  <p class="note">How this Clax is running, for its owner. Each part reads the daemon now; parts other work is still building say so.</p>

  <section aria-labelledby="dbg-status">
    <h4 id="dbg-status">Status</h4>
    {@render failed(hz.err)}
    {#if hz.v}
      <dl>
        <dt>Version</dt><dd>{hz.v.version}{#if hz.v.commit} (<code>{hz.v.commit.slice(0, 12)}</code>){/if}{#if hz.v.build}, build {hz.v.build}{/if}</dd>
        <dt>Up</dt><dd>{since(hz.v.started_at)}, since {hz.v.started_at}{#if hz.v.pid}, process {hz.v.pid}{/if}</dd>
      </dl>
    {/if}
    {@render failed(jr.err)}
    {#if jr.v}
      <dl>
        <dt>Toolpath journal</dt><dd>{jr.v.journal ? "on" : "off"}{#if jr.v.journal}, at <code>{jr.v.dir}</code>{#if jr.v.lag_ms != null}, {jr.v.lag_ms} ms behind{/if}{/if}</dd>
        {#if jr.v.last_error}<dt>Journal error</dt><dd class="err">{jr.v.last_error}</dd>{/if}
        {#if jr.v.warning}<dt>Journal warning</dt><dd>{jr.v.warning}</dd>{/if}
      </dl>
    {/if}
    {@render failed(ext.err)}
    {#if ext.v}
      <dl>
        <dt>Chrome extension</dt><dd>{ext.v.live_credentials ? `paired${ext.v.last_used_at ? `, last used ${ext.v.last_used_at}` : ""}` : "not paired"}</dd>
      </dl>
    {/if}
    {@render failed(me.err)}
    {#if me.v}
      <dl>
        <dt>This browser</dt>
        <dd>{me.v.person.owner ? "The owner: this Clax's own browser, or the owner's tailnet login on one of the owner's devices." : `A connected person (${me.v.person.display_name ?? me.v.person.public_id}), not the owner. If this is the owner's own Mac over the tailnet, name its node in [serve] owner_tailscale_nodes; the agent log says why it was not counted as the owner's device.`}</dd>
      </dl>
    {/if}
    {@render pending("work-clax-debugger-stint", "recent errors and warnings from agent.log, and the extension version each paired Chrome runs")}
  </section>

  <section aria-labelledby="dbg-audit">
    <h4 id="dbg-audit">Audit</h4>
    <p><a href="/audit">Open the audit trail</a>, filtered by time, kind, actor and artifact.</p>
    {@render pending("work-clax-contract-gen-stint", "following the trail live")}
  </section>

  <section aria-labelledby="dbg-sites">
    <h4 id="dbg-sites">Sites and joins</h4>
    {@render failed(sites.err)}
    {#if sites.v}
      {#if sites.v.sites.length}
        <ul>
          {#each sites.v.sites as s (s.site.key)}
            <li><b>{s.site.name}</b>{s.site.joined ? ", joined" : ""}: {s.site.origins.join(", ")}{#if s.last_activity}, active {s.last_activity}{/if}</li>
          {/each}
        </ul>
      {:else}<p class="note">No site has live pages.</p>{/if}
    {/if}
    {@render pending("work-clax-unjoin-stint", "unjoining a joined site")}
  </section>

  <section aria-labelledby="dbg-int">
    <h4 id="dbg-int">Integrations and deployments</h4>
    {@render pending("work-clax-script-stint", "which integrations and deployments are live, where, and the config or overlay version loaded")}
  </section>

  <section aria-labelledby="dbg-queues">
    <h4 id="dbg-queues">Queues and responders</h4>
    {@render failed(me.err)}
    {#if me.v}
      {#if grants.length}
        <p class="note">Service grants an outside agent can answer a queue with:</p>
        <ul>
          {#each grants as g (g.id)}
            <li><b>{g.client.name ?? "unnamed client"}</b> ({g.status}{g.via ? `, via ${g.via}` : ""}){#if g.last_used_at}, last used {g.last_used_at}{/if}</li>
          {/each}
        </ul>
      {:else}<p class="note">No live service grant.</p>{/if}
    {/if}
    {@render pending("work-clax-script-stint", "each queue's responders (ClaxBot profile or outside agent) and its recent claims and replies")}
  </section>

  <section aria-labelledby="dbg-delivery">
    <h4 id="dbg-delivery">Agent delivery</h4>
    {@render failed(sent.err)}
    {#if sent.v}
      {#if sent.v.events.length}
        <ul>{#each sent.v.events.slice(0, 10) as e (e.seq)}<li>{e.at}: {e.summary}</li>{/each}</ul>
      {:else}<p class="note">No Send-to-agent outcome is in the audit trail.</p>{/if}
    {/if}
    {@render pending("work-chitin-clax-agent-sent-stint", "the chitin relay's state, and why a thread says no agent is running to take it")}
  </section>

  <section aria-labelledby="dbg-runs">
    <h4 id="dbg-runs">ClaxBot runs</h4>
    {@render failed(runs.err)}
    {#if runs.v}
      {#if runEvents.length}
        <ul>{#each runEvents as e (e.seq)}<li>{e.at} · run <code>{e.run}</code> · {who(e)}: {e.summary}</li>{/each}</ul>
      {:else}<p class="note">No ClaxBot run is in the audit trail.</p>{/if}
    {/if}
    {@render pending("work-clax-debugger-stint", "each run's conversation streamed to the local pathbase, linked here")}
  </section>

  <section aria-labelledby="dbg-provider">
    <h4 id="dbg-provider">ClaxBot provider</h4>
    {@render failed(prov.err)}
    {#if prov.v}
      <dl>
        <dt>API</dt><dd>{prov.v.api} at <code>{prov.v.base_url}</code></dd>
        {#each Object.entries(prov.v.sources) as [k, src] (k)}<dt>{k}</dt><dd>from {typeof src === "string" ? src : JSON.stringify(src)}</dd>{/each}
      </dl>
      <div class="acts">
        <button type="button" onclick={runTest}>Test connection</button>
        <span role="status">{#if test}<span class={test.ok ? "ok" : "err"}>{test.text}</span>{/if}</span>
      </div>
    {/if}
  </section>
</section>

<style>
  :global {
    .bpg .dbg h4 { margin: 14px 0 6px; font: 600 13.5px/1.4 var(--font); }
    .bpg .dbg dl { display: grid; grid-template-columns: max-content 1fr; gap: 2px 12px; margin: 0 0 6px; }
    .bpg .dbg dt { color: var(--fg-muted); }
    .bpg .dbg dd { margin: 0; overflow-wrap: anywhere; }
    .bpg .dbg ul { margin: 0 0 6px; padding-left: 18px; }
    .bpg .dbg .nyb { margin: 4px 0; font-size: 12.5px; color: var(--fg-muted); font-style: italic; }
  }
</style>
