<svelte:options css="injected" />

<script lang="ts">
  // The top bar's Publish button (docs/contract.md "Publishing to a public
  // Clax"): in the owner's shell on a local daemon signed in to a public Clax
  // (`clax remote add`), it publishes the artifact there, and once it is
  // public, links to its public copy and pushes newer versions (Push v<N>).
  // Pressing it opens a confirmation naming where it goes and as whom, with
  // a choice to let agents push new versions without asking; when someone
  // published on the public copy since, it offers a fork of the public copy
  // or a new public artifact instead. Anywhere else (a hosted daemon, another
  // viewer, no remote) it shows nothing. At phone width the buttons are
  // hidden and the more menu lists Publish (`setPublishOpener`). Loaded after
  // the first paint (`more-menu.svelte.ts`).
  import { getToken } from "../api";
  import { hostOf, push, remotesOf, type Link, type Mode, type Remote, type Remotes } from "./publish-api";
  import { setPublishOpener } from "./more-menu.svelte";

  let { id, title, latest }: { id: string; title: string; latest: number } = $props();
  let info = $state<Remotes | null>(null);
  let open = $state(false);
  let busy = $state(false);
  let err = $state<string | null>(null);
  let moved = $state(false);
  let follow = $state(false);
  let done = $state<{ url: string; n: number } | null>(null);
  let root: HTMLElement | undefined = $state();

  const link = $derived<Link | null>(info?.links[0] ?? null);
  const remote = $derived<Remote | null>(
    (link && info?.remotes.find(r => r.id === link.remote_id)) || info?.remotes[0] || null,
  );
  const host = $derived(hostOf(remote?.origin ?? link?.origin ?? null));
  const behind = $derived(link ? Math.max(0, latest - link.pushed_n) : 0);
  const action = $derived(link ? `Push v${latest}` : `Publish to ${host}`);
  const usable = $derived(!!remote);

  async function refresh() {
    const t = await getToken();
    // The owner's shell on a local daemon: a hosted owner's token is `cxo_`.
    if (!t || t.startsWith("cxo_")) return;
    try { info = await remotesOf(id); } catch { info = null; }
  }

  function show() { err = null; moved = false; done = null; follow = link?.auto ?? false; open = true; }
  function close() { open = false; }

  async function go(mode: Mode) {
    if (!remote || busy) return;
    busy = true; err = null;
    try {
      const r = await push(id, remote.id, mode, follow);
      done = { url: r.url, n: r.pushed.length };
      moved = false;
      await refresh();
    } catch (e) {
      const x = e as { code?: string | null; message?: string };
      moved = x.code === "remote_moved";
      err = x.message ?? String(e);
    } finally { busy = false; }
  }

  function outside(e: PointerEvent) {
    if (open && root && !root.contains(e.target as Node)) open = false;
  }
  function key(e: KeyboardEvent) {
    if (open && e.key === "Escape") { e.stopPropagation(); close(); }
  }

  $effect(() => {
    // A newer version refreshes what is behind.
    void latest;
    void refresh();
  });
  $effect(() => {
    setPublishOpener(usable && (!link || behind > 0) ? { label: `${action}…`, open: show } : null);
    return () => setPublishOpener(null);
  });
</script>

<svelte:window onpointerdown={outside} />

{#if usable}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <span class="pub" bind:this={root} onkeydown={key}>
    {#if !link || behind > 0}
      <button type="button" class="publish hide-sm" aria-haspopup="dialog" aria-expanded={open} onclick={() => (open ? close() : show())}>{action}</button>
    {/if}
    {#if link}
      <a class="public hide-sm" href={link.url} target="_blank" rel="noopener noreferrer" title={`Its public copy on ${host}`}>Public ↗</a>
    {/if}
    {#if open}
      <div class="pub-pop" role="dialog" aria-label={action}>
        {#if done}
          <p>{done.n === 0 ? "Up to date" : done.n === 1 ? "Pushed 1 version" : `Pushed ${done.n} versions`} to {host}.</p>
          <div class="pub-acts"><a class="go" href={done.url} target="_blank" rel="noopener noreferrer">Open the public page</a><button type="button" onclick={close}>Close</button></div>
        {:else if moved}
          <p class="err" role="alert">{err}</p>
          <div class="pub-acts">
            <button type="button" class="primary" disabled={busy} onclick={() => go("fork")}>Push onto a fork</button>
            <button type="button" disabled={busy} onclick={() => go("new")}>Publish as new</button>
            <button type="button" onclick={close}>Cancel</button>
          </div>
        {:else}
          {#if link}
            <p>Push {behind === 1 ? `version ${latest}` : `versions ${link.pushed_n + 1} to ${latest}`} of “{title}” to its public copy on {host}, as {remote?.person_name ?? "you"}.</p>
          {:else}
            <p>Publish “{title}” on {host}, as {remote?.person_name ?? "you"}. Its {latest === 1 ? "version" : `${latest} versions`} become public there: whoever its link reaches can read it.</p>
          {/if}
          <label class="pub-follow"><input type="checkbox" checked={follow} onchange={e => { follow = e.currentTarget.checked; }} /> Push new versions without asking me</label>
          {#if err}<p class="err" role="alert">{err}</p>{/if}
          <div class="pub-acts">
            <button type="button" class="primary" disabled={busy} onclick={() => go("update")}>{busy ? "Publishing…" : link ? "Push" : "Publish"}</button>
            <button type="button" onclick={close}>Cancel</button>
          </div>
        {/if}
      </div>
    {/if}
  </span>
{/if}

<style>
  .pub { position: relative; display: inline-flex; align-items: center; gap: 6px; }
  .public { font: 600 14px var(--font); color: var(--accent-ink); padding: 0 6px; white-space: nowrap; text-decoration: none; }
  .public:hover { text-decoration: underline; }
  .publish { white-space: nowrap; }
  .pub-pop { position: absolute; right: 0; top: calc(100% + 8px); z-index: 20; width: 320px; background: var(--raised); color: var(--fg); border: 1px solid var(--border-hover); border-radius: var(--radius); box-shadow: var(--elev-lg); padding: 14px 16px; display: flex; flex-direction: column; gap: 10px; }
  .pub-pop p { margin: 0; font-size: 14px; line-height: 1.45; overflow-wrap: anywhere; }
  .pub-follow { display: flex; align-items: center; gap: 8px; font-size: 13.5px; color: var(--muted); }
  .pub-acts { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; }
  .pub-acts .go { font: 600 14px var(--font); color: var(--accent-ink); margin-right: auto; }
  .err { color: var(--danger); }
  @media (max-width: 700px) {
    .pub { position: static; }
    .pub-pop { position: fixed; left: 16px; right: 16px; top: 64px; width: auto; }
  }
</style>
