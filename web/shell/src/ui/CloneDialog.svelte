<svelte:options css="injected" />

<script lang="ts">
  // The Clone dialog (hosted spec §9 "Tokens"), loaded on first use: the
  // viewer picks a token's scope among those their rights allow (`read`:
  // clone and fetch; `write`: push as well), makes one, and copies the git
  // and jj commands that use it, for a new clone or a checkout they already
  // have, with when it expires. Each command carries the token as git's
  // `http.extraHeader`, so nothing saved in the checkout holds it (the
  // shell's history keeps a pasted command, which the dialog says). A modal
  // dialog: everything else is inert while it is open, Tab stays inside,
  // Escape, Close or a click outside closes it and focus goes back.
  import { expiresIn, mintToken, type CloneOptions, type CloneScope, type GitToken } from "./clone-api";
  import { onMount, tick } from "svelte";
  import { inertOutside, trapTab } from "./modal";

  let { id, options, onClose }: { id: string; options: CloneOptions; onClose(): void } = $props();
  const back = document.activeElement as HTMLElement | null;
  let panel: HTMLElement | undefined = $state();
  // svelte-ignore state_referenced_locally
  let scope = $state<CloneScope>(options.scopes[0] ?? "read");
  let tok = $state<GitToken | null>(null);
  let busy = $state(false);
  let error = $state("");
  let copied = $state("");
  let timer: ReturnType<typeof setTimeout> | undefined;
  const canWrite = $derived(options.scopes.includes("write"));
  const hours = $derived(Math.round((options.ttl_s ?? 3600) / 3600));
  const focus = (el: HTMLElement) => { el.focus(); };
  let backdrop: HTMLElement | undefined = $state();
  // Once: everything outside the dialog is inert while it is open; on close
  // that is undone, then focus goes back.
  onMount(() => {
    const undo = backdrop ? inertOutside(backdrop) : () => {};
    return () => {
      undo();
      clearTimeout(timer);
      if (back?.isConnected) back.focus?.();
    };
  });

  async function make() {
    if (busy) return;
    busy = true;
    error = "";
    try {
      tok = await mintToken(id, scope);
      // The commands are what the viewer came for: focus the first Copy.
      await tick();
      panel?.querySelector<HTMLElement>("button[aria-label=\"Copy git\"]")?.focus();
    }
    catch (e) { error = e instanceof Error ? e.message.replace(/^\d+ /, "") : "No token was made"; }
    finally { busy = false; }
  }

  function copy(label: string, text: string) {
    void navigator.clipboard?.writeText(text).then(() => {
      copied = label;
      clearTimeout(timer);
      timer = setTimeout(() => { copied = ""; }, 1600);
    }, () => { copied = ""; });
  }

  function keydown(e: KeyboardEvent) {
    if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); onClose(); return; }
    // The dialog's keys are its own, never the shell's.
    if (e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey) e.stopPropagation();
    if (panel) trapTab(e, panel);
  }
</script>

{#snippet field(label: string, text: string)}
  <div class="cf">
    <div class="cf-head"><span class="cf-label">{label}</span><button type="button" class="small" aria-label={`Copy ${label}`} onclick={() => copy(label, text)}>{copied === label ? "Copied" : "Copy"}</button></div>
    <pre aria-label={label}>{text}</pre>
  </div>
{/snippet}

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="clone-backdrop" bind:this={backdrop} onclick={e => { if (e.target === e.currentTarget) onClose(); }}>
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div class="clone-panel" role="dialog" aria-modal="true" aria-labelledby="clone-title" tabindex="-1" bind:this={panel} onkeydown={keydown}>
    <header>
      <h2 id="clone-title">Clone with git or jj</h2>
      <button type="button" class="ghost" {@attach focus} onclick={onClose}>Close</button>
    </header>
    <p class="lede">Each version is a commit on <code>main</code>, tagged <code>v1</code>, <code>v2</code> and so on.{#if canWrite}{" "}A push to <code>main</code> becomes a new version, published by you.{/if}</p>
    <fieldset>
      <legend class="sr">Token</legend>
      {#each options.scopes as s (s)}
        <label><input type="radio" name="clone-scope" value={s} checked={scope === s} onchange={() => { scope = s; }} />
          {#if s === "read"}Read <span class="muted">clone and fetch</span>{:else}Write <span class="muted">also push</span>{/if}</label>
      {/each}
      <!-- Not disabled while busy: focus stays on it, in the dialog. -->
      <button type="button" class="primary" aria-disabled={busy} onclick={make}>{tok ? "Make a new token" : "Make a token"}</button>
    </fieldset>
    {#if error}<p class="error" role="alert">{error}</p>{/if}
    {#if tok}
      <section aria-label="New clone">
        <h3>New clone</h3>
        {@render field("git", tok.clone_command)}
        {@render field("jj", tok.jj_clone_command)}
      </section>
      <section aria-label="Existing checkout">
        <h3>An existing checkout</h3>
        {@render field("git remote", [tok.existing_checkout.add_remote_command, tok.existing_checkout.fetch_command, ...(tok.existing_checkout.push_command ? [tok.existing_checkout.push_command] : [])].join("\n"))}
        {@render field("jj remote", [tok.existing_checkout.jj_add_remote_command, tok.existing_checkout.jj_fetch_command, ...(tok.existing_checkout.jj_push_command ? [tok.existing_checkout.jj_push_command] : [])].join("\n"))}
      </section>
      <p class="meta">The token expires {expiresIn(tok.expires_at)}. Until then anyone holding it can {tok.scope === "write" ? "push to" : "clone"} this artifact. Each command sends it as a header and none saves it in the checkout, but your shell's history keeps a command you paste.</p>
    {:else}
      <p class="meta">A token lives {hours <= 1 ? "an hour" : `${hours} hours`} and is shown once.</p>
    {/if}
    <span class="sr" aria-live="polite">{copied ? `${copied} copied` : ""}</span>
  </div>
</div>

<!-- The dialog's styles travel with its lazy chunk (injected on mount), so
     the artifact entry's eager CSS does not carry them. -->
<style>
  .clone-backdrop { position: fixed; inset: 0; z-index: 40; background: var(--scrim); display: grid; place-items: center; padding: var(--gutter, 16px); }
  .clone-panel { background: var(--raised); color: var(--fg); border: 1px solid var(--border-hover); border-radius: var(--radius); box-shadow: var(--elev-lg); width: min(640px, 100%); max-height: calc(100dvh - 32px); overflow: auto; padding: 16px 20px 20px; box-sizing: border-box; }
  header { display: flex; align-items: center; gap: 12px; margin-bottom: 6px; }
  h2 { margin: 0; font-size: 17px; flex: 1; }
  h3 { margin: 16px 0 6px; font-size: 13px; font-weight: 600; }
  .lede { margin: 0 0 12px; font-size: 13px; color: var(--muted); line-height: 1.6; }
  .lede code { font: 12px var(--mono); background: var(--hover); border: 1px solid var(--border); border-radius: var(--radius-xs); padding: 1px 5px; color: var(--fg); }
  fieldset { border: 0; margin: 0; padding: 0; display: flex; flex-wrap: wrap; align-items: center; gap: 8px 16px; }
  fieldset label { display: inline-flex; align-items: center; gap: 6px; font-weight: 500; font-size: 14px; }
  fieldset label input { min-height: 0; accent-color: var(--accent); margin: 0; }
  fieldset .muted { font-weight: 400; font-size: 12.5px; color: var(--muted); }
  fieldset button { margin-left: auto; }
  .cf { display: grid; gap: 4px; min-width: 0; margin-bottom: 10px; }
  .cf-head { display: flex; align-items: center; justify-content: space-between; gap: 8px; }
  .cf-label { font-size: 12.5px; color: var(--muted); font-weight: 500; }
  .small { min-height: 26px; padding: 3px 10px; font-size: 12.5px; }
  pre { display: block; margin: 0; font: 12.5px/1.55 var(--mono); background: var(--hover); border: 1px solid var(--border); border-radius: var(--radius-xs); padding: 7px 9px; color: var(--fg); overflow-wrap: anywhere; white-space: pre-wrap; user-select: all; }
  .meta { margin: 12px 0 0; font-size: 12.5px; color: var(--muted); line-height: 1.5; }
  .error { margin: 10px 0 0; font-size: 12.5px; color: var(--danger, #c0392b); }
  .sr { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); }
  @media (max-width: 700px) {
    .clone-panel { padding: 14px 14px 16px; }
    fieldset button { margin-left: 0; width: 100%; }
  }
</style>
