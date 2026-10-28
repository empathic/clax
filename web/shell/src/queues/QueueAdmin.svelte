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
  import { refusal } from "./model";

  const p: { queue: api.Queue } = $props();
  let members = $state<api.QueueMember[] | null>(null);
  let grants = $state<api.ServiceGrant[] | null>(null);
  let memberErr = $state<string | null>(null);
  let grantErr = $state<string | null>(null);
  let pid = $state("");
  let role = $state<api.QueueRole>("triage");
  let grantName = $state("");
  let made = $state<api.ServiceGrantCreated | null>(null);
  let copied = $state<"token" | "key" | null>(null);
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
    if (!confirm(`Remove ${m.display_name ?? m.public_id} from ${p.queue.name}?`)) return;
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
  const copy = (what: "token" | "key", text: string) => { void navigator.clipboard?.writeText(text).then(() => { copied = what; }, () => {}); };
  const day = (iso: string) => iso.slice(0, 10);

  onMount(() => { void loadMembers(); void loadGrants(); });
</script>

<div class="qadmin">
  <section class="part" aria-label={`Members of ${p.queue.name}`}>
    <h4>Members</h4>
    {#if members?.length}
      <ul class="lines">
        {#each members as m (m.public_id)}
          <li class="line" data-member={m.public_id}>
            <span class="who">{m.display_name ?? m.public_id}{#if m.display_name}<span class="key">{m.public_id}</span>{/if}</span>
            <select aria-label={`Role of ${m.display_name ?? m.public_id}`} value={m.role} onchange={e => void changeRole(m, e.currentTarget.value as api.QueueRole)}>
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
</style>
