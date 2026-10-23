<svelte:options css="injected" />

<script lang="ts">
  // The Share dialog (crates/clax-server/src/hosted.rs, the sharing routes),
  // loaded on first use, for whoever may share the artifact (the owner and
  // its maker). First the Comments switch: off, nobody comments on the
  // artifact, whatever their access (`onComments` hears each change). Add people: a GitHub login (`@login`), or one of the people
  // here the daemon suggests (the owner's dialog only), at Can view, Can
  // comment or Can edit; a login no one has signed in with yet gets access
  // at its first GitHub sign-in. The people with access, each with their
  // access and Remove. General access: Restricted (the people added alone),
  // Anyone with the link, or Public (listed on the public page too), the
  // last two at Can view or Can comment. Copy link. Names and logins are
  // people's text, rendered as text. A modal dialog: everything else is
  // inert while it is open, Tab stays inside, Escape, Done or a click
  // outside closes it and focus goes back.
  import { onMount, tick } from "svelte";
  import * as api from "../hosted/api";
  import { inertOutside, trapTab } from "./modal";
  import {
    ACCESS_LABEL, ACCESSES, COMMENTS_LABEL, COMMENTS_NOTE, GENERAL_LABEL, GENERAL_NOTE, GENERALS, LINK_ACCESSES,
    linkRequest, shareDetail, shareName, subject, suggest,
  } from "./share-model";

  type Open = Extract<api.Sharing, { can_share: true }>;
  let { id, title, initial, onClose, onComments }: { id: string; title: string; initial: Open; onClose(): void; onComments?(on: boolean): void } = $props();
  const back = document.activeElement as HTMLElement | null;
  // svelte-ignore state_referenced_locally
  let data = $state<Open>(initial);
  let query = $state("");
  let chosen = $state<api.ShareCandidate | null>(null);
  let addAccess = $state<api.ShareAccess>("viewer");
  let busy = $state(false);
  let err = $state("");
  let copied = $state(false);
  let panel: HTMLElement | undefined = $state();
  let backdrop: HTMLElement | undefined = $state();
  let timer: ReturnType<typeof setTimeout> | undefined;
  const hints = $derived(chosen ? [] : suggest(query, data.suggestions, data.people));
  const target = $derived(subject(query, chosen, data.suggestions));
  const linkAccess = $derived<api.LinkAccess>(data.link_access ?? "viewer");

  onMount(() => {
    const undo = backdrop ? inertOutside(backdrop) : () => {};
    panel?.querySelector<HTMLElement>("input[name=\"who\"]")?.focus();
    return () => { undo(); clearTimeout(timer); if (back?.isConnected) back.focus?.(); };
  });

  async function act(fn: () => Promise<unknown>): Promise<boolean> {
    if (busy) return false;
    busy = true; err = "";
    try {
      await fn();
      const fresh = await api.sharing(id);
      if (fresh.can_share) data = fresh; else onClose();
      return true;
    } catch (x) { err = x instanceof Error ? x.message : String(x); return false; } finally { busy = false; }
  }

  async function add(e: Event) {
    e.preventDefault();
    const to = target;
    if (!to) { err = "Type a GitHub login, such as @octocat, or pick someone."; return; }
    // Only the text sent is cleared: what is typed while the add is in flight stays.
    const sent = query;
    if (await act(() => api.share(id, to, addAccess)) && query === sent) {
      query = ""; chosen = null;
      await tick();
      panel?.querySelector<HTMLElement>("input[name=\"who\"]")?.focus();
    }
  }
  const pick = (c: api.ShareCandidate) => { chosen = c; query = c.name ?? `@${c.github_login}`; };
  const typed = (v: string) => { query = v; chosen = null; err = ""; };
  const change = (s: api.Share, access: api.ShareAccess) =>
    act(() => api.share(id, s.person ? { person: s.person } : { github_login: s.github_login ?? "" }, access));
  async function toggleComments(box: HTMLInputElement) {
    const on = box.checked;
    // The change is heard once the daemon holds it, even if the dialog's
    // reload after it fails.
    await act(async () => {
      const held = await api.setComments(id, on);
      data = { ...data, comments: held };
      onComments?.(held);
    });
    // The switch shows what the daemon holds, whether or not the change took.
    box.checked = data.comments;
  }
  const remove = (s: api.Share) => act(() => api.unshare(id, s.id));
  const setGeneral = (g: api.GeneralAccess, a: api.LinkAccess) => {
    const r = linkRequest(g, a);
    return act(() => api.setLink(id, r.access, r.listed));
  };

  function copy() {
    void navigator.clipboard?.writeText(data.url).then(() => {
      copied = true;
      clearTimeout(timer);
      timer = setTimeout(() => { copied = false; }, 1600);
    }, () => {});
  }

  function keydown(e: KeyboardEvent) {
    if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); onClose(); return; }
    // The dialog's keys are its own, never the shell's.
    if (e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey) e.stopPropagation();
    if (panel) trapTab(e, panel);
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="share-backdrop" bind:this={backdrop} onclick={e => { if (e.target === e.currentTarget) onClose(); }}>
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div class="share-panel" role="dialog" aria-modal="true" aria-labelledby="share-title" aria-busy={busy} tabindex="-1" bind:this={panel} onkeydown={keydown}>
    <header>
      <h2 id="share-title">Share <span class="t">{title}</span></h2>
    </header>

    <div class="comments-sw">
      <label class="sw"><input type="checkbox" role="switch" aria-describedby="share-comments-note" checked={data.comments} disabled={busy} onchange={e => { void toggleComments(e.currentTarget); }} /><span class="track" aria-hidden="true"></span><span class="lbl">{COMMENTS_LABEL}</span></label>
      <p class="meta" id="share-comments-note" data-testid="comments-note">{data.comments ? COMMENTS_NOTE.on : COMMENTS_NOTE.off}</p>
    </div>

    <form class="add" onsubmit={add} aria-label="Add people">
      <div class="add-who">
        <input name="who" autocomplete="off" spellcheck="false" placeholder={data.suggestions.length ? "@github-login or a name" : "@github-login"}
          aria-label="GitHub login" value={query} oninput={e => typed(e.currentTarget.value)} />
        {#if hints.length}
          <ul class="hints" aria-label="Suggestions">
            {#each hints as c (c.person)}
              <li><button type="button" class="ghost" onclick={() => pick(c)}><span class="nm">{c.name ?? `@${c.github_login}`}</span>{#if c.github_login && c.name}<span class="lg">@{c.github_login}</span>{/if}</button></li>
            {/each}
          </ul>
        {/if}
      </div>
      <select aria-label="Access for the person added" onchange={e => { addAccess = e.currentTarget.value as api.ShareAccess; }}>
        {#each ACCESSES as a (a)}<option value={a} selected={a === addAccess}>{ACCESS_LABEL[a]}</option>{/each}
      </select>
      <button type="submit" class="primary" aria-disabled={busy || !target}>Add</button>
    </form>
    <p class="meta">They get access when they sign in with GitHub. Can edit also publishes new versions and resolves threads; only the owner and the maker share or delete.</p>

    <h3>People with access</h3>
    {#if data.people.length}
      <ul class="shared">
        {#each data.people as s (s.id)}
          <li>
            <span class="av" aria-hidden="true">{shareName(s).replace(/^@/, "").slice(0, 1).toUpperCase()}</span>
            <span class="who-l"><span class="nm">{shareName(s)}</span>{#if shareDetail(s)}<span class="lg">{shareDetail(s)}</span>{/if}</span>
            <select aria-label="Access for {shareName(s)}" onchange={e => { void change(s, e.currentTarget.value as api.ShareAccess); }}>
              {#each ACCESSES as a (a)}<option value={a} selected={a === s.access}>{ACCESS_LABEL[a]}</option>{/each}
            </select>
            <button type="button" class="ghost small" aria-label="Remove {shareName(s)}" onclick={() => remove(s)}>Remove</button>
          </li>
        {/each}
      </ul>
    {:else}
      <p class="none">No one yet. The owner and the maker can always open it.</p>
    {/if}

    <h3>General access</h3>
    <div class="general">
      <select aria-label="General access" onchange={e => { void setGeneral(e.currentTarget.value as api.GeneralAccess, linkAccess); }}>
        {#each GENERALS.filter(g => g !== "public" || data.public_gallery || data.general === "public") as g (g)}<option value={g} selected={g === data.general}>{GENERAL_LABEL[g]}</option>{/each}
      </select>
      {#if data.general !== "restricted"}
        <select aria-label="What anyone with the link may do" onchange={e => { void setGeneral(data.general, e.currentTarget.value as api.LinkAccess); }}>
          {#each LINK_ACCESSES as a (a)}<option value={a} selected={a === linkAccess}>{ACCESS_LABEL[a]}</option>{/each}
        </select>
      {/if}
    </div>
    <p class="meta" data-testid="general-note">{GENERAL_NOTE[data.general]}</p>

    {#if err}<p class="error" role="alert">{err}</p>{/if}
    <footer>
      <button type="button" class="copy" onclick={copy}>{copied ? "Copied" : "Copy link"}</button>
      <span class="sp"></span>
      <button type="button" class="primary" onclick={onClose}>Done</button>
    </footer>
    <span class="sr" aria-live="polite">{copied ? "Link copied" : ""}</span>
  </div>
</div>

<!-- The dialog's styles travel with its lazy chunk (injected on mount). -->
<style>
  .share-backdrop { position: fixed; inset: 0; z-index: 40; background: var(--scrim); display: grid; place-items: center; padding: var(--gutter, 16px); }
  .share-panel { background: var(--raised); color: var(--fg); border: 1px solid var(--border-hover); border-radius: var(--radius); box-shadow: var(--elev-lg); width: min(560px, 100%); max-height: calc(100dvh - 32px); overflow: auto; padding: 16px 20px 18px; box-sizing: border-box; }
  header { display: flex; align-items: center; gap: 12px; margin-bottom: 12px; }
  h2 { margin: 0; font-size: 17px; flex: 1; min-width: 0; overflow-wrap: anywhere; }
  h2 .t { font-weight: 600; }
  h3 { margin: 18px 0 8px; font-size: 13px; font-weight: 600; }
  .add { display: flex; gap: 8px; align-items: flex-start; flex-wrap: wrap; }
  .add-who { position: relative; flex: 1 1 220px; min-width: 0; }
  .add-who input { width: 100%; box-sizing: border-box; margin: 0; }
  .hints { position: absolute; left: 0; right: 0; top: calc(100% + 4px); z-index: 2; list-style: none; margin: 0; padding: 4px 0; background: var(--raised); border: 1px solid var(--border-hover); border-radius: var(--radius-sm); box-shadow: var(--elev-lg); }
  .hints button { display: flex; width: 100%; gap: 8px; align-items: baseline; justify-content: flex-start; border: 0; border-radius: 0; padding: 7px 12px; min-height: 0; text-align: left; }
  .hints button:hover { background: var(--hover); }
  .nm { font-weight: 500; font-size: 14px; overflow-wrap: anywhere; }
  .lg { font-size: 12.5px; color: var(--muted); overflow-wrap: anywhere; }
  .shared { list-style: none; margin: 0; padding: 0; display: grid; gap: 2px; }
  .shared li { display: flex; align-items: center; gap: 10px; padding: 6px 0; }
  .shared li + li { border-top: 1px dashed var(--border); }
  .av { flex: none; width: 28px; height: 28px; border-radius: 50%; display: grid; place-items: center; background: var(--hover); border: 1px solid var(--border); font: 600 12.5px/1 var(--font); color: var(--muted); }
  .who-l { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 1px; }
  .comments-sw { margin: 0 0 16px; padding-bottom: 14px; border-bottom: 1px solid var(--border); }
  .comments-sw .meta { margin-top: 4px; padding-left: 44px; }
  .sw { display: inline-flex; align-items: center; gap: 10px; cursor: pointer; font-weight: 600; font-size: 14px; position: relative; }
  .sw input { position: absolute; opacity: 0; width: 34px; height: 20px; margin: 0; cursor: pointer; }
  .sw .track { flex: none; width: 34px; height: 20px; border-radius: 999px; background: var(--border-strong); position: relative; transition: background-color var(--t); }
  .sw .track::after { content: ""; position: absolute; top: 2px; left: 2px; width: 16px; height: 16px; border-radius: 50%; background: var(--raised); box-shadow: 0 1px 2px rgb(0 0 0 / .25); transition: transform var(--t); }
  .sw input:checked + .track { background: var(--fg); }
  .sw input:checked + .track::after { transform: translateX(14px); }
  .sw input:focus-visible + .track { outline: 2px solid var(--focus); outline-offset: 2px; }
  .general { display: flex; flex-wrap: wrap; gap: 8px; }
  .meta, .none { margin: 8px 0 0; font-size: 12.5px; color: var(--muted); line-height: 1.5; }
  .none { margin: 0; }
  .small { min-height: 28px; padding: 3px 10px; font-size: 12.5px; }
  .error { margin: 12px 0 0; font-size: 12.5px; color: var(--danger, #c0392b); }
  footer { display: flex; align-items: center; gap: 8px; margin-top: 18px; padding-top: 14px; border-top: 1px solid var(--border); }
  footer .sp { flex: 1; }
  .sr { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); }
  @media (max-width: 520px) {
    .share-panel { padding: 14px 14px 16px; }
    .add select { flex: 1; }
    .shared li { flex-wrap: wrap; }
    .shared li .who-l { flex-basis: calc(100% - 38px); }
    .shared li select { margin-left: 38px; flex: 1; }
  }
</style>
