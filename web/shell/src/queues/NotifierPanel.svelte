<svelte:options css="injected" />

<script lang="ts">
  // A queue's Zulip notifier, for its admins: where people are told that a fix
  // has something to check or waits for one of them. The key is write-only:
  // only its last four come back, and a save that leaves it empty keeps it.
  // Every change names the `rev` this panel read; a notifier another admin
  // changed is read again, and the panel says so. A failure is shown in the
  // daemon's fixed words, the stream's and direct messages' apart. A test uses
  // the saved settings and reaches the stream alone, and leaves what was typed
  // here as it is.
  import { onMount } from "svelte";
  import type { Queue } from "./api";
  import * as api from "./notifier-api";
  import { refusal } from "../people/refusal";

  const p: { queue: Queue } = $props();
  // svelte-ignore state_referenced_locally
  const qid = p.queue.id;
  let n = $state<api.ZulipNotifier | null>(null);
  let loading = $state(true);
  let err = $state<string | null>(null);
  let busy = $state(false);
  let tested = $state(false);
  let f = $state({ site: "", botEmail: "", apiKey: "", stream: "", people: "" });

  const count = (k: number, what: string) => `${k} ${what}${k === 1 ? "" : "s"}`;
  const status = $derived.by(() => {
    if (!n) return "Not connected.";
    if (n.last_failure) return `The last try failed. ${n.last_failure}`;
    return n.last_ok_at ? "Connected." : "Details saved; nothing sent yet.";
  });

  function fill(v: api.ZulipNotifier | null) {
    n = v;
    if (!v) { f = { site: "", botEmail: "", apiKey: "", stream: "", people: "" }; return; }
    f = { site: v.site, botEmail: v.bot_email, apiKey: "", stream: v.stream, people: Object.entries(v.people).map(([k, e]) => `${k} ${e}`).join("\n") };
  }
  /** The people box as a map, or the first line it cannot read or has twice. */
  function people(): Record<string, string> | string {
    const out: Record<string, string> = {};
    const seen = new Set<string>();
    for (const line of f.people.split("\n").map(l => l.trim()).filter(Boolean)) {
      const parts = line.split(/\s+/);
      if (parts.length !== 2 || seen.has(parts[0].toLowerCase())) return line;
      seen.add(parts[0].toLowerCase());
      out[parts[0]] = parts[1];
    }
    return out;
  }
  async function load() {
    try { fill((await api.getNotifier(qid)).notifier); err = null; }
    catch (e) { err = refusal(e); }
    loading = false;
  }
  /** Runs `act`; a notifier that moved on is read again, and the refusal stays. */
  async function guarded(act: () => Promise<void>) {
    if (busy) return;
    busy = true; err = null; tested = false;
    try { await act(); }
    catch (e) {
      const moved = (e as { code?: string }).code === "notifier_changed";
      if (moved) await load();
      err = refusal(e);
    }
    busy = false;
  }
  function save(e: SubmitEvent) {
    e.preventDefault();
    const map = people();
    if (typeof map === "string") { err = `Each line is a GitHub login, once, and a Zulip email: ${map}`; return; }
    void guarded(async () => {
      const key = f.apiKey.trim();
      fill((await api.putNotifier(qid, { site: f.site.trim(), bot_email: f.botEmail.trim(), ...(key ? { api_key: key } : {}), stream: f.stream.trim(), people: map, if_rev: n?.rev ?? null })).notifier);
    });
  }
  const test = () => guarded(async () => {
    if (!n) return;
    n = (await api.testNotifier(qid, n.rev)).notifier;
    tested = true;
  });
  const disconnect = () => guarded(async () => {
    if (!n || !confirm("Disconnect Zulip? Its key and the notices not yet sent are removed.")) return;
    await api.deleteNotifier(qid, n.rev);
    fill(null);
  });
  onMount(() => { void load(); });
</script>

<section class="part" aria-label={`Notifications of ${p.queue.name}`}>
  <h4>Zulip</h4>
  {#if !loading}
    <p class="stat">{status}</p>
    {#if n?.direct_failure}<p class="err" data-direct-failure>A direct message failed. {n.direct_failure}</p>{/if}
    {#if n && n.pending > 0}<p class="hint" data-pending>{count(n.pending, "notice")} waiting to be sent.</p>{/if}
    {#if n && n.expired > 0}<p class="hint" data-expired>{count(n.expired, "notice")} could not be sent within a week.</p>{/if}
    <p class="hint">A reporter named below gets a direct message with the link when a fix has something to check. For anyone else the stream gets a link to the report, which only the queue's people can open, and no name, preview link or report text. A fix held for a person is told on the stream too, so choose one the queue's people read.</p>
    <form class="nform" onsubmit={save}>
      <div class="grid">
        <label class="field">Site <input name="site" autocomplete="off" spellcheck="false" required placeholder="https://acme.zulipchat.com" value={f.site} oninput={e => { f.site = e.currentTarget.value; }} /></label>
        <label class="field">Bot's email <input name="bot_email" autocomplete="off" spellcheck="false" required value={f.botEmail} oninput={e => { f.botEmail = e.currentTarget.value; }} /></label>
        <label class="field">Bot's API key <input name="api_key" type="password" autocomplete="new-password" spellcheck="false" placeholder={n?.api_key.last4 ? `Set, ending ${n.api_key.last4}` : ""} value={f.apiKey} oninput={e => { f.apiKey = e.currentTarget.value; }} /></label>
        <label class="field">Stream <input name="stream" autocomplete="off" spellcheck="false" required value={f.stream} oninput={e => { f.stream = e.currentTarget.value; }} /></label>
      </div>
      {#if n?.api_key.last4}<p class="hint" data-key>The key ends {n.api_key.last4}. Leave the box empty to keep it; a new site needs a new key.</p>{/if}
      {#if n?.api_key.problem}<p class="err" role="alert">{n.api_key.problem}</p>{/if}
      <label class="field">People, one per line: GitHub login, then Zulip email
        <textarea name="people" rows="3" spellcheck="false" placeholder="octocat octocat@acme.example.com" value={f.people} oninput={e => { f.people = e.currentTarget.value; }}></textarea>
      </label>
      {#if err}<p class="err" role="alert">{err}</p>{/if}
      {#if tested}<p class="hint" data-tested>Sent with the saved settings. Look for it on the stream, under "Clax test".</p>{/if}
      {#if n}<p class="hint" data-test-says>A test message uses the saved settings. It posts to the stream, and does not try a direct message.</p>{/if}
      <div class="row">
        <button class="primary" type="submit" aria-disabled={busy}>{busy ? "Saving…" : n ? "Save" : "Connect"}</button>
        {#if n}<button class="ghost" type="button" onclick={() => void test()}>Send a test message</button>{/if}
        {#if n}<button class="ghost danger" type="button" onclick={() => void disconnect()}>Disconnect</button>{/if}
      </div>
    </form>
  {:else if err}
    <p class="err" role="alert">{err}</p>
  {/if}
</section>

<style>
  .part { margin-top: 14px; padding-top: 12px; border-top: 1px solid var(--border); min-width: 0; }
  h4 { margin: 0 0 6px; font-size: 12.5px; font-weight: 600; color: var(--muted); text-transform: uppercase; letter-spacing: .04em; }
  .stat { margin: 0 0 4px; font-size: 14px; overflow-wrap: anywhere; }
  .nform { display: flex; flex-direction: column; gap: 10px; margin-top: 10px; min-width: 0; }
  .grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(220px, 1fr)); gap: 8px 12px; min-width: 0; }
  .field { display: flex; flex-direction: column; gap: 4px; font-size: 12.5px; color: var(--muted); min-width: 0; }
  .field input { width: 100%; min-width: 0; box-sizing: border-box; }
  .field textarea { width: 100%; max-width: 480px; box-sizing: border-box; font: 13px/1.5 var(--mono); color: var(--fg); background: var(--bg); border: 1px solid var(--border); border-radius: var(--radius-sm); padding: 6px 8px; }
  .hint { margin: 6px 0 0; color: var(--muted); font-size: 12.5px; overflow-wrap: anywhere; }
  .err { margin: 6px 0 0; color: var(--danger); font-size: 13px; overflow-wrap: anywhere; }
  .row { display: flex; flex-wrap: wrap; gap: 8px; }
  button.danger { color: var(--danger); }
</style>
