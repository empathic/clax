<svelte:options css="injected" />

<script lang="ts">
  // A queue's members and service grants, for its admins. A member is a
  // person, at admin (changes the queue, makes service grants) or triage
  // (reads and works its reports). The queue's maker is listed first, as
  // its admin, and can be neither changed nor removed. A person is added
  // by finding them by name (or by their whole public ID) among the owner
  // and the people connected to this Clax (`GET
  // /api/queues/<qid>/people`), choosing one, and a role; adding a member
  // already there changes their role. A search answered after a later one
  // was asked is dropped. A teammate is added by GitHub login too: text
  // that starts with `@`, or a login the search finds no one by, offers
  // "Add @login" (`PUT /api/queues/<qid>/members`); one who has signed in
  // here is a member at once, anyone else is listed as waiting for their
  // GitHub sign-in, with Remove, until that sign-in makes them a member.
  // A new service grant's token and
  // private key are shown here once, each with Copy, under a warning that
  // they are not shown again; Done forgets them. Revoking a grant, after
  // confirming, ends every reporter token it minted. A queue that lists
  // origin patterns (a preview deployment's, such as
  // `https://app-pr-*.fly.dev`) offers a grant limited to them, which
  // mints for its previews alone; the list names a limited grant's
  // origins. Each list is fetched
  // when the panel opens and after each change.
  import { onMount } from "svelte";
  import * as api from "./api";
  import Installs from "./Installs.svelte";
  import { containerPath, deployments, installTag, type OverlayPin, refusal } from "./model";

  const p: { queue: api.Queue } = $props();
  let members = $state<api.QueueMember[] | null>(null);
  let grants = $state<api.ServiceGrant[] | null>(null);
  let memberErr = $state<string | null>(null);
  let grantErr = $state<string | null>(null);
  let find = $state("");
  let found = $state<api.PersonMatch[] | null>(null);
  let chosen = $state<api.PersonMatch | null>(null);
  let asked = 0;
  let role = $state<api.QueueRole>("triage");
  let grantName = $state("");
  let previewsOnly = $state(false);
  let made = $state<api.ServiceGrantCreated | null>(null);
  let copied = $state<string | null>(null);
  // svelte-ignore state_referenced_locally
  const qid = p.queue.id;
  /** The queue's origin patterns: the origins that hold a `*`. */
  const patterns = $derived(p.queue.origins.filter(o => o.includes("*")));

  let pending = $state<api.PendingMember[]>([]);
  // What the last add did, said to a screen reader too.
  let memberNote = $state<string | null>(null);
  const loadMembers = () => api.listMembers(qid).then(b => { members = b.members; pending = b.pending ?? []; }, x => { memberErr = `Could not load the members: ${refusal(x)}`; });
  /** The GitHub login the text offers to add: any login after `@`, or a
   * login the search found no one by. */
  const login = $derived(find.trim().startsWith("@") || found?.length === 0 ? api.githubLogin(find) : null);
  const loadGrants = () => api.listGrants(qid).then(b => { grants = b.grants; }, x => { grantErr = `Could not load the service grants: ${refusal(x)}`; });

  const nameOf = (m: { name?: string | null; owner?: boolean; public_id: string }) => m.name ?? (m.owner ? "The owner" : m.public_id);
  async function search(text: string) {
    find = text; chosen = null;
    const n = ++asked;
    // `@login` names a GitHub account, not someone to find by name.
    if (!text.trim() || text.trim().startsWith("@")) { found = null; return; }
    try {
      const b = await api.searchPeople(qid, text.trim());
      if (n === asked) found = b.people;
    } catch (x) {
      if (n === asked) { found = []; memberErr = `Could not search: ${refusal(x)}`; }
    }
  }
  const pick = (m: api.PersonMatch) => { chosen = m; find = nameOf(m); found = null; };
  async function addMember(e: Event) {
    e.preventDefault();
    if (!chosen && !login) return;
    memberErr = null; memberNote = null;
    try {
      if (chosen) {
        await api.setMember(qid, chosen.public_id, role);
        memberNote = `Added ${nameOf(chosen)}.`;
      } else if (login) {
        const b = await api.addMemberByLogin(qid, login, role);
        memberNote = b.member ? `Added ${nameOf(b.member)} (@${login}).` : `@${login} becomes a member when they first sign in here with GitHub.`;
      }
      chosen = null; find = ""; found = null;
    } catch (x) { memberErr = refusal(x); }
    await loadMembers();
  }
  async function removeWaiting(w: api.PendingMember) {
    if (!confirm(`Remove @${w.github_login} from ${p.queue.name}?`)) return;
    memberErr = null; memberNote = null;
    try { await api.removePending(qid, w.id); } catch (x) { memberErr = refusal(x); }
    await loadMembers();
  }
  async function changeRole(m: api.QueueMember, r: api.QueueRole) {
    memberErr = null;
    try { await api.setMember(qid, m.public_id, r); } catch (x) { memberErr = refusal(x); }
    await loadMembers();
  }
  async function removeMember(m: api.QueueMember) {
    if (!confirm(`Remove ${nameOf(m)} from ${p.queue.name}?`)) return;
    memberErr = null;
    try { await api.removeMember(qid, m.public_id); } catch (x) { memberErr = refusal(x); }
    await loadMembers();
  }
  async function makeGrant(e: Event) {
    e.preventDefault();
    const name = grantName.trim();
    if (!name) return;
    grantErr = null; copied = null;
    try { made = await api.createGrant(qid, name, previewsOnly && patterns.length ? patterns : undefined); grantName = ""; previewsOnly = false; } catch (x) { grantErr = refusal(x); }
    await loadGrants();
  }
  async function revoke(g: api.ServiceGrant) {
    if (!confirm(`Revoke the service grant "${g.name}"? Every reporter token it made stops working.`)) return;
    grantErr = null;
    try { await api.revokeGrant(qid, g.id); } catch (x) { grantErr = refusal(x); }
    if (made?.grant.id === g.id) made = null;
    await loadGrants();
  }
  const copy = (what: string, text: string) => { void navigator.clipboard?.writeText(text).then(() => { copied = what; }, () => {}); };
  const day = (iso: string) => iso.slice(0, 10);

  // The one-tag install: the overlay this instance serves, pinned by version
  // and integrity when the container's answer names it.
  let pin = $state<OverlayPin | null>(null);
  let pinned = $state(true);
  const loadPin = () => fetch(containerPath(qid), { credentials: "omit" }).then(r => (r.ok ? r.json() : null)).then(b => { pin = b?.overlay ?? null; }, () => { pin = null; });
  const tag = $derived(installTag(location.origin, qid, pinned ? pin : null));
  const deploys = $derived(deployments(location.origin, qid, pinned ? pin : null));


  // Who answers the queue (Addendum 9): an ordered list, the first the
  // first responder; each change saves the whole list.
  let responders = $state<api.Responder[] | null>(null);
  let respErr = $state<string | null>(null);
  let agentName = $state("");
  let agentGrant = $state("");
  let agentEscalate = $state("");
  const loadResponders = () => api.listResponders(qid).then(b => { responders = b.responders; }, x => { respErr = `Could not load who answers: ${refusal(x)}`; });
  async function saveResponders(next: api.Responder[]) {
    respErr = null;
    try { responders = (await api.setResponders(qid, next)).responders; } catch (x) { respErr = refusal(x); }
  }
  async function addAgent(e: Event) {
    e.preventDefault();
    const name = agentName.trim();
    if (!name || !responders) return;
    const mins = Number(agentEscalate);
    const r: api.Responder = { kind: "agent", name, ...(agentGrant ? { service_grant_id: agentGrant } : {}), ...(agentEscalate && mins > 0 ? { escalate_after_secs: Math.round(mins * 60) } : {}) };
    await saveResponders([...responders, r]);
    if (!respErr) { agentName = ""; agentGrant = ""; agentEscalate = ""; }
  }
  const moveUp = (i: number) => { if (responders && i > 0) void saveResponders([...responders.slice(0, i - 1), responders[i], responders[i - 1], ...responders.slice(i + 1)]); };
  const dropResponder = (i: number) => { if (responders) void saveResponders(responders.filter((_, j) => j !== i)); };
  const responderName = (r: api.Responder) => (r.kind === "agent" ? r.name : `ClaxBot (${r.profile.tier})`);
  const responderNote = (r: api.Responder) => [
    r.kind === "agent" && r.service_grant_id ? `through ${grants?.find(g => g.id === r.service_grant_id)?.name ?? "a revoked grant"}` : "",
    r.escalate_after_secs ? `next asked after ${Math.round(r.escalate_after_secs / 60)} min` : "",
  ].filter(Boolean).join(" · ");

  onMount(() => { void loadMembers(); void loadGrants(); void loadPin(); void loadResponders(); });
</script>

<div class="qadmin">
  <section class="part" aria-labelledby={`install-${qid}`}>
    <h4 id={`install-${qid}`}>Install</h4>
    <p class="hint">Paste this tag into any page on this queue's origins. No build step: Clax serves the overlay and its settings, and upgrades it when Clax deploys. <code>data-auth</code> is your backend's token route, which mints reporter tokens with a service grant (below).</p>
    <div class="cmd"><code aria-label="Install tag">{tag}</code><button type="button" onclick={() => copy("tag", tag)}>{copied === "tag" ? "Copied" : "Copy"}</button></div>
    {#each deploys.slice(1) as d (d.id)}
      <details class="deploy">
        <summary>{d.label}</summary>
        <p class="hint">{d.where}</p>
        <div class="cmd"><pre><code aria-label={`${d.label} snippet`}>{d.artifact}</code></pre><button type="button" onclick={() => copy(d.id, d.artifact)}>{copied === d.id ? "Copied" : "Copy"}</button></div>
      </details>
    {/each}
    <p class="hint">Or the Clax extension: on any page, its owner adds the site to this queue and the extension brings the overlay; a page that has the tag keeps its own.</p>
    {#if pin}
      <label class="check"><input type="checkbox" checked={pinned} onchange={e => { pinned = e.currentTarget.checked; }} /> Pin this version (with integrity); unpinned follows each Clax deploy within an hour</label>
    {/if}
  </section>

  <section class="part" aria-label={`Members of ${p.queue.name}`}>
    <h4>Members</h4>
    {#if members?.length}
      <ul class="lines">
        {#each members as m (m.public_id)}
          <li class="line" data-member={m.public_id}>
            <span class="who">{nameOf(m)}{#if m.maker}<span class="key">made this queue</span>{:else if m.name}<span class="key">{m.public_id}</span>{/if}</span>
            {#if m.maker}
              <span class="fixed">Admin</span>
            {:else}
              <select aria-label={`Role of ${nameOf(m)}`} value={m.role} onchange={e => void changeRole(m, e.currentTarget.value as api.QueueRole)}>
                <option value="triage">Triage</option><option value="admin">Admin</option>
              </select>
              <button type="button" class="ghost danger" onclick={() => void removeMember(m)}>Remove</button>
            {/if}
          </li>
        {/each}
      </ul>
    {:else if members}
      <p class="none">No members yet.</p>
    {/if}
    {#if pending.length}
      <ul class="lines" aria-label="Waiting for their GitHub sign-in">
        {#each pending as w (w.id)}
          <li class="line" data-pending={w.github_login}>
            <span class="who">@{w.github_login}<span class="key">waiting for their GitHub sign-in</span></span>
            <span class="fixed">{w.role === "admin" ? "Admin" : "Triage"}</span>
            <button type="button" class="ghost danger" aria-label={`Remove @${w.github_login}`} onclick={() => void removeWaiting(w)}>Remove</button>
          </li>
        {/each}
      </ul>
    {/if}
    <form class="add" onsubmit={addMember}>
      <div class="field pick">
        <label for={`qa-find-${qid}`}>Add a person</label>
        <input id={`qa-find-${qid}`} name="person" autocomplete="off" spellcheck="false" placeholder="Name, public ID or @github-login" value={find} oninput={e => void search(e.currentTarget.value)} />
        {#if found?.length}
          <ul class="found" aria-label="People found">
            {#each found as f (f.public_id)}
              {@const there = members?.some(m => m.public_id === f.public_id)}
              <li data-found={f.public_id}>
                <button type="button" class="ghost" disabled={there} onclick={() => pick(f)}>
                  {#if f.avatar_url}<img class="av" src={f.avatar_url} alt="" width="20" height="20" referrerpolicy="no-referrer" />{/if}
                  <span>{nameOf(f)}</span><span class="key">{there ? "already a member" : f.public_id}</span>
                </button>
              </li>
            {/each}
          </ul>
        {:else if found && login}
          <p class="none" role="status">No one connected to this Clax goes by that name. Add @{login} by their GitHub login: they become a member when they first sign in here with GitHub.</p>
        {:else if found}
          <p class="none" role="status">No one connected to this Clax goes by that name. Add a teammate by their GitHub login, as @login, or the owner invites people in Settings › People.</p>
        {:else if login}
          <p class="none" role="status">Add @{login} by their GitHub login: someone who has signed in here is a member at once, anyone else when they first sign in with GitHub.</p>
        {/if}
      </div>
      <label class="field">Role <select value={role} onchange={e => { role = e.currentTarget.value as api.QueueRole; }}><option value="triage">Triage</option><option value="admin">Admin</option></select></label>
      <button type="submit" disabled={!chosen && !login}>{!chosen && login ? `Add @${login}` : "Add member"}</button>
    </form>
    <p class="hint">Triage reads and works the queue's reports; admin also changes the queue and its service grants.</p>
    <p class="hint" role="status">{memberNote ?? ""}</p>
    {#if memberErr}<p class="err" role="alert">{memberErr}</p>{/if}
  </section>

  <section class="part" aria-labelledby={`resp-${qid}`}>
    <h4 id={`resp-${qid}`}>Who answers</h4>
    <p class="hint">Who answers this queue's reports, in order: the first is the first responder; the next is asked when one lets a report wait past its time.</p>
    {#if responders?.length}
      <ol class="lines" aria-label={`Who answers ${p.queue.name}`}>
        {#each responders as r, i (responderName(r))}
          <li class="line" data-responder={i}>
            <span class="who">{i === 0 ? "First: " : ""}{responderName(r)}{#if responderNote(r)}<span class="key">{responderNote(r)}</span>{/if}</span>
            {#if i > 0}<button type="button" class="ghost" aria-label={`Move ${responderName(r)} up`} onclick={() => moveUp(i)}>Move up</button>{/if}
            <button type="button" class="ghost danger" aria-label={`Remove ${responderName(r)}`} onclick={() => dropResponder(i)}>Remove</button>
          </li>
        {/each}
      </ol>
    {:else if responders}
      <p class="none">No one is named to answer yet: members triage it by hand.</p>
    {/if}
    <form class="add" onsubmit={addAgent}>
      <label class="field">Agent (its chitin title) <input name="agent_name" autocomplete="off" maxlength="100" placeholder="work-changeling-queue-agent" value={agentName} oninput={e => { agentName = e.currentTarget.value; }} /></label>
      <label class="field">Through grant
        <select name="agent_grant" value={agentGrant} onchange={e => { agentGrant = e.currentTarget.value; }}>
          <option value="">None</option>
          {#each grants ?? [] as g (g.id)}<option value={g.id}>{g.name}</option>{/each}
        </select>
      </label>
      <label class="field">Escalate after (min) <input name="agent_escalate" type="number" min="1" max="43200" inputmode="numeric" value={agentEscalate} oninput={e => { agentEscalate = e.currentTarget.value; }} /></label>
      <button type="submit" disabled={!agentName.trim() || !responders}>Add agent</button>
    </form>
    <p class="hint">ClaxBot answers a queue once this Clax has a model provider's key; its per-queue profile is set here then.</p>
    {#if respErr}<p class="err" role="alert">{respErr}</p>{/if}
  </section>

  <section class="part" aria-label={`Service grants of ${p.queue.name}`}>
    <h4>Service grants</h4>
    <p class="hint">A service grant lets your backend mint reporter tokens for this queue's pages.</p>
    {#if made}
      <div class="secret" role="region" aria-label="New service grant">
        <p class="warn">Copy the token and private key of "{made.grant.name}" now: they are not shown again.</p>
        <span class="lbl">Token</span>
        <div class="cmd"><code>{made.token}</code><button type="button" onclick={() => copy("token", made!.token)}>{copied === "token" ? "Copied" : "Copy"}</button></div>
        <span class="lbl">Private key (PKCS#8, base64url)</span>
        <div class="cmd"><code>{made.private_key}</code><button type="button" onclick={() => copy("key", made!.private_key)}>{copied === "key" ? "Copied" : "Copy"}</button></div>
        <div class="row"><button type="button" class="primary" onclick={() => { made = null; copied = null; }}>Done</button></div>
      </div>
    {/if}
    {#if grants?.length}
      <ul class="lines">
        {#each grants as g (g.id)}
          <li class="line" data-grant={g.id}>
            <span class="who">{g.name}<span class="key">made {day(g.created_at)}</span>{#if g.origins}<span class="key limit">{g.origins.every(o => o.includes("*")) ? "previews" : "only"}: {g.origins.join(", ")}</span>{/if}</span>
            <button type="button" class="ghost danger" onclick={() => void revoke(g)}>Revoke</button>
          </li>
        {/each}
      </ul>
    {:else if grants}
      <p class="none">No service grants.</p>
    {/if}
    <form class="add" onsubmit={makeGrant}>
      <label class="field">Name <input name="grant_name" autocomplete="off" maxlength="80" placeholder="production backend" value={grantName} oninput={e => { grantName = e.currentTarget.value; }} /></label>
      {#if patterns.length}
        <label class="hint"><input type="checkbox" name="previews_only" checked={previewsOnly} onchange={e => { previewsOnly = e.currentTarget.checked; }} /> Previews only: mint only for {patterns.join(", ")}</label>
      {/if}
      <button type="submit" disabled={!grantName.trim()}>Create service grant</button>
    </form>
    {#if grantErr}<p class="err" role="alert">{grantErr}</p>{/if}
  </section>

  <Installs queue={p.queue} admin={true} />
</div>

<style>
  .qadmin { display: flex; flex-direction: column; gap: 0; min-width: 0; }
  .part { margin-top: 14px; padding-top: 12px; border-top: 1px solid var(--border); min-width: 0; }
  h4 { margin: 0 0 6px; font-size: 12.5px; font-weight: 600; color: var(--muted); text-transform: uppercase; letter-spacing: .04em; }
  .lines { list-style: none; margin: 0 0 8px; padding: 0; }
  .line { display: flex; flex-wrap: wrap; align-items: center; gap: 4px 10px; min-height: 36px; min-width: 0; padding: 2px 0; }
  .line + .line { border-top: 1px dashed var(--border); }
  .who { flex: 1; min-width: 0; font-weight: 600; font-size: 14px; overflow-wrap: anywhere; }
  .key { margin-left: 8px; font: 400 12px var(--mono); color: var(--muted); }
  .add { display: flex; flex-wrap: wrap; align-items: end; gap: 8px 12px; margin-top: 6px; }
  .field { display: flex; flex-direction: column; gap: 4px; font-size: 12.5px; color: var(--muted); min-width: 0; }
  .field input { width: 220px; max-width: 100%; }
  .pick { position: relative; }
  .found { list-style: none; margin: 4px 0 0; padding: 4px; border: 1px solid var(--border); border-radius: var(--radius-sm); background: var(--card); max-width: 320px; }
  .found button { display: flex; align-items: center; gap: 8px; width: 100%; min-height: 32px; justify-content: flex-start; text-align: left; color: var(--fg); }
  .found .av { border-radius: 50%; flex: none; }
  .fixed { color: var(--muted); font-size: 13px; }
  @media (pointer: coarse) { .found button { min-height: 44px; } }
  .hint, .none { margin: 6px 0 0; color: var(--muted); font-size: 12.5px; }
  .err { margin: 6px 0 0; color: var(--danger); font-size: 13px; }
  button.danger { color: var(--danger); }
  select { min-width: 0; max-width: 100%; }
  .secret { display: flex; flex-direction: column; gap: 6px; margin: 8px 0 12px; padding: 12px; border-radius: var(--radius-sm); background: var(--danger-tint); }
  .warn { margin: 0 0 4px; font-size: 13.5px; font-weight: 600; }
  .lbl { font-size: 12.5px; color: var(--muted); }
  .cmd { display: flex; gap: 8px; align-items: stretch; min-width: 0; }
  .cmd code { flex: 1; min-width: 0; padding: 8px 10px; border: 1px solid var(--border); border-radius: var(--radius-sm); background: var(--bg); font: 13px/1.4 var(--mono); overflow-wrap: anywhere; user-select: all; }
  .row { display: flex; gap: 8px; margin-top: 4px; }
  .check { display: flex; align-items: center; gap: 6px; margin-top: 8px; font-size: 12.5px; color: var(--muted); }
  .hint code { font: 12px var(--mono); }
  .deploy { margin-top: 8px; }
  .deploy summary { cursor: pointer; font-size: 13px; min-height: 24px; }
  .cmd pre { flex: 1; min-width: 0; margin: 0; overflow-x: auto; }
  .cmd pre code { display: block; white-space: pre; overflow-wrap: normal; }
</style>
