<svelte:options css="injected" />

<script lang="ts">
  // One person on People (spec 2026-10-08-multi-agent-multi-person §3.4,
  // §7.4, §9.5, §17): their machines with key fingerprints, and their live
  // agents by machine. For the owner (`view` "owner"): the person's preset,
  // Revoke, Disconnect per machine, raising one agent (an elevation at every
  // artifact, until its session ends), ending one, renaming one, and the
  // approvals of theirs the owner made ("Their content reaching your
  // agents", with Revoke, and Always allow) and what waits on the owner.
  // For the viewer themself (`view` "self"): narrowing a machine's agents
  // or one agent, renaming and ending their agents. An agent of the
  // daemon's own machine (no grant) acts with the token, which rights do not
  // limit: it gets no rights controls (the daemon refuses them,
  // `token_agent`). Each change goes to the daemon at once; a refusal is
  // shown as the daemon words it. Names are text.
  import type { PeopleAgent, PeopleApproval, PeopleGrant, PeopleRequest, Rights } from "../api";
  import * as api from "./api";
  import { agentCaps, elevation, MACHINE_BOUNDARY, machineCaps, narrowAgent, narrowGrant, PRESET_NAMES, presetOf, raiseCaps } from "./model";

  type Who = { public_id: string | null; display_name: string | null; revoked_at: string | null; rights: Rights; grants: PeopleGrant[]; agents: PeopleAgent[] };
  type Props = {
    view: "owner" | "self";
    who: Who;
    /** The card is the owner's own. */
    owner?: boolean;
    /** The owner's approvals of this person's content (`view` "owner"). */
    approvals?: PeopleApproval[];
    /** Open requests for this person's content, waiting on the owner. */
    waiting?: PeopleRequest[];
    onChanged(): void;
  };
  // No defaults: a default would load Svelte's prop helper, which the
  // artifact view's entry then shares with this chunk.
  let { view, who, owner, approvals, waiting, onChanged }: Props = $props();
  const aps = $derived(approvals ?? []);
  const wait = $derived(waiting ?? []);
  let busy = $state(false);
  let err = $state<string | null>(null);
  let renaming = $state<string | null>(null);
  let newName = $state("");
  const name = $derived(who.display_name ?? (view === "self" ? "You" : "Unnamed"));
  const machines = $derived(who.grants.filter(g => g.status === "approved" && !g.ended_at));
  const grantOf = (a: PeopleAgent) => who.grants.find(g => g.id === a.grant_id) ?? null;
  const preset = $derived(presetOf(who.rights));
  /** Runs one change, after asking `ask` when given; then the page
   * refetches. A refused change puts control `el` back as it was. */
  async function act(fn: () => Promise<unknown>, ask?: string, el?: HTMLInputElement | HTMLSelectElement) {
    if (ask && !confirm(ask)) return;
    busy = true; err = null;
    try { await fn(); onChanged(); } catch (e) {
      err = e instanceof Error ? e.message : String(e);
      if (el instanceof HTMLInputElement) el.checked = !el.checked;
      else if (el) el.value = preset ?? "";
    } finally { busy = false; }
  }
  /** Turns a capability on or off from its checkbox's change. */
  const toggle = (e: Event, fn: (on: boolean) => Promise<unknown>) => { const el = e.currentTarget as HTMLInputElement; void act(() => fn(el.checked), undefined, el); };
  const SCOPE = { everywhere: "Allowed everywhere", artifact: "Allowed on one page", thread: "Allowed in one thread" } as const;
  const everywhere = $derived(aps.some(a => a.scope === "everywhere"));
  const rename = (a: PeopleAgent) => act(async () => { await api.rename(a.handle, newName.trim()); renaming = null; });
</script>

<article class="pcard person" data-person={who.public_id}>
  <header>
    <h3>{name}{#if view === "self" && who.display_name}<span class="you">{" (you)"}</span>{/if}</h3>
    <span class="sp"></span>
    {#if view === "owner"}
      <label class="pick">Rights
        <select class="preset" disabled={busy} onchange={e => { const el = e.currentTarget; if (el.value && who.public_id) void act(() => api.setRights(who.public_id!, el.value), undefined, el); }}>
          {#if !preset}<option value="" disabled selected>Custom</option>{/if}
          {#each Object.keys(PRESET_NAMES) as v (v)}<option value={v} selected={v === preset}>{PRESET_NAMES[v]}</option>{/each}
        </select>
      </label>
      <button type="button" class="danger" disabled={busy} onclick={() => act(() => api.revokePerson(who.public_id!), `Revoke ${name}? Their machines disconnect and their agents end. What they wrote stays theirs.`)}>Revoke {name}</button>
    {/if}
  </header>
  {#if err}<p class="err" role="alert">{err}</p>{/if}

  <section class="part">
    <h4>Machines</h4>
    {#each machines as g (g.id)}
      <div class="machine">
        <div class="line">
          <span class="mname">{g.client.name}</span>
          <span class="key">key {g.client.key ?? "unknown"}</span>
          <span class="ip">{g.client.peer_ip}</span>
          <span class="sp"></span>
          {#if view === "owner"}<button type="button" class="ghost" disabled={busy} onclick={() => act(() => api.revokeGrant(g.id), `Disconnect ${g.client.name}? Its agents end.`)}>Disconnect</button>{/if}
        </div>
        {#if view === "self"}
          <details class="narrow">
            <summary>What its agents may do</summary>
            <div class="caps">
              {#each machineCaps(who.rights, g) as c (c.key)}
                <label><input type="checkbox" checked={c.on} disabled={busy} onchange={e => toggle(e, on => api.narrowGrant(g.id, narrowGrant(who.rights, g, c.key, on)))} /> {c.label}</label>
              {/each}
            </div>
          </details>
        {/if}
      </div>
    {:else}
      <p class="none">{owner ? "No other machine of yours is connected." : "No machine is connected."}</p>
    {/each}
  </section>

  <section class="part">
    <h4>Agents</h4>
    {#each who.agents as a (a.handle)}
      {@const g = grantOf(a)}
      <div class="agent">
        <div class="line">
          {#if renaming === a.handle}
            <form class="rename" onsubmit={e => { e.preventDefault(); void rename(a); }}>
              <input aria-label="New name for {a.name}" value={newName} oninput={e => { newName = e.currentTarget.value; }} maxlength="32" />
              <button type="submit" disabled={busy || !newName.trim()}>Save</button>
              <button type="button" class="ghost" onclick={() => { renaming = null; }}>Cancel</button>
            </form>
          {:else}
            <span class="aname">{a.name}</span>
          {/if}
          <span class="meta">{a.harness} · {g ? g.client.name : "this machine"}</span>
          <span class="sp"></span>
          {#if renaming !== a.handle}<button type="button" class="ghost" disabled={busy} onclick={() => { renaming = a.handle; newName = a.name; }}>Rename</button>{/if}
          <button type="button" class="ghost" disabled={busy} onclick={() => act(() => api.endAgent(a.handle), `End ${a.name}? Its questions are withdrawn and its threads released.`)}>End</button>
        </div>
        {#if !a.grant_id}
          <p class="note">On this machine: it acts with Clax's own token, which rights do not limit.</p>
        {:else if view === "self"}
          <details class="narrow">
            <summary>What this agent may do</summary>
            <p class="note">{MACHINE_BOUNDARY}</p>
            <div class="caps">
              {#each agentCaps(who.rights, g, a) as c (c.key)}
                <label title={c.machineOff ? "Off for the whole machine" : undefined}><input type="checkbox" checked={c.on} disabled={busy || c.machineOff} onchange={e => toggle(e, on => api.narrowAgent(a.handle, narrowAgent(who.rights, g, a, c.key, on)))} /> {c.label}</label>
              {/each}
            </div>
          </details>
        {:else}
          <details class="raise">
            <summary>Raise this agent{a.elevate ? " (raised)" : ""}</summary>
            <p class="note">{MACHINE_BOUNDARY} A raise lasts until the agent's session ends.</p>
            <div class="caps">
              {#each raiseCaps(who.rights, g, a) as c (c.key)}
                <label><input type="checkbox" checked={c.on} disabled={busy || c.has} onchange={e => toggle(e, on => api.elevate(a.handle, elevation(who.rights, g, a, c.key, on)))} /> {c.label}</label>
              {/each}
            </div>
          </details>
        {/if}
      </div>
    {:else}
      <p class="none">No agent is running.</p>
    {/each}
  </section>

  {#if view === "owner"}
    <section class="part approvals">
      <h4>Their content reaching your agents</h4>
      {#each aps as ap (ap.id)}
        <div class="line"><span>{SCOPE[ap.scope]}</span><span class="sp"></span><button type="button" class="ghost" disabled={busy} onclick={() => act(() => api.revokeApproval(ap.id))}>Revoke</button></div>
      {:else}
        <p class="none">Not yet: your agents ask you first.</p>
      {/each}
      {#if !everywhere && who.public_id}<button type="button" disabled={busy} onclick={() => act(() => api.allow(who.public_id!))}>Always allow {name}</button>{/if}
      {#if wait.length}
        <h4>Waiting</h4>
        {#each wait as r (r.id)}
          <a class="line wait" href="/inbox?approval={encodeURIComponent(r.id)}">{r.count} from {name}{r.agent ? ` for ${r.agent.name}` : ""}</a>
        {/each}
      {/if}
    </section>
  {/if}
</article>
