<svelte:options css="injected" />

<script lang="ts">
  import { refusal } from "./refusal";
  // Invite a person (spec 2026-10-08-multi-agent-multi-person §3.1, §6.2):
  // a new person by name, or another machine of the owner's (**Me**), at a
  // preset (Collaborator by default). Made, it shows the `clax connect`
  // command (its code is shown this once), "expires in 24 h, one use", the
  // LAN warning and, beside the first invite, the `*unconnected` switch
  // (§9.6). A daemon listening on loopback only answers `not_on_lan`, whose
  // message says how to restart it; no invite is made then. Opened, the
  // keyboard's focus is on Name; made, on Copy.
  import { onMount, tick } from "svelte";
  import type { NewInvite } from "../api";
  import * as api from "./api";
  import { expiresIn, PRESET_NAMES } from "./model";
  import UnconnectedSwitch from "./UnconnectedSwitch.svelte";

  type Props = {
    /** No invite was made before: the switch shows beside this one. */
    first: boolean;
    unconnected: { on: boolean; approval_id: string | null };
    now?: Date;
    onMade(): void;
    onClose(): void;
    onSwitched(): void;
  };
  let { first, unconnected, now, onMade, onClose, onSwitched }: Props = $props();
  let who = $state<"name" | "me">("name");
  let name = $state("");
  let preset = $state("clax:collaborator");
  let busy = $state(false);
  let err = $state<string | null>(null);
  let made = $state<NewInvite | null>(null);
  let copied = $state(false);
  // A second submit while one is in flight is ignored; the button stays
  // enabled so the keyboard's focus stays on it.
  const create = async (e: Event) => {
    e.preventDefault();
    if (busy) return;
    busy = true; err = null;
    try {
      made = (await api.invite(who === "me" ? { me: true } : { name: name.trim() }, who === "me" ? undefined : preset)).invite;
      onMade();
      await tick();
      document.querySelector<HTMLElement>(".invite .cmd button")?.focus();
    } catch (x) { err = refusal(x); } finally { busy = false; }
  };
  // By query, not `bind:this`: its helper would be split into a chunk the
  // artifact view's entry then loads.
  onMount(() => { document.querySelector<HTMLElement>('.invite input[name="name"]')?.focus(); });
  const copy = () => { if (made) void navigator.clipboard?.writeText(made.command).then(() => { copied = true; }, () => {}); };
</script>

<section class="pcard invite" aria-labelledby="invite-h">
  <header><h3 id="invite-h">Invite a person</h3><span class="sp"></span><button type="button" class="ghost" onclick={onClose}>{made ? "Done" : "Cancel"}</button></header>
  {#if made}
    <p class="lead">On {made.person.display_name ? `${made.person.display_name}'s` : "your other"} machine, run:</p>
    <div class="cmd"><code>{made.command}</code><button type="button" onclick={copy}>{copied ? "Copied" : "Copy"}</button></div>
    <p class="meta">Expires in {expiresIn(made.expires_at, now ?? new Date())}, one use. The code is shown this once.</p>
    <p class="warn">Use a trusted LAN: Clax traffic is not encrypted.</p>
    {#if first}<UnconnectedSwitch sw={unconnected} onChanged={onSwitched} />{/if}
  {:else}
    <form onsubmit={create}>
      <fieldset class="who">
        <legend class="sr">Who</legend>
        <label><input type="radio" name="who" value="name" checked={who === "name"} onchange={() => { who = "name"; }} /> A new person</label>
        <label><input type="radio" name="who" value="me" checked={who === "me"} onchange={() => { who = "me"; }} /> Me, on another machine</label>
      </fieldset>
      {#if who === "name"}
        <div class="row">
          <label class="field">Name <input name="name" autocomplete="off" required maxlength="60" value={name} oninput={e => { name = e.currentTarget.value; }} /></label>
          <label class="field">Rights <select onchange={e => { preset = e.currentTarget.value; }}>
            {#each Object.keys(PRESET_NAMES) as v (v)}<option value={v} selected={v === preset}>{PRESET_NAMES[v]}</option>{/each}
          </select></label>
        </div>
      {/if}
      <div class="row"><button class="primary" type="submit" disabled={who === "name" && !name.trim()}>Create invite</button></div>
    </form>
  {/if}
  {#if err}<p class="err" role="alert">{err}</p>{/if}
</section>

<style>
  :global {
    .invite form { display: flex; flex-direction: column; gap: 12px; }
    .invite .who { display: flex; flex-wrap: wrap; gap: 6px 18px; margin: 0; padding: 0; border: 0; }
    .invite .who label { display: inline-flex; align-items: center; gap: 6px; font-size: 14px; }
    .invite .row { display: flex; flex-wrap: wrap; align-items: end; gap: 10px 14px; }
    .invite .field { display: flex; flex-direction: column; gap: 4px; font-size: 12.5px; color: var(--muted); }
    .invite .field input { width: 220px; max-width: 100%; }
    .invite .lead { margin: 0 0 8px; font-size: 13.5px; }
    .invite .cmd { display: flex; gap: 8px; align-items: stretch; }
    .invite .cmd code { flex: 1; min-width: 0; padding: 8px 10px; border: 1px solid var(--border); border-radius: var(--radius-sm); background: var(--bg); font: 13px/1.4 var(--mono); overflow-wrap: anywhere; user-select: all; }
    .invite .meta { margin: 8px 0 0; font-size: 12.5px; color: var(--muted); }
    .invite .warn { margin: 6px 0 14px; font-size: 13px; font-weight: 600; }
    .sr { position: absolute; width: 1px; height: 1px; overflow: hidden; clip: rect(0 0 0 0); white-space: nowrap; }
  }
</style>
