<svelte:options css="injected" />

<script lang="ts">
  // A connection request in the owner's inbox (spec
  // 2026-10-08-multi-agent-multi-person §3.2, §6.3): the machine's name,
  // its address and its key's fingerprint, with **Approve…** and **Deny**.
  // Approve asks for the code the requesting machine shows (a mismatch
  // approves nothing; the third denies the request), a name for the new
  // person and the preset (Collaborator by default), and is enabled only
  // with a code and a name typed. A request only makes a new person: the
  // card says that a machine of someone already connected, or of the
  // owner, takes an invite, and links to Invite on People. A decided
  // request says how it went. Every change is the owner's browser's (M20);
  // the daemon's refusals show as they are. Names are text.
  import { tick } from "svelte";
  import type { InboxItem, PeopleGrant } from "../api";
  import { relativeTime } from "../format";
  import * as api from "./api";
  import { PRESET_NAMES } from "./model";

  let { item, now }: { item: InboxItem; now?: Date } = $props();
  // The grant as a decision made here left it, until the item itself says so.
  let made = $state<Pick<PeopleGrant, "status" | "person"> | null>(null);
  let open = $state(false);
  let code = $state("");
  let name = $state("");
  let preset = $state("clax:collaborator");
  let busy = $state(false);
  let err = $state<string | null>(null);

  const g = $derived(item.connection ?? null);
  const status = $derived(g && g.status !== "pending" ? g.status : made?.status ?? g?.status ?? "gone");
  const person = $derived((g && g.status !== "pending" ? g.person : made?.person ?? g?.person) ?? null);
  const ready = $derived(!!code.trim() && !!name.trim());
  const outcome = $derived(status === "approved" ? `Approved: ${person?.display_name ?? "the new person"} is connected` : status === "denied" ? "Denied" : status === "expired" ? "Expired: the machine stopped waiting" : "This request no longer waits for you");

  const card = () => [...document.querySelectorAll<HTMLElement>(".ccard")].find(e => e.dataset.grant === g?.id);
  async function begin() {
    open = true;
    await tick();
    // By query, not `bind:this`: its helper would be split into a chunk the artifact view's entry then loads.
    card()?.querySelector<HTMLInputElement>('input[name="code"]')?.focus();
  }
  async function run(fn: () => Promise<{ grant: PeopleGrant }>) {
    if (busy) return;
    busy = true; err = null;
    try {
      const r = await fn();
      made = { status: r.grant.status, person: r.grant.person };
      open = false;
      queueMicrotask(() => card()?.focus({ preventScroll: true }));
    } catch (x) { err = x instanceof Error ? x.message : String(x); } finally { busy = false; }
  }
  const approve = (e: Event) => { e.preventDefault(); if (g && ready) void run(() => api.approveGrant(g.id, { user_code: code.trim(), name: name.trim(), rights: preset })); };
  const deny = () => { if (g) void run(() => api.denyGrant(g.id)); };
</script>

<article class="ccard" data-grant={g?.id ?? ""} data-status={status} tabindex="-1" aria-label="Connection request">
  {#if g}
    <header class="chead">
      <span>Connection request from {g.client.name} ({g.client.peer_ip})</span>
      <time datetime={item.created_at}>{relativeTime(item.created_at, now ?? new Date())}</time>
    </header>
    <p class="meta key">key {g.client.key ?? "unknown"}</p>
  {:else}
    <header class="chead"><span>A connection request</span></header>
  {/if}
  {#if status === "pending"}
    {#if open}
      <form onsubmit={approve}>
        <p class="meta">Approving makes a new person. To add this machine to someone already connected, or to you, make an invite instead: <a href="/people?invite">Invite</a></p>
        <div class="row">
          <label class="field">Code shown on {g!.client.name}
            <input name="code" autocomplete="off" spellcheck="false" autocapitalize="characters" placeholder="WXYZ-4821" value={code} oninput={e => { code = e.currentTarget.value; }} /></label>
          <label class="field">Name for the new person
            <input name="name" autocomplete="off" maxlength="60" value={name} oninput={e => { name = e.currentTarget.value; }} /></label>
          <label class="field">Rights
            <select onchange={e => { preset = e.currentTarget.value; }}>
              {#each Object.keys(PRESET_NAMES) as v (v)}<option value={v} selected={v === preset}>{PRESET_NAMES[v]}</option>{/each}
            </select></label>
        </div>
        <div class="acts">
          <button type="submit" class="primary" disabled={!ready || busy}>Approve</button>
          <button type="button" class="ghost" disabled={busy} onclick={() => { open = false; err = null; }}>Cancel</button>
        </div>
      </form>
    {:else}
      <div class="acts">
        <button type="button" class="primary" onclick={begin}>Approve…</button>
        <button type="button" class="ghost" disabled={busy} onclick={deny}>Deny</button>
      </div>
    {/if}
  {:else}
    <p class="outcome" role="status">{outcome}</p>
  {/if}
  {#if err}<p class="err" role="alert">{err}</p>{/if}
</article>

<style>
  :global {
    .ccard { position: relative; display: grid; gap: 6px; background: var(--card); border: 1px solid var(--border); border-left: 3px solid var(--agent); border-radius: var(--radius); padding: 11px 12px 10px 13px; min-width: 0; font-size: 13.5px; }
    .ccard:focus { outline: none; }
    .ccard:focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; }
    .ccard:not([data-status="pending"]) { border-left-color: var(--border-strong); }
    .ccard p { margin: 0; }
    .ccard .chead { display: flex; flex-wrap: wrap; align-items: baseline; gap: 2px 8px; min-width: 0; font-weight: 600; overflow-wrap: anywhere; }
    .ccard .chead > span { min-width: 0; }
    .ccard time { margin-left: auto; font: 400 11.5px var(--font); color: var(--muted); white-space: nowrap; font-variant-numeric: tabular-nums; }
    .ccard .meta { color: var(--muted); font-size: 12.5px; overflow-wrap: anywhere; }
    .ccard a { color: var(--fg); text-decoration: underline; text-decoration-color: var(--border-strong); text-underline-offset: 2px; }
    .ccard a:hover { text-decoration-color: currentColor; }
    .ccard .acts { display: flex; flex-wrap: wrap; gap: 6px; margin-top: 4px; }
    .ccard .acts button { max-width: 100%; white-space: normal; overflow-wrap: anywhere; text-align: start; }
    .ccard .outcome { font-weight: 600; font-size: 13px; }
    .ccard .err { color: var(--danger); font-size: 13px; }
    @media (max-width: 520px) { .ccard .acts button { flex: 1 1 100%; justify-content: flex-start; } }
    .ccard form { display: grid; gap: 8px; }
    .ccard .key { font: 12px var(--mono); }
    .ccard .row { display: flex; flex-wrap: wrap; align-items: end; gap: 8px 14px; }
    .ccard .field { display: flex; flex-direction: column; gap: 4px; min-width: 0; font-size: 12.5px; color: var(--muted); overflow-wrap: anywhere; }
    .ccard .field input { width: 200px; max-width: 100%; }
    .ccard .field input[name="code"] { font-family: var(--mono); text-transform: uppercase; }
    .ccard .field select { max-width: 100%; }
    @media (max-width: 520px) { .ccard .field, .ccard .field input { width: 100%; } }
  }
</style>
