<svelte:options css="injected" />

<script lang="ts">
  // A queue's members and service grants, for its admins. A member is a
  // person by public ID, at admin (changes the queue, makes service
  // grants) or triage (reads and works its reports); adding a member
  // already there changes their role. A new service grant's token and
  // private key are shown here once, each with Copy, under a warning that
  // they are not shown again; Done forgets them. Revoking a grant, after
  // confirming, ends every reporter token it minted. Each list is fetched
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
  let pid = $state("");
  let role = $state<api.QueueRole>("triage");
  let grantName = $state("");
  let made = $state<api.ServiceGrantCreated | null>(null);
  let copied = $state<string | null>(null);
  // svelte-ignore state_referenced_locally
  const qid = p.queue.id;

  const loadMembers = () => api.listMembers(qid).then(b => { members = b.members; }, x => { memberErr = `Could not load the members: ${refusal(x)}`; });
  const loadGrants = () => api.listGrants(qid).then(b => { grants = b.grants; }, x => { grantErr = `Could not load the service grants: ${refusal(x)}`; });

  async function addMember(e: Event) {
    e.preventDefault();
    const id = pid.trim();
    if (!id) return;
    memberErr = null;
    try { await api.setMember(qid, id, role); pid = ""; } catch (x) { memberErr = refusal(x); }
    await loadMembers();
  }
  async function changeRole(m: api.QueueMember, r: api.QueueRole) {
    memberErr = null;
    try { await api.setMember(qid, m.public_id, r); } catch (x) { memberErr = refusal(x); }
    await loadMembers();
  }
  async function removeMember(m: api.QueueMember) {
    if (!confirm(`Remove ${m.name ?? m.public_id} from ${p.queue.name}?`)) return;
    memberErr = null;
    try { await api.removeMember(qid, m.public_id); } catch (x) { memberErr = refusal(x); }
    await loadMembers();
  }
  async function makeGrant(e: Event) {
    e.preventDefault();
    const name = grantName.trim();
    if (!name) return;
    grantErr = null; copied = null;
    try { made = await api.createGrant(qid, name); grantName = ""; } catch (x) { grantErr = refusal(x); }
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
            <span class="who">{m.name ?? m.public_id}{#if m.name}<span class="key">{m.public_id}</span>{/if}</span>
            <select aria-label={`Role of ${m.name ?? m.public_id}`} value={m.role} onchange={e => void changeRole(m, e.currentTarget.value as api.QueueRole)}>
              <option value="triage">Triage</option><option value="admin">Admin</option>
            </select>
            <button type="button" class="ghost danger" onclick={() => void removeMember(m)}>Remove</button>
          </li>
        {/each}
      </ul>
    {:else if members}
      <p class="none">No members besides you.</p>
    {/if}
    <form class="add" onsubmit={addMember}>
      <label class="field">Public ID <input name="public_id" autocomplete="off" spellcheck="false" value={pid} oninput={e => { pid = e.currentTarget.value; }} /></label>
      <label class="field">Role <select value={role} onchange={e => { role = e.currentTarget.value as api.QueueRole; }}><option value="triage">Triage</option><option value="admin">Admin</option></select></label>
      <button type="submit" disabled={!pid.trim()}>Add member</button>
    </form>
    <p class="hint">Triage reads and works the queue's reports; admin also changes the queue and its service grants.</p>
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
            <span class="who">{g.name}<span class="key">made {day(g.created_at)}</span></span>
            <button type="button" class="ghost danger" onclick={() => void revoke(g)}>Revoke</button>
          </li>
        {/each}
      </ul>
    {:else if grants}
      <p class="none">No service grants.</p>
    {/if}
    <form class="add" onsubmit={makeGrant}>
      <label class="field">Name <input name="grant_name" autocomplete="off" maxlength="80" placeholder="production backend" value={grantName} oninput={e => { grantName = e.currentTarget.value; }} /></label>
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
