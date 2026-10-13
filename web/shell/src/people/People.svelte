<svelte:options css="injected" />

<script lang="ts">
  // `/people` (spec 2026-10-08-multi-agent-multi-person §3.1, §3.4, §9.5,
  // §9.6, §17, §18). For the owner: invites (with the command, its expiry
  // and the LAN warning; the `*unconnected` switch beside the first),
  // connection requests waiting (approved in the inbox), the switch, the
  // owner's own other machines and agents, every connected person (rights,
  // machines, agents, the approvals of theirs the owner made and what waits
  // on the owner), and revoked people as history. For a connected person:
  // themselves alone, their machines and agents, the approvals they made
  // and what waits on them. Anyone else is told People is not theirs.
  // Every change is the browser's (M20) and refetches the page; the page
  // also refetches on the stream's `people` (the owner's) and `agents`
  // topics, so a person who connects appears.
  import { onDestroy, onMount } from "svelte";
  import type { PeopleApproval, PeopleGrant, PeopleInvite, PeopleList, PeopleMe, PeopleRequest } from "../api";
  import type { EventStream } from "../stream";
  import * as api from "./api";
  import InviteDialog from "./InviteDialog.svelte";
  import { expiresIn, liveInvites } from "./model";
  import PersonCard from "./PersonCard.svelte";
  import UnconnectedSwitch from "./UnconnectedSwitch.svelte";

  let { stream, now }: { stream?: EventStream; now?: () => Date } = $props();
  const clock = () => now?.() ?? new Date();
  let me = $state<PeopleMe | null>(null);
  let denied = $state<string | null>(null);
  let err = $state<string | null>(null);
  let list = $state<PeopleList | null>(null);
  let invites = $state<PeopleInvite[]>([]);
  let pending = $state<PeopleGrant[]>([]);
  let approvals = $state<PeopleApproval[]>([]);
  let requests = $state<PeopleRequest[]>([]);
  let banner = $state<string | null>(null);
  let inviting = $state(false);
  // The open dialog is the first invite's: the switch shows in it, not apart.
  let firstInvite = $state(false);
  let gone = false;
  let off: (() => void) | null = null;
  const msg = (e: unknown) => (e instanceof Error ? e.message : String(e));

  async function load() {
    let m: PeopleMe;
    try { m = await api.me(); } catch (e) { if (!gone) denied = msg(e); return; }
    const owner = m.person.owner;
    try {
      const [l, inv, pend, aps, reqs] = await Promise.all([
        owner ? api.list() : null, owner ? api.invites() : null, owner ? api.pending() : null, api.approvals(), api.requests(),
      ]);
      if (gone) return;
      me = m; list = l; invites = inv?.invites ?? []; pending = pend?.grants ?? [];
      approvals = aps.approvals; requests = reqs.requests.filter(r => r.status === "open"); banner = reqs.banner;
      err = null;
    } catch (e) { if (!gone) { me = m; err = msg(e); } }
    if (!off && stream && !gone) off = stream.watch(owner ? ["people", "agents"] : ["agents"], e => { if (e.type !== "stream_down" && e.type !== "stream_up" && e.type !== "refused") reload(); });
  }
  // One load at a time; a change during one loads once more after it.
  let loading = false;
  let again = false;
  function reload() {
    if (loading) { again = true; return; }
    loading = true;
    void load().finally(() => { loading = false; if (again && !gone) { again = false; reload(); } });
  }
  const owner = $derived(!!me?.person.owner);
  const self = $derived(me && { public_id: me.person.public_id, display_name: me.person.display_name, revoked_at: null, rights: me.person.rights, grants: me.grants, agents: me.agents });
  const people = $derived(list?.people.filter(p => !p.revoked_at) ?? []);
  const revoked = $derived(list?.people.filter(p => p.revoked_at) ?? []);
  const live = $derived(liveInvites(invites, clock()));
  const SCOPE = { everywhere: "everywhere", artifact: "on one page", thread: "in one thread" } as const;
  async function act(fn: () => Promise<unknown>, ask?: string) {
    if (ask && !confirm(ask)) return;
    try { await fn(); reload(); } catch (e) { err = msg(e); }
  }
  onMount(() => { document.title = "People · Clax"; reload(); });
  onDestroy(() => { gone = true; off?.(); });
</script>

<div class="ppl">
  <div class="phead">
    <h2>People</h2>
    {#if owner}<span class="pcount">{people.length === 1 ? "1 person" : `${people.length} people`}</span>{/if}
    <span class="sp"></span>
    {#if owner && !inviting}<button type="button" class="primary" onclick={() => { firstInvite = invites.length === 0; inviting = true; }}>Invite a person</button>{/if}
  </div>
  {#if denied}<p class="note">{denied}</p>{/if}
  {#if err}<p class="err" role="alert">{err}</p>{/if}
  {#if banner}<p class="banner">{banner}</p>{/if}

  {#if owner && list}
    {#if inviting}
      <InviteDialog first={firstInvite} unconnected={list.unconnected} now={clock()} onMade={reload} onSwitched={reload} onClose={() => { inviting = false; }} />
    {/if}
    {#if live.length}
      <section class="pcard">
        <h3>Invites</h3>
        {#each live as i (i.id)}
          <div class="line"><span>{i.owner ? "You, on another machine" : i.person.display_name}</span><span class="meta">expires in {expiresIn(i.expires_at, clock())}</span><span class="sp"></span>
            <button type="button" class="ghost" onclick={() => act(() => api.revokeInvite(i.id))}>Revoke</button></div>
        {/each}
      </section>
    {/if}
    {#if pending.length}
      <section class="pcard requests">
        <h3>Connection requests</h3>
        {#each pending as g (g.id)}
          <div class="line"><span class="mname">{g.client.name}</span><span class="key">key {g.client.key ?? "unknown"}</span><span class="ip">{g.client.peer_ip}</span><span class="sp"></span>
            <a href="/inbox?kind=connection">Approve in the inbox</a>
            <button type="button" class="ghost" onclick={() => act(() => api.denyGrant(g.id))}>Deny</button></div>
        {/each}
      </section>
    {/if}
    {#if !inviting || !firstInvite}
      <section class="pcard">
        <h3>People on your LAN who are not connected</h3>
        <UnconnectedSwitch sw={list.unconnected} onChanged={reload} />
      </section>
    {/if}
  {/if}

  {#if self}
    <PersonCard view="self" owner={owner} who={self} onChanged={reload} />
  {/if}

  {#if owner}
    {#each people as p (p.public_id)}
      <PersonCard view="owner" who={p} approvals={approvals.filter(a => a.author.public_id === p.public_id)} waiting={requests.filter(r => r.author.public_id === p.public_id)} onChanged={reload} />
    {:else}
      {#if list}<p class="note">No one is connected yet. Invite a person to let their agents work with yours.</p>{/if}
    {/each}
    {#if revoked.length}
      <section class="pcard revoked">
        <h3>Revoked</h3>
        {#each revoked as p (p.public_id)}<div class="line"><span>{p.display_name ?? "Unnamed"}</span><span class="meta">revoked {p.revoked_at?.slice(0, 10)}</span></div>{/each}
      </section>
    {/if}
  {:else if me}
    <section class="pcard approvals">
      <h3>Who reaches your agents</h3>
      {#each approvals as ap (ap.id)}
        <div class="line"><span>{ap.author.name}</span><span class="meta">{SCOPE[ap.scope]}</span><span class="sp"></span>
          <button type="button" class="ghost" onclick={() => act(() => api.revokeApproval(ap.id))}>Revoke</button></div>
      {:else}
        <p class="none">No one yet: your agents ask you first.</p>
      {/each}
      {#if requests.length}
        <h4>Waiting</h4>
        {#each requests as r (r.id)}<div class="line"><span>{r.count} from {r.author.name}{r.agent ? ` for ${r.agent.name}` : ""}</span></div>{/each}
      {/if}
    </section>
  {/if}
</div>

<style>
  :global {
    .ppl { max-width: 880px; margin: 0 auto; min-width: 0; display: flex; flex-direction: column; gap: 14px; }
    .ppl .phead { display: flex; flex-wrap: wrap; align-items: center; gap: 8px 12px; padding-bottom: 12px; border-bottom: 1px solid var(--border); }
    .ppl h2 { margin: 0; font-size: 22px; line-height: 1.2; }
    .ppl .pcount, .ppl .meta, .ppl .ip { color: var(--muted); font-size: 12.5px; font-variant-numeric: tabular-nums; }
    .ppl .sp { flex: 1; }
    .ppl .err { margin: 0; color: var(--danger); font-size: 13px; }
    .ppl .note, .ppl .none { margin: 4px 0; color: var(--muted); font-size: 13px; }
    .ppl .banner { margin: 0; padding: 10px 12px; border-radius: var(--radius-sm); background: var(--danger-tint); font-size: 13.5px; }
    .ppl .pcard { background: var(--card); border: 1px solid var(--border); border-radius: var(--radius); padding: 14px 16px; min-width: 0; }
    .ppl .pcard > header { display: flex; flex-wrap: wrap; align-items: center; gap: 8px 12px; }
    .ppl h3 { margin: 0 0 6px; font-size: 16px; overflow-wrap: anywhere; }
    .ppl .pcard > header h3 { margin: 0; }
    .ppl .you { color: var(--muted); font-weight: 400; }
    .ppl h4 { margin: 0 0 6px; font-size: 12.5px; font-weight: 600; color: var(--muted); text-transform: uppercase; letter-spacing: .04em; }
    .ppl .part { margin-top: 14px; padding-top: 12px; border-top: 1px solid var(--border); }
    .ppl .line { display: flex; flex-wrap: wrap; align-items: center; gap: 4px 10px; min-height: 36px; min-width: 0; }
    .ppl .line > span { min-width: 0; overflow-wrap: anywhere; }
    .ppl .mname, .ppl .aname { font-weight: 600; font-size: 14px; }
    .ppl .key { font: 12px var(--mono); color: var(--muted); }
    .ppl .machine, .ppl .agent { padding: 4px 0; }
    .ppl .machine + .machine, .ppl .agent + .agent { border-top: 1px dashed var(--border); }
    .ppl details { margin: 2px 0 6px; }
    .ppl summary { cursor: pointer; font-size: 13px; color: var(--muted); padding: 4px 0; }
    .ppl .caps { display: grid; grid-template-columns: repeat(auto-fill, minmax(200px, 1fr)); gap: 2px 14px; padding: 4px 0 0 2px; }
    .ppl .caps label { display: flex; align-items: center; gap: 8px; min-height: 30px; font-size: 13.5px; }
    .ppl .caps input { min-height: 0; }
    .ppl .pick { display: inline-flex; align-items: center; gap: 6px; font-size: 12.5px; color: var(--muted); }
    .ppl button.danger { color: var(--danger); }
    .ppl .rename { display: flex; gap: 6px; flex-wrap: wrap; }
    .ppl .rename input { width: 180px; max-width: 100%; }
    .ppl a { color: var(--fg); }
    .ppl .wait { color: var(--fg); font-size: 13.5px; }
    @media (max-width: 700px) {
      .ppl .pcard { padding: 12px; }
      .ppl .caps { grid-template-columns: 1fr; }
    }
  }
</style>
