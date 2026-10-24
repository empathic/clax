<svelte:options css="injected" />

<script lang="ts">
  import { refusal } from "./refusal";
  // One person on People (spec 2026-10-08-multi-agent-multi-person §3.4,
  // §7.4, §9.5, §17): their machines with key fingerprints, and their live
  // agents by machine. For the owner (`view` "owner"): the person's preset,
  // Revoke, Disconnect per machine, raising one agent (an elevation at every
  // artifact or at one, until its session ends), ending one, renaming one,
  // what an artifact's creator raised one with (O11, read-only: a raise is
  // its granter's to remove), the approvals of theirs the owner made
  // ("Their content reaching your agents", with Revoke, and Always allow),
  // what waits on the owner, and their approvals of the owner's content
  // ("Approvals of your content", O12, read-only), and how much they made
  // (pages and storage: counts, never which). A revoked person's card is
  // history: their name, when they were revoked, what they made, and on a
  // hosted Clax Reinstate, which serves their pages again (they sign in
  // again themselves). For the viewer themself
  // (`view` "self"): narrowing a machine's agents or one agent, renaming
  // and ending their agents, with who raised an agent with what shown
  // beside its narrowing. An agent of the
  // daemon's own machine (no grant) acts with the token, which rights do not
  // limit: it gets no rights controls (the daemon refuses them,
  // `token_agent`). Each change goes to the daemon at once; a refusal is
  // shown as the daemon words it. Names are text.
  import { tick } from "svelte";
  import type { PeopleAgent, PeopleApproval, PeopleElevation, PeopleGrant, PeopleMade, PeopleRequest, Rights, PeopleGitHub } from "../api";
  import * as api from "./api";
  import { madeText } from "../format";
  import { agentCaps, CAPS, elevation, elevationsOf, MACHINE_BOUNDARY, machineCaps, narrowAgent, narrowGrant, othersRaises, ownerRaisedAt, PRESET_NAMES, presetOf, raiseCaps, waitText } from "./model";

  type Who = { public_id: string | null; display_name: string | null; revoked_at: string | null; rights: Rights; grants: PeopleGrant[]; agents: PeopleAgent[]; github?: PeopleGitHub | null; made?: PeopleMade };
  type Props = {
    view: "owner" | "self";
    who: Who;
    /** The card is the owner's own. */
    owner?: boolean;
    /** The owner's approvals of this person's content (`view` "owner"). */
    approvals?: PeopleApproval[];
    /** Open requests for this person's content, waiting on the owner. */
    waiting?: PeopleRequest[];
    /** This person's approvals of the owner's content (`view` "owner"). */
    ofYours?: PeopleApproval[];
    /** The live artifacts, by ID and title: where a raise can apply, and
     * how a raise names its artifact; null until asked for. */
    artifacts?: { id: string; title: string }[] | null;
    /** Asks for `artifacts` (a raise panel opened). */
    needArtifacts?(): void;
    /** The daemon is a hosted Clax: a revoked person can be reinstated. */
    hosted?: boolean;
    onChanged(): void;
  };
  // No defaults: a default would load Svelte's prop helper, which the
  // artifact view's entry then shares with this chunk.
  let { view, who, owner, approvals, waiting, ofYours, artifacts, needArtifacts, hosted, onChanged }: Props = $props();
  const aps = $derived(approvals ?? []);
  const yours = $derived(ofYours ?? []);
  const wait = $derived(waiting ?? []);
  // A change in flight: further changes are ignored until it answers. No
  // control is disabled meanwhile, so the keyboard's focus stays on the one
  // just used.
  let busy = $state(false);
  let err = $state<string | null>(null);
  let renaming = $state<string | null>(null);
  let newName = $state("");
  // Where the owner raises each agent (by handle): `*`, or an artifact ID.
  let raiseAt = $state<Record<string, string>>({});
  const name = $derived(who.display_name ?? (view === "self" ? "You" : "Unnamed"));
  const machines = $derived(who.grants.filter(g => g.status === "approved" && !g.ended_at));
  const grantOf = (a: PeopleAgent) => who.grants.find(g => g.id === a.grant_id) ?? null;
  const preset = $derived(presetOf(who.rights));
  /** An artifact a raise names, by title: "every artifact" for `*`. */
  const where = (loc: string) => (loc === "*" ? "every artifact" : `“${artifacts?.find(x => x.id === loc)?.title ?? loc}”`);
  /** What the owner raised agent `a` with, one line per action and place. */
  /** A capability raised at `loc`: asking has no place, so it names none. */
  const raisedAs = (c: { type: string; label: string }, loc: string) => (c.type === "clax-ask" ? c.label : `${c.label} on ${where(loc)}`);
  const raises = (a: PeopleAgent) => CAPS.flatMap(c => ownerRaisedAt(a, c).map(loc => raisedAs(c, loc)));
  /** What an artifact's creator raised an agent with, one line per action. */
  const creatorRaise = (e: PeopleElevation) =>
    CAPS.flatMap(c => e.access.filter(x => x.type === c.type && x.actions.includes(c.action)).flatMap(x => (x.locations ?? ["*"]).map(loc => raisedAs(c, loc))));
  const byName = (g: PeopleElevation["granter"]) => (g.owner ? "the owner" : g.name);
  /** The note on a raised capability, in its person's view: who raised it
   * and where. The agent's narrowing caps every raise, so off here it is off
   * where raised too; an action its person never had is not theirs to turn
   * off, only to take away by removing the raise. */
  const raisedNote = (c: { by: { who: PeopleElevation["granter"]; at: string[] }[]; on: boolean; locked: boolean }) => {
    const names = c.by.map(r => `${byName(r.who)}${r.at.includes("*") ? "" : ` on ${r.at.map(where).join(", ")}`}`).join("; by ");
    if (c.locked) return `Raised by ${names} until this session ends; remove the raise to take it away`;
    return c.on ? `Raised by ${names} until this session ends` : `Raised by ${names}, but off while this is off`;
  };
  /** The heading of the section event `e`'s control is in: where the
   * keyboard's focus goes when that control goes. */
  const heading = (e: Event) => (e.currentTarget as Element).closest("section")?.querySelector("h4") ?? null;
  const focus = (el: Element | null | undefined) => { if (el instanceof HTMLElement) el.focus(); };
  const putBack = (el?: HTMLInputElement | HTMLSelectElement) => {
    if (el instanceof HTMLInputElement) el.checked = !el.checked;
    else if (el) el.value = preset ?? "";
  };
  /** Runs one change, after asking `ask` when given; then the page
   * refetches and the focus moves to `to`, if given. A refused change, or
   * one made while another is in flight, puts control `el` back as it was. */
  async function act(fn: () => Promise<unknown>, o: { ask?: string; el?: HTMLInputElement | HTMLSelectElement; to?: Element | null } = {}) {
    if (busy) { putBack(o.el); return false; }
    if (o.ask && !confirm(o.ask)) return false;
    busy = true; err = null;
    try { await fn(); onChanged(); focus(o.to); return true; } catch (e) {
      err = refusal(e);
      putBack(o.el);
      return false;
    } finally { busy = false; }
  }
  /** Turns a capability on or off from its checkbox's change. */
  const toggle = (e: Event, fn: (on: boolean) => Promise<unknown>) => { const el = e.currentTarget as HTMLInputElement; void act(() => fn(el.checked), { el }); };
  const SCOPE = { everywhere: "Allowed everywhere", artifact: "Allowed on one page", thread: "Allowed in one thread" } as const;
  /** An approval's scope, naming its page when the daemon names it. */
  const scopeOf = (ap: PeopleApproval) => !ap.artifact_title || ap.scope === "everywhere" ? SCOPE[ap.scope]
    : ap.scope === "artifact" ? `Allowed on “${ap.artifact_title}”` : `Allowed in one thread on “${ap.artifact_title}”`;
  const everywhere = $derived(aps.some(a => a.scope === "everywhere"));
  /** Rename's button for agent `handle` in `card`, once it shows again. */
  async function closeRename(handle: string, card: Element | null) {
    renaming = null;
    await tick();
    focus([...card?.querySelectorAll<HTMLElement>("[data-rename]") ?? []].find(b => b.dataset.rename === handle));
  }
  async function openRename(e: Event, a: PeopleAgent) {
    const card = (e.currentTarget as Element).closest("article");
    renaming = a.handle; newName = a.name;
    await tick();
    card?.querySelector<HTMLInputElement>(".rename input")?.focus();
  }
  const save = async (e: Event, a: PeopleAgent) => {
    const card = (e.currentTarget as Element).closest("article");
    if (await act(() => api.rename(a.handle, newName.trim()))) await closeRename(a.handle, card);
  };
</script>

{#snippet raiseLine(a: PeopleAgent, e: PeopleElevation)}
  <div class="line raised-by">
    <span class="note by">Raised by {byName(e.granter)} until this session ends: {creatorRaise(e).join(", ")}</span>
    <span class="sp"></span>
    <button type="button" class="ghost" aria-label="Remove {byName(e.granter)}'s raise of {a.name}" onclick={ev => act(() => api.removeRaise(a.handle, e.granter.public_id), { to: heading(ev) })}>Remove</button>
  </div>
{/snippet}

<article class="pcard person" data-person={who.public_id} aria-busy={busy}>
  <header>
    <h3 tabindex="-1">{name}{#if view === "self" && who.display_name}<span class="you">{" (you)"}</span>{/if}</h3>
    {#if who.github}<a class="ghlogin" href="https://github.com/{encodeURIComponent(who.github.login)}" rel="noopener noreferrer" target="_blank" data-testid="github-login">@{who.github.login}</a>{/if}
    {#if who.revoked_at}<span class="meta">revoked {who.revoked_at.slice(0, 10)}</span>{/if}
    <span class="sp"></span>
    {#if who.revoked_at}
      {#if hosted}<button type="button" aria-label="Reinstate {name}" onclick={() => act(() => api.reinstatePerson(who.public_id!))}>Reinstate</button>{/if}
    {:else if view === "owner"}
      <label class="pick">Rights
        <select class="preset" onchange={e => { const el = e.currentTarget; if (el.value && who.public_id) void act(() => api.setRights(who.public_id!, el.value), { el }); }}>
          {#if !preset}<option value="" disabled selected>Custom</option>{/if}
          {#each Object.keys(PRESET_NAMES) as v (v)}<option value={v} selected={v === preset}>{PRESET_NAMES[v]}</option>{/each}
        </select>
      </label>
      <button type="button" class="danger" aria-label="{who.github ? "Block" : "Revoke"} {name}" onclick={e => act(() => api.revokePerson(who.public_id!), { ask: who.github ? `Block ${name}? They are signed out, their machines disconnect and their agents end, and their GitHub account @${who.github.login} cannot sign in or sign up again. What they wrote stays theirs.` : `Revoke ${name}? Their machines disconnect and their agents end. What they wrote stays theirs.`, to: (e.currentTarget as Element).closest(".ppl")?.querySelector("h2") })}>{who.github ? "Block" : "Revoke"}</button>
    {/if}
  </header>
  {#if err}<p class="err" role="alert">{err}</p>{/if}
  {#if who.made}<p class="meta">{madeText(who.made)}</p>{/if}
  {#if !who.revoked_at}
  <section class="part">
    <h4 tabindex="-1">Machines</h4>
    {#each machines as g (g.id)}
      <div class="machine">
        <div class="line">
          <span class="mname">{g.client.name}</span>
          <span class="key">key {g.client.key ?? "unknown"}</span>
          <span class="ip">{g.client.peer_ip}</span>
          <span class="sp"></span>
          {#if view === "owner"}<button type="button" class="ghost" aria-label="Disconnect {g.client.name}" onclick={e => act(() => api.revokeGrant(g.id), { ask: `Disconnect ${g.client.name}? Its agents end.`, to: heading(e) })}>Disconnect</button>{/if}
        </div>
        {#if view === "self"}
          <details class="narrow">
            <summary>What its agents may do</summary>
            <div class="caps">
              {#each machineCaps(who.rights, g) as c (c.key)}
                <label><input type="checkbox" checked={c.on} onchange={e => toggle(e, on => api.narrowGrant(g.id, narrowGrant(who.rights, g, c.key, on)))} /> {c.label}</label>
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
    <h4 tabindex="-1">Agents</h4>
    {#each who.agents as a (a.handle)}
      {@const g = grantOf(a)}
      {@const loc = raiseAt[a.handle] ?? "*"}
      <div class="agent">
        <div class="line">
          {#if renaming === a.handle}
            <form class="rename" onsubmit={e => { e.preventDefault(); void save(e, a); }}>
              <input aria-label="New name for {a.name}" value={newName} oninput={e => { newName = e.currentTarget.value; }} onkeydown={e => { if (e.key === "Escape") void closeRename(a.handle, e.currentTarget.closest("article")); }} maxlength="32" />
              <button type="submit" disabled={!newName.trim()}>Save</button>
              <button type="button" class="ghost" onclick={e => closeRename(a.handle, e.currentTarget.closest("article"))}>Cancel</button>
            </form>
          {:else}
            <span class="aname">{a.name}</span>
          {/if}
          <span class="meta">{a.harness} · {g ? g.client.name : "this machine"}</span>
          <span class="acts">
            {#if renaming !== a.handle}<button type="button" class="ghost" data-rename={a.handle} aria-label="Rename {a.name}" onclick={e => openRename(e, a)}>Rename</button>{/if}
            <button type="button" class="ghost" aria-label="End {a.name}" onclick={e => act(() => api.endAgent(a.handle), { ask: `End ${a.name}? Its questions are withdrawn and its threads released.`, to: heading(e) })}>End</button>
          </span>
        </div>
        {#if !a.grant_id}
          <p class="note">On this machine: it acts with Clax's own token, which rights do not limit.</p>
        {:else if view === "self"}
          {#each elevationsOf(a) as e (e.granter.public_id ?? "owner")}
            {@render raiseLine(a, e)}
          {/each}
          <details class="narrow">
            <summary>What this agent may do</summary>
            <p class="note">{MACHINE_BOUNDARY}</p>
            <div class="caps">
              {#each agentCaps(who.rights, g, a) as c (c.key)}
                <label title={c.machineOff && !c.locked ? "Off for the whole machine" : undefined}><input type="checkbox" checked={c.on} disabled={c.locked || c.machineOff} onchange={e => toggle(e, on => api.narrowAgent(a.handle, narrowAgent(who.rights, g, a, c.key, on)))} /> <span>{c.label}{#if c.by.length}<span class="raised">{raisedNote(c)}</span>{/if}</span></label>
              {/each}
            </div>
          </details>
        {:else}
          {#each othersRaises(a) as e (e.granter.public_id)}
            {@render raiseLine(a, e)}
          {/each}
          <details class="raise" ontoggle={e => { if (e.currentTarget.open) needArtifacts?.(); }}>
            <summary>Raise this agent{a.elevate ? " (raised)" : ""}</summary>
            <p class="note">{MACHINE_BOUNDARY} A raise lasts until the agent's session ends.</p>
            {#if a.elevate}<ul class="raises">{#each raises(a) as r (r)}<li>{r}</li>{/each}</ul>{/if}
            <label class="pick where">Where
              <select onchange={e => { raiseAt[a.handle] = e.currentTarget.value; }}>
                <option value="*" selected={loc === "*"}>Every artifact</option>
                {#each artifacts ?? [] as x (x.id)}<option value={x.id} selected={x.id === loc}>{x.title}</option>{/each}
              </select>
            </label>
            <div class="caps">
              {#each raiseCaps(who.rights, g, a, loc) as c (c.key)}
                <label title={c.wider ? "Raised at every artifact" : undefined}><input type="checkbox" checked={c.on} disabled={c.has || c.wider || (c.capped && !c.on)} onchange={e => toggle(e, on => api.elevate(a.handle, elevation(a, c.key, on, loc)))} /> <span>{c.label}{#if c.capped}<span class="raised">Off by {name}'s narrowing: a raise gives nothing while it is off</span>{/if}</span></label>
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
      <h4 tabindex="-1">Their content reaching your agents</h4>
      {#each aps as ap (ap.id)}
        <div class="line"><span>{scopeOf(ap)}</span><span class="sp"></span><button type="button" class="ghost" aria-label="Revoke approval of {name}'s content: {scopeOf(ap)}" onclick={e => act(() => api.revokeApproval(ap.id), { to: heading(e) })}>Revoke</button></div>
      {:else}
        <p class="none">Not yet: your agents ask you first.</p>
      {/each}
      {#if !everywhere && who.public_id}<button type="button" aria-label="Always allow {name}" onclick={e => act(() => api.allow(who.public_id!), { to: heading(e) })}>Always allow</button>{/if}
      {#if wait.length}
        <h4>Waiting</h4>
        {#each wait as r (r.id)}
          <a class="line wait" href="/inbox?approval={encodeURIComponent(r.id)}"><span>{waitText(r, name)}</span></a>
        {/each}
      {/if}
    </section>
    <section class="part approvals yours">
      <h4 tabindex="-1">Approvals of your content</h4>
      {#each yours as ap (ap.id)}
        <div class="line yours"><span>{scopeOf(ap)}{#if ap.artifact_id && ap.artifact_title}{" · "}<a href={`/a/${encodeURIComponent(ap.artifact_id)}${ap.thread_id ? `?thread=${encodeURIComponent(ap.thread_id)}` : ""}`}>{ap.thread_id ? "Open the thread" : "Open the page"}</a>{/if}</span></div>
      {:else}
        <p class="none">Not yet: their agents see your content once they allow it.</p>
      {/each}
    </section>
  {/if}
  {/if}
</article>
