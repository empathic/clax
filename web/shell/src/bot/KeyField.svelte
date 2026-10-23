<svelte:options css="injected" />

<script lang="ts">
  // A write-only key (spec 2026-10-21-claxbot §8.1): never shown, only
  // whether it is set, its last four characters, when and by whom it was
  // set, and when a call last went through with it. Replace (or Set)
  // opens a password field; Save sends it and forgets it; Clear (or
  // Remove) asks first. `readOnly` (a key from the environment) shows the
  // state alone. A refusal shows as the daemon words it.
  import { relativeTime } from "../format";
  import type { KeyInfo } from "./api";
  import { refusal } from "../people/refusal";

  let { info, label, clearLabel = "Clear", readOnly = false, now, onSave, onClear }: {
    info: KeyInfo | null; label: string; clearLabel?: string; readOnly?: boolean; now?: () => Date;
    onSave(key: string): Promise<unknown>; onClear(): Promise<unknown>;
  } = $props();
  let editing = $state(false);
  let value = $state("");
  let input: HTMLInputElement | undefined = $state();
  $effect(() => { input?.focus(); });
  let busy = $state(false);
  let err = $state<string | null>(null);
  const id = `key-${Math.random().toString(36).slice(2, 8)}`;
  const at = (iso: string) => relativeTime(iso, now?.() ?? new Date());
  const status = $derived(!info?.set ? "Not set" : `Set, ending ${info.last4 ?? "…"}${info.set_at ? ` · set ${at(info.set_at)}${info.set_by ? ` by ${info.set_by.name ?? info.set_by.id}` : ""}` : ""}${info.last_ok_at ? ` · last call OK ${at(info.last_ok_at)}` : ""}`);

  async function save(e: SubmitEvent) {
    e.preventDefault();
    if (!value.trim() || busy) return;
    busy = true; err = null;
    try { await onSave(value.trim()); value = ""; editing = false; } catch (x) { err = refusal(x); } finally { busy = false; }
  }
  async function clear() {
    if (busy || !confirm(`${clearLabel} the ${label.toLowerCase()}? ClaxBot stops using it at once.`)) return;
    busy = true; err = null;
    try { await onClear(); } catch (x) { err = refusal(x); } finally { busy = false; }
  }
</script>

<div class="bkey">
  <p class="bkey-s" id={`${id}-s`}><span class="bkey-dots" aria-hidden="true">{info?.set ? "••••••••" : ""}</span>{status}</p>
  {#if !readOnly}
    {#if editing}
      <form onsubmit={save}>
        <label class="bot-sr" for={id}>{label}</label>
        <input {id} name="clax-provider-api-key" type="password" autocomplete="new-password" data-1p-ignore data-lpignore="true" spellcheck="false" placeholder="sk-ant-…" bind:value bind:this={input} />
        <button type="submit" class="primary" aria-disabled={busy || !value.trim()}>{busy ? "Saving…" : "Save"}</button>
        <button type="button" class="ghost" onclick={() => { editing = false; value = ""; err = null; }}>Cancel</button>
      </form>
    {:else}
      <div class="bkey-acts">
        <button type="button" aria-describedby={`${id}-s`} onclick={() => { editing = true; err = null; }}>{info?.set ? "Replace" : "Set"} <span class="bot-sr">{label}</span></button>
        {#if info?.set}<button type="button" class="ghost danger" disabled={busy} onclick={clear}>{clearLabel} <span class="bot-sr">{label}</span></button>{/if}
      </div>
    {/if}
  {/if}
  {#if err}<p class="err" role="alert">{err}</p>{/if}
</div>

<style>
  :global {
    .bkey { display: grid; gap: 6px; min-width: 0; }
    .bkey-s { margin: 0; font-size: 13px; color: var(--muted); overflow-wrap: anywhere; }
    .bkey-dots { font-family: var(--mono); color: var(--fg); margin-right: 8px; letter-spacing: .1em; }
    .bkey form, .bkey-acts { display: flex; flex-wrap: wrap; gap: 6px; }
    .bkey input { flex: 1 1 200px; min-width: 0; font-family: var(--mono); }
    .bkey .danger { color: var(--danger); }
    .bkey .err { margin: 0; color: var(--danger); font-size: 13px; }
  }
</style>
