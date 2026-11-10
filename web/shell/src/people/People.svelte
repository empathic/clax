<svelte:options css="injected" />

<script lang="ts">
  import { peopleDenied, refusal } from "./refusal";
  // Settings' screens about people and their agents (spec
  // 2026-10-08-multi-agent-multi-person §3.1, §3.4, §9.5, §9.6, §17, §18),
  // one at a time by `part`:
  // - `people` (the owner's): invites (with the command, its expiry and the
  //   LAN warning; the `*unconnected` switch beside the first), connection
  //   requests waiting (approved in the inbox), the switch, every connected
  //   person (rights, machines, agents, the approvals of theirs the owner
  //   made and what waits on the owner), the people on other Clax the owner
  //   approved or is asked about (authors of comments mirrored from a
  //   public copy, keyed `remote:<origin>#<ID>`, named "<name> (on
  //   <remote>)" by the daemon), and revoked people as history.
  //   `/settings/people?invite` opens Invite at once.
  // - `agents` (the owner's and a connected person's): themselves alone,
  //   their machines and agents, and for a person the approvals they made
  //   and what waits on them; on a hosted daemon, led by their agent tokens.
  // - `account` and `sharing` (a hosted daemon's): this browser's sign-in
  //   and devices, and the pages shared by link.
  // Anyone else is told the screen is not theirs. Every change is the
  // browser's (M20) and refetches the screen; it also refetches on the
  // stream's `people` (the owner's) and `agents` topics, so a person who
  // connects appears, and on its refusal. When it stops being theirs (a
  // person revoked), the screen clears and says so, and the header's
  // links are told to check again. The hosted parts (`HostedPanel`) are a
  // chunk of their own fetched beside `GET /api/hosted/me`, so a local
  // daemon's screen carries none of it and a hosted one waits on no extra
  // round trip; there invites are links, and the viewers who are not
  // connected are guests.
  import { onDestroy, onMount, tick, type Component } from "svelte";
  import { listArtifacts } from "../api";
  import type { PeopleApproval, PeopleGrant, PeopleInvite, PeopleList, PeopleMe, PeopleRequest } from "../api";
  import type { EventStream } from "../stream";
  import * as api from "./api";
  import InviteDialog from "./InviteDialog.svelte";
  import { elevationsOf, expiresIn, liveInvites, waitText } from "./model";
  import PersonCard from "./PersonCard.svelte";
  import UnconnectedSwitch from "./UnconnectedSwitch.svelte";
  import * as hostedApi from "../hosted/api";

  type Part = "people" | "agents" | "account" | "sharing";
  let { stream, now, part = "people" }: { stream?: EventStream; now?: () => Date; part?: Part } = $props();
  const TITLE: Record<Part, string> = { people: "People", agents: "Agents & machines", account: "Account", sharing: "Shared links" };
  /** The hosted panel's parts each screen shows. */
  const HOSTED: Record<Part, ("me" | "devices" | "tokens" | "links")[]> = { people: [], agents: ["tokens"], account: ["me", "devices"], sharing: ["links"] };
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
  // The live artifacts, for raising an agent at one: loaded when a raise
  // panel opens, or when an agent shown is raised at one.
  let artifacts = $state<{ id: string; title: string }[] | null>(null);
  let artifactsWanted = false;
  let inviting = $state(false);
  let hosted = $state<hostedApi.HostedMe | null>(null);
  let HostedPanel = $state<Component<{ me: hostedApi.HostedMe; parts: ("me" | "devices" | "tokens" | "links")[] }> | null>(null);
  // The open dialog is the first invite's: the switch shows in it, not apart.
  let firstInvite = $state(false);
  let gone = false;
  let off: (() => void) | null = null;
  const msg = refusal;

  async function load() {
    let m: PeopleMe;
    try { m = await api.me(); } catch (e) {
      if (gone) return;
      const had = !!me;
      denied = peopleDenied(e, had && !me?.person.owner); me = null; list = null; invites = []; pending = []; approvals = []; requests = []; banner = null; inviting = false;
      if (had) dispatchEvent(new Event("clax:people-gone"));
      return;
    }
    denied = null;
    const owner = m.person.owner;
    try {
      const [l, inv, pend, aps, reqs] = await Promise.all([
        owner ? api.list() : null, owner ? api.invites() : null, owner ? api.pending() : null, api.approvals(), api.requests(),
      ]);
      if (gone) return;
      me = m; list = l; invites = inv?.invites ?? []; pending = pend?.grants ?? [];
      // `?invite` (the inbox's connection card links here): Invite opens at once.
      if (owner && part === "people" && new URLSearchParams(location.search).has("invite")) {
        history.replaceState(history.state, "", location.pathname);
        firstInvite = invites.length === 0; inviting = true;
      }
      approvals = aps.approvals; requests = reqs.requests.filter(r => r.status === "open"); banner = reqs.banner;
      err = null;
    } catch (e) { if (!gone) { me = m; err = msg(e); } }
    // Raises name artifacts by ID: the titles load when any agent shown has one.
    const shown = [...m.agents, ...(list?.people.flatMap(p => p.agents) ?? [])];
    if (artifactsWanted || shown.some(a => elevationsOf(a).some(e => e.access.some(x => x.locations?.some(l => l !== "*"))))) void needArtifacts();
    if (!off && stream && !gone) off = stream.watch(owner ? ["people", "agents"] : ["agents"], e => { if (e.type !== "stream_down" && e.type !== "stream_up") reload(); });
  }
  async function needArtifacts() {
    artifactsWanted = true;
    try { const l = await listArtifacts(); if (!gone) artifacts = l.map(a => ({ id: a.id, title: a.title })); } catch { /* raises name artifact IDs meanwhile */ }
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
  /** Whether an author is someone on another Clax: a key no person here has. */
  const remote = (a: { public_id: string | null }) => !!a.public_id?.startsWith("remote:");
  const remoteApprovals = $derived(approvals.filter(a => remote(a.author)));
  const remoteWaiting = $derived(requests.filter(r => remote(r.author)));
  const SCOPE = { everywhere: "everywhere", artifact: "on one page", thread: "in one thread" } as const;
  /** An approval's scope, naming its page when the daemon names it. */
  const scopeOf = (ap: PeopleApproval) => !ap.artifact_title || ap.scope === "everywhere" ? SCOPE[ap.scope]
    : ap.scope === "artifact" ? `on “${ap.artifact_title}”` : `in one thread on “${ap.artifact_title}”`;
  /** Runs one change, after asking `ask` when given, then refetches. The
   * row of event `e`'s control goes with it: the keyboard's focus moves to
   * its section's heading, or the page's when it was the section's last. */
  async function act(e: Event, fn: () => Promise<unknown>, ask?: string) {
    const sec = (e.currentTarget as Element).closest("section");
    const to = sec && sec.querySelectorAll(".line").length > 1 ? sec.querySelector("h3") : (e.currentTarget as Element).closest(".ppl")?.querySelector("h2");
    if (ask && !confirm(ask)) return;
    try { await fn(); reload(); to?.focus(); } catch (x) { err = msg(x); }
  }
  async function closeInvite() {
    inviting = false;
    await tick();
    document.querySelector<HTMLElement>(".ppl .invite-open")?.focus();
  }
  /** Whose content an approval or request is about; the owner marked as
   * such (the daemon names an unnamed owner "The owner"). */
  const authorOf = (a: { name: string; owner?: boolean }) => (a.owner && a.name !== "The owner" ? `${a.name} (the owner)` : a.name);
  onMount(() => {
    document.title = `${TITLE[part]} · Settings · Clax`;
    reload();
    const panel = HOSTED[part].length ? import("../hosted/HostedPanel.svelte").then(m => m.default, () => null) : null;
    void hostedApi.me().then(h => {
      if (gone) return;
      hosted = h;
      if (h?.person) void panel?.then(c => { if (!gone && c) HostedPanel = c; });
    }, () => {});
  });
  onDestroy(() => { gone = true; off?.(); });
</script>

<div class="ppl">
  <div class="phead">
    <h2 tabindex="-1">{TITLE[part]}</h2>
    {#if owner && part === "people"}<span class="pcount">{people.length === 1 ? "1 person" : `${people.length} people`}</span>{/if}
    <span class="sp"></span>
    {#if owner && part === "people" && !inviting}<button type="button" class="primary invite-open" onclick={() => { firstInvite = invites.length === 0; inviting = true; }}>Invite a person</button>{/if}
  </div>
  {#if denied}<p class="note">{denied}</p>{/if}
  {#if err}<p class="err" role="alert">{err}</p>{/if}
  {#if banner && part !== "account" && part !== "sharing"}<p class="banner">{banner}</p>{/if}
  {#if hosted?.person && me && HostedPanel}<HostedPanel me={hosted} parts={HOSTED[part]} />{/if}

  {#if part === "people" && me && !owner}<p class="note">The people connected to this Clax are the owner's to see. Your own agents and machines are under <a href="/settings/agents">Agents &amp; machines</a>.</p>{/if}
  {#if part === "people" && owner && list}
    {#if inviting}
      <InviteDialog first={firstInvite} unconnected={list.unconnected} now={clock()} onMade={reload} onSwitched={reload} onClose={closeInvite} hosted={!!hosted} />
    {/if}
    {#if live.length}
      <section class="pcard">
        <h3 id="ppl-invites" tabindex="-1">Invites</h3>
        {#each live as i (i.id)}
          <div class="line"><span>{i.owner ? "You, on another machine" : i.person.display_name}</span><span class="meta">expires in {expiresIn(i.expires_at, clock())}</span><span class="sp"></span>
            <button type="button" class="ghost" aria-label={i.owner ? "Revoke the invite for your other machine" : `Revoke ${i.person.display_name}'s invite`} onclick={e => act(e, () => api.revokeInvite(i.id))}>Revoke</button></div>
        {/each}
      </section>
    {/if}
    {#if pending.length}
      <section class="pcard requests">
        <h3 id="ppl-requests" tabindex="-1">Connection requests</h3>
        {#each pending as g (g.id)}
          <div class="line"><span class="mname">{g.client.name}</span><span class="key">key {g.client.key ?? "unknown"}</span><span class="ip">{g.client.peer_ip}</span><span class="sp"></span>
            <a href="/inbox?kind=connection">Approve in the inbox</a>
            <button type="button" class="ghost" aria-label="Deny {g.client.name}" onclick={e => act(e, () => api.denyGrant(g.id))}>Deny</button></div>
        {/each}
      </section>
    {/if}
    {#if !inviting || !firstInvite}
      <section class="pcard">
        <h3 id="ppl-unconnected">{hosted ? "Guests" : "People on your LAN who are not connected"}</h3>
        <UnconnectedSwitch sw={list.unconnected} onChanged={reload} hosted={!!hosted} />
      </section>
    {/if}
  {/if}

  {#if part === "agents" && self}
    <PersonCard view="self" owner={owner} who={self} {artifacts} onChanged={reload} />
  {/if}

  {#if part === "people" && owner}
    {#each people as p (p.public_id)}
      <PersonCard view="owner" who={p} approvals={approvals.filter(a => a.author.public_id === p.public_id)} waiting={requests.filter(r => r.author.public_id === p.public_id)} ofYours={p.approvals_of_yours ?? []} {artifacts} {needArtifacts} onChanged={reload} />
    {:else}
      {#if list}<p class="note">No one is connected yet. Invite a person to let their agents work with yours.</p>{/if}
    {/each}
    {#if remoteApprovals.length || remoteWaiting.length}
      <section class="pcard approvals remote-authors">
        <h3 id="ppl-remote" tabindex="-1">People on other Clax</h3>
        <p class="note">They commented on the public copies of your apps. Your agents read their comments once you approve them.</p>
        {#each remoteApprovals as ap (ap.id)}
          <div class="line"><span>{ap.author.name}</span><span class="meta">{scopeOf(ap)}</span><span class="sp"></span>
            <button type="button" class="ghost" aria-label="Revoke the approval of {ap.author.name}, {scopeOf(ap)}" onclick={e => act(e, () => api.revokeApproval(ap.id))}>Revoke</button></div>
        {/each}
        {#if remoteWaiting.length}
          <h4>Waiting</h4>
          {#each remoteWaiting as r (r.id)}<a class="line wait" href="/inbox?approval={encodeURIComponent(r.id)}"><span>{waitText(r, r.author.name)}</span></a>{/each}
        {/if}
      </section>
    {/if}
    {#if revoked.length}
      <section class="pcard revoked">
        <h3 id="ppl-revoked">Revoked</h3>
        {#each revoked as p (p.public_id)}<PersonCard view="owner" who={p} hosted={!!hosted} onChanged={reload} />{/each}
      </section>
    {/if}
  {:else if part === "agents" && me && !owner}
    <section class="pcard approvals">
      <h3 id="ppl-reach" tabindex="-1">Who reaches your agents</h3>
      {#each approvals as ap (ap.id)}
        <div class="line"><span>{authorOf(ap.author)}</span><span class="meta">{scopeOf(ap)}</span><span class="sp"></span>
          <button type="button" class="ghost" aria-label="Revoke the approval of {authorOf(ap.author)}, {scopeOf(ap)}" onclick={e => act(e, () => api.revokeApproval(ap.id))}>Revoke</button></div>
      {:else}
        <p class="none">No one yet: your agents ask you first.</p>
      {/each}
      {#if requests.length}
        <h4>Waiting</h4>
        {#each requests as r (r.id)}<a class="line wait" href="/inbox?approval={encodeURIComponent(r.id)}"><span>{waitText(r, authorOf(r.author))}</span></a>{/each}
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
    .ppl .note, .ppl .none { margin: 4px 0; color: var(--muted); font-size: 13px; overflow-wrap: anywhere; }
    .ppl .banner { margin: 0; padding: 10px 12px; border-radius: var(--radius-sm); background: var(--danger-tint); font-size: 13.5px; }
    .ppl .pcard { background: var(--card); border: 1px solid var(--border); border-radius: var(--radius); padding: 14px 16px; min-width: 0; }
    .ppl .pcard > header { display: flex; flex-wrap: wrap; align-items: center; gap: 8px 12px; }
    .ppl h3 { margin: 0 0 6px; font-size: 16px; overflow-wrap: anywhere; }
    .ppl .pcard > header h3 { margin: 0; }
    .ppl .you { color: var(--muted); font-weight: 400; }
    .ppl .ghlogin { color: var(--muted); font: 12.5px var(--mono); text-decoration: none; overflow-wrap: anywhere; }
    .ppl .ghlogin:hover { text-decoration: underline; }
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
    /* A name in a button or a long title in a select wraps or narrows; it never widens the page. */
    .ppl button { max-width: 100%; white-space: normal; overflow-wrap: anywhere; text-align: start; }
    .ppl select { min-width: 0; max-width: 100%; }
    .ppl [tabindex="-1"]:focus { outline: none; }
    .ppl .acts { display: inline-flex; gap: 4px; margin-left: auto; }
    .ppl .where { display: flex; flex-wrap: wrap; margin: 4px 0 2px; }
    .ppl .where select { flex: 0 1 auto; }
    .ppl .raises { margin: 4px 0; padding-left: 18px; font-size: 13px; overflow-wrap: anywhere; }
    .ppl .raised { display: block; color: var(--muted); font-size: 12px; }
    @media (max-width: 700px) {
      .ppl .pcard { padding: 12px; }
      .ppl .caps { grid-template-columns: 1fr; }
    }
    /* At phone width an agent's harness and machine take a row of their own, below its name and Rename and End. */
    @media (max-width: 520px) { .ppl .agent .meta { order: 3; flex-basis: 100%; } }
  }
</style>
