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
  // was asked is dropped. A new service grant's token and
  // private key are shown here once, each with Copy, under a warning that
  // they are not shown again; Done forgets them. Revoking a grant, after
  // confirming, ends every reporter token it minted. A queue that lists
  // origin patterns (a preview deployment's, such as
  // `https://app-pr-*.fly.dev`) offers a grant limited to them, which
  // mints for its previews alone; the list names a limited grant's
  // origins. A webhook endpoint is a URL Clax posts the report events it
  // was chosen for to, signed with its secret, which is shown here once
  // as a grant's token is. It is sent nothing but its verification until
  // it answers that rightly (the box with the secret says how); an
  // unverified one offers Verify again, a disabled one Re-enable, which
  // verifies it again. Its deliveries' log opens under it, each report
  // delivery with Redeliver. Each list is fetched when the panel opens and
  // after each change.
  import { onMount } from "svelte";
  import * as api from "./api";
  import { refusal } from "./model";

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
  let copied = $state<"token" | "key" | null>(null);
  // svelte-ignore state_referenced_locally
  const qid = p.queue.id;
  /** The queue's origin patterns: the origins that hold a `*`. */
  const patterns = $derived(p.queue.origins.filter(o => o.includes("*")));

  const loadMembers = () => api.listMembers(qid).then(b => { members = b.members; }, x => { memberErr = `Could not load the members: ${refusal(x)}`; });
  const loadGrants = () => api.listGrants(qid).then(b => { grants = b.grants; }, x => { grantErr = `Could not load the service grants: ${refusal(x)}`; });

  const nameOf = (m: { name?: string | null; owner?: boolean; public_id: string }) => m.name ?? (m.owner ? "The owner" : m.public_id);
  async function search(text: string) {
    find = text; chosen = null;
    const n = ++asked;
    if (!text.trim()) { found = null; return; }
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
    if (!chosen) return;
    memberErr = null;
    try { await api.setMember(qid, chosen.public_id, role); chosen = null; find = ""; found = null; } catch (x) { memberErr = refusal(x); }
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
  // Webhooks.
  const EVENTS: { id: api.WebhookEventType; label: string }[] = [
    { id: "report.created", label: "Filed" },
    { id: "report.claimed", label: "Claimed" },
    { id: "report.released", label: "Released" },
    { id: "report.replied", label: "Replied" },
    { id: "report.fix_proposed", label: "Fix proposed" },
    { id: "report.verified", label: "Verified" },
    { id: "report.reopened", label: "Reopened" },
    { id: "report.resolved", label: "Resolved" },
  ];
  let hooks = $state<api.Webhook[] | null>(null);
  let hookErr = $state<string | null>(null);
  let hookUrl = $state("");
  let hookEvents = $state<api.WebhookEventType[]>(EVENTS.map(e => e.id));
  let madeHook = $state<api.WebhookCreated | null>(null);
  let copiedSecret = $state(false);
  let openLog = $state<string | null>(null);
  let deliveries = $state<api.WebhookDelivery[] | null>(null);
  const loadHooks = () => api.listWebhooks(qid).then(b => { hooks = b.webhooks; }, x => { hookErr = `Could not load the webhooks: ${refusal(x)}`; });
  const loadLog = (wid: string) => api.webhookDeliveries(qid, wid).then(b => { if (openLog === wid) deliveries = b.deliveries; }, x => { hookErr = `Could not load the deliveries: ${refusal(x)}`; });
  const eventsText = (h: api.Webhook) => h.events.length === EVENTS.length ? "every event" : h.events.map(id => EVENTS.find(e => e.id === id)?.label ?? id).join(", ");
  function toggleEvent(id: api.WebhookEventType, on: boolean) {
    hookEvents = on ? EVENTS.map(e => e.id).filter(x => x === id || hookEvents.includes(x)) : hookEvents.filter(x => x !== id);
  }
  async function makeHook(e: Event) {
    e.preventDefault();
    const url = hookUrl.trim();
    if (!url || !hookEvents.length) return;
    hookErr = null; copiedSecret = false;
    try { madeHook = await api.createWebhook(qid, url, hookEvents.length === EVENTS.length ? [] : hookEvents); hookUrl = ""; } catch (x) { hookErr = refusal(x); }
    await loadHooks();
  }
  async function removeHook(h: api.Webhook) {
    if (!confirm(`Remove the webhook to ${h.url}? Its deliveries not yet sent are dropped.`)) return;
    hookErr = null;
    try { await api.deleteWebhook(qid, h.id); } catch (x) { hookErr = refusal(x); }
    if (madeHook?.webhook.id === h.id) madeHook = null;
    if (openLog === h.id) { openLog = null; deliveries = null; }
    await loadHooks();
  }
  function toggleLog(h: api.Webhook) {
    if (openLog === h.id) { openLog = null; deliveries = null; return; }
    openLog = h.id; deliveries = null;
    void loadLog(h.id);
  }
  async function resend(h: api.Webhook, d: api.WebhookDelivery) {
    hookErr = null;
    try { await api.redeliver(qid, h.id, d.id); } catch (x) { hookErr = refusal(x); }
    await loadLog(h.id);
  }
  async function verifyAgain(h: api.Webhook) {
    hookErr = null;
    try { await api.verifyWebhook(qid, h.id); } catch (x) { hookErr = refusal(x); }
    await loadHooks();
    if (openLog === h.id) await loadLog(h.id);
  }
  const when = (iso: string | null) => iso ? iso.slice(0, 16).replace("T", " ") : "";
  const VERIFY = "webhook.verify";
  const deliveryText = (d: api.WebhookDelivery) => d.event === VERIFY
    ? (d.state === "delivered" ? `verified ${when(d.delivered_at)}` : d.state === "failed" ? "not verified" : "sending")
    : d.state === "delivered" ? `delivered ${when(d.delivered_at)}` : d.state === "failed" ? `gave up after ${d.attempts} attempts` : d.attempts ? `retrying ${when(d.next_at)} (${d.attempts} failed)` : "sending";
  const stateText = (h: api.Webhook) => h.state === "verified" ? "verified"
    : h.state === "disabled" ? `disabled ${when(h.disabled_at)} after ${h.given_up} deliveries in a row failed`
    : h.state === "expired" ? "never verified: remove it and add it again"
    : `waiting for its verification answer, until ${when(h.verify_by)}`;

  const copy = (what: "token" | "key", text: string) => { void navigator.clipboard?.writeText(text).then(() => { copied = what; }, () => {}); };
  const day = (iso: string) => iso.slice(0, 10);

  onMount(() => { void loadMembers(); void loadGrants(); void loadHooks(); });
</script>

<div class="qadmin">
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
    <form class="add" onsubmit={addMember}>
      <div class="field pick">
        <label for={`qa-find-${qid}`}>Add a person</label>
        <input id={`qa-find-${qid}`} name="person" autocomplete="off" spellcheck="false" placeholder="Name or public ID" value={find} oninput={e => void search(e.currentTarget.value)} />
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
        {:else if found}
          <p class="none" role="status">No one connected to this Clax goes by that name. Only the owner and connected people can be added; the owner invites people in Settings › People.</p>
        {/if}
      </div>
      <label class="field">Role <select value={role} onchange={e => { role = e.currentTarget.value as api.QueueRole; }}><option value="triage">Triage</option><option value="admin">Admin</option></select></label>
      <button type="submit" disabled={!chosen}>Add member</button>
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

  <section class="part" aria-label={`Webhooks of ${p.queue.name}`}>
    <h4>Webhooks</h4>
    <p class="hint">Clax posts each chosen report event to the URL, signed with its secret, and retries until it answers 2xx, once the URL has answered its verification. A delivery carries the reporter's name and email: name only endpoints you control.</p>
    {#if madeHook}
      <div class="secret" role="region" aria-label="New webhook">
        <p class="warn">Copy the signing secret of {madeHook.webhook.url} now: it is not shown again.</p>
        <span class="lbl">Signing secret</span>
        <div class="cmd"><code>{madeHook.secret}</code><button type="button" onclick={() => { void navigator.clipboard?.writeText(madeHook!.secret).then(() => { copiedSecret = true; }, () => {}); }}>{copiedSecret ? "Copied" : "Copy"}</button></div>
        <p class="hint answer">Clax has sent it a signed <code>webhook.verify</code> event and sends it no report event until it answers 2xx with <code>{'{"answer": "<hex HMAC-SHA256 of webhook.verify.<challenge> under the secret>"}'}</code>. The SDK's <code>verifyWebhook</code> sets that reply on the event: answer <code>res.json(event.reply)</code>. Deploy the secret, then Verify again.</p>
        <div class="row"><button type="button" class="primary" onclick={() => { madeHook = null; copiedSecret = false; }}>Done</button></div>
      </div>
    {/if}
    {#if hooks?.length}
      <ul class="lines">
        {#each hooks as h (h.id)}
          <li class="line hook" data-webhook={h.id}>
            <span class="who url">{h.url}<span class="key">{eventsText(h)}</span></span>
            <span class="state" data-state={h.state} title={h.state !== "verified" ? (h.last_error ?? "") : ""}>{stateText(h)}</span>
            {#if h.state === "verified" && h.failing_since}<span class="failing" title={h.last_error ?? ""}>failing since {when(h.failing_since)}</span>{/if}
            {#if h.dropped}<span class="failing dropped">{h.dropped} {h.dropped === 1 ? "event" : "events"} dropped over the hourly cap, latest {when(h.last_dropped_at)}</span>{/if}
            {#if h.state === "unverified"}<button type="button" class="ghost" onclick={() => void verifyAgain(h)}>Verify again</button>{/if}
            {#if h.state === "disabled"}<button type="button" class="ghost" onclick={() => void verifyAgain(h)}>Re-enable</button>{/if}
            <button type="button" class="ghost" aria-expanded={openLog === h.id} onclick={() => toggleLog(h)}>Deliveries</button>
            <button type="button" class="ghost danger" onclick={() => void removeHook(h)}>Remove</button>
            {#if openLog === h.id}
              <div class="log">
                {#if deliveries?.length}
                  <ul class="deliveries">
                    {#each deliveries as d (d.id)}
                      <li class="delivery" data-delivery={d.id} data-state={d.state}>
                        <span class="ev">{d.event}</span>
                        <span class="st">{deliveryText(d)}{d.last_status ? ` · ${d.last_status}` : ""}</span>
                        {#if d.last_error && d.state !== "delivered"}<span class="why">{d.last_error}</span>{/if}
                        {#if d.event !== VERIFY && h.state === "verified"}<button type="button" class="ghost" onclick={() => void resend(h, d)}>Redeliver</button>{/if}
                      </li>
                    {/each}
                  </ul>
                {:else if deliveries}
                  <p class="none">No deliveries yet.</p>
                {/if}
              </div>
            {/if}
          </li>
        {/each}
      </ul>
    {:else if hooks}
      <p class="none">No webhooks.</p>
    {/if}
    <form class="add hookform" onsubmit={makeHook}>
      <label class="field">URL <input name="webhook_url" type="url" autocomplete="off" spellcheck="false" placeholder="https://app.example.com/clax/webhook" value={hookUrl} oninput={e => { hookUrl = e.currentTarget.value; }} /></label>
      <fieldset class="events"><legend>Events</legend>
        {#each EVENTS as ev (ev.id)}
          <label><input type="checkbox" name="event" value={ev.id} checked={hookEvents.includes(ev.id)} onchange={e => toggleEvent(ev.id, e.currentTarget.checked)} /> {ev.label}</label>
        {/each}
      </fieldset>
      <button type="submit" disabled={!hookUrl.trim() || !hookEvents.length}>Add webhook</button>
    </form>
    {#if hookErr}<p class="err" role="alert">{hookErr}</p>{/if}
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
  .url { font: 600 13px var(--mono); }
  .url .key { display: block; margin-left: 0; font-family: inherit; font-weight: 400; }
  .failing { font-size: 12.5px; color: var(--danger); }
  .state { font-size: 12.5px; color: var(--muted); }
  .state[data-state="disabled"], .state[data-state="expired"] { color: var(--danger); }
  .answer code { font: 12px var(--mono); overflow-wrap: anywhere; }
  .log { flex-basis: 100%; min-width: 0; padding: 4px 0 6px 12px; border-left: 2px solid var(--border); }
  .deliveries { list-style: none; margin: 0; padding: 0; }
  .delivery { display: flex; flex-wrap: wrap; align-items: center; gap: 2px 10px; padding: 3px 0; font-size: 13px; min-width: 0; }
  .delivery .ev { font: 12.5px var(--mono); }
  .delivery .st { color: var(--muted); }
  .delivery[data-state="failed"] .st { color: var(--danger); }
  .delivery .why { flex-basis: 100%; color: var(--muted); font-size: 12.5px; overflow-wrap: anywhere; }
  .events { display: flex; flex-wrap: wrap; gap: 4px 12px; margin: 0; padding: 0; border: 0; font-size: 13px; min-width: 0; }
  .events legend { padding: 0; margin-bottom: 4px; font-size: 12.5px; color: var(--muted); }
  .events label { display: inline-flex; align-items: center; gap: 4px; }
</style>
