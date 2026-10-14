<svelte:options css="injected" />

<script lang="ts">
  // People's part on a hosted daemon (crates/clax-server/src/hosted.rs), for
  // a member (the owner or a person): this browser (who it is signed in
  // as, a one-use sign-in link for another device, sign out), agent tokens
  // for remote agents (made with the `claude mcp add` command and Codex's
  // config, shown this once; listed and revoked), and pages shared by link
  // (anyone with the link may view, or comment once they give a name). Only
  // the owner and a page's maker share it; the daemon says so otherwise.
  import { onMount } from "svelte";
  import { listArtifacts } from "../api";
  import * as api from "./api";

  let { me }: { me: api.HostedMe } = $props();
  const person = $derived(me.person);
  let err = $state<string | null>(null);
  let link = $state<{ url: string; expires_at: string } | null>(null);
  let label = $state("");
  let minted = $state<api.NewAgentToken | null>(null);
  let tokens = $state<api.AgentToken[]>([]);
  let shared = $state<api.SharedLink[]>([]);
  let pages = $state<{ id: string; title: string }[]>([]);
  let pick = $state("");
  let pickAccess = $state<api.LinkAccess>("commenter");
  let copied = $state<string | null>(null);
  const msg = (x: unknown) => (x instanceof Error ? x.message : String(x));
  const ACCESS: Record<api.LinkAccess, string> = { viewer: "Can view", commenter: "Can comment" };
  // Listed by key, not by destructured entries: a destructuring `#each`
  // pulls a runtime helper into the chunk both entries load.
  const LEVELS = Object.keys(ACCESS) as api.LinkAccess[];

  async function load() {
    try {
      const [t, l, a] = await Promise.all([api.tokens(), api.links(), listArtifacts()]);
      tokens = t.tokens; shared = l.links; pages = a.map(x => ({ id: x.id, title: x.title }));
      if (!pick || shared.some(s => s.artifact_id === pick)) pick = pages.find(p => !shared.some(s => s.artifact_id === p.id))?.id ?? "";
    } catch (x) { err = msg(x); }
  }
  async function act(fn: () => Promise<unknown>) {
    err = null;
    try { await fn(); await load(); } catch (x) { err = msg(x); }
  }
  const copy = (what: string, text: string) => { void navigator.clipboard?.writeText(text).then(() => { copied = what; }, () => {}); };
  const makeLink = () => act(async () => { link = await api.signinLink(); });
  const signOut = () => act(async () => { await api.signOut(); location.assign("/signin"); });
  const mint = (e: Event) => { e.preventDefault(); void act(async () => { minted = await api.mintToken(label.trim()); label = ""; }); };
  const share = (e: Event) => { e.preventDefault(); if (pick) void act(() => api.setLink(pick, pickAccess)); };
  const minutes = (iso: string) => Math.max(0, Math.round((Date.parse(iso) - Date.now()) / 60000));
  const day = (iso: string | null) => (iso ? iso.slice(0, 10) : "never");
  const unshared = $derived(pages.filter(p => !shared.some(s => s.artifact_id === p.id)));
  onMount(() => { void load(); });
</script>

<section class="pcard hosted-me" aria-labelledby="hm-h">
  <h3 id="hm-h">This browser</h3>
  <div class="line"><span>Signed in as <strong>{person?.display_name ?? "an unnamed person"}</strong>{person?.owner ? " (the owner)" : ""}</span><span class="sp"></span>
    <button type="button" onclick={makeLink}>Sign in another device</button>
    <button type="button" class="ghost" onclick={signOut}>Sign out</button></div>
  {#if link}
    <p class="lead">Open this link on the other device:</p>
    <div class="cmd"><code>{link.url}</code><button type="button" onclick={() => copy("link", link!.url)}>{copied === "link" ? "Copied" : "Copy"}</button></div>
    <p class="meta">One use; it works for {minutes(link.expires_at)} more minutes. Anyone holding it signs in as you, so send it only to yourself.</p>
  {/if}
</section>

<section class="pcard hosted-tokens" aria-labelledby="ht-h">
  <h3 id="ht-h">Agent tokens</h3>
  <p class="note">An agent on any machine connects to this Clax as you, with your rights, by its token. Make one per machine.</p>
  <form class="mint" onsubmit={mint}>
    <label class="field">Machine <input name="label" autocomplete="off" maxlength="64" placeholder="laptop" value={label} oninput={e => { label = e.currentTarget.value; }} /></label>
    <button class="primary" type="submit">Create token</button>
  </form>
  {#if minted}
    <div class="minted" role="status">
      <p class="lead">Add it to Claude Code:</p>
      <div class="cmd"><code>{minted.claude_command}</code><button type="button" onclick={() => copy("claude", minted!.claude_command)}>{copied === "claude" ? "Copied" : "Copy"}</button></div>
      <p class="lead">Or to Codex, in <code>~/.codex/config.toml</code>:</p>
      <div class="cmd"><pre>{minted.codex_config}</pre><button type="button" onclick={() => copy("codex", minted!.codex_config)}>{copied === "codex" ? "Copied" : "Copy"}</button></div>
      <p class="meta">The token is shown this once.</p>
    </div>
  {/if}
  {#each tokens as t (t.id)}
    <div class="line tok"><span class="mname">{t.label}</span><span class="meta">made {day(t.created_at)} · last used {day(t.last_used_at)}{t.status !== "approved" ? ` · ${t.status}` : ""}</span><span class="sp"></span>
      {#if t.status === "approved"}<button type="button" class="ghost danger" aria-label="Revoke the token for {t.label}" onclick={() => act(() => api.revokeToken(t.id))}>Revoke</button>{/if}</div>
  {/each}
</section>

<section class="pcard hosted-links" aria-labelledby="hl-h">
  <h3 id="hl-h">Shared by link</h3>
  <p class="note">Anyone with a shared page's link sees it. To comment there they give a name and become a guest, whose comments reach your agents only once you approve them.</p>
  {#each shared as s (s.artifact_id)}
    <div class="line"><a class="mname" href={s.url}>{s.title}</a><span class="sp"></span>
      <select aria-label="Who may do what on {s.title}" onchange={e => { const v = e.currentTarget.value; void act(() => api.setLink(s.artifact_id, v === "none" ? null : (v as api.LinkAccess))); }}>
        {#each LEVELS as v (v)}<option value={v} selected={v === s.access}>{ACCESS[v]}</option>{/each}
        <option value="none">Stop sharing</option>
      </select>
      <button type="button" class="ghost" onclick={() => copy(s.artifact_id, s.url)}>{copied === s.artifact_id ? "Copied" : "Copy link"}</button></div>
  {:else}
    <p class="none">No page is shared by link.</p>
  {/each}
  {#if unshared.length}
    <form class="share" onsubmit={share}>
      <select aria-label="Page to share" onchange={e => { pick = e.currentTarget.value; }}>
        {#each unshared as p (p.id)}<option value={p.id} selected={p.id === pick}>{p.title}</option>{/each}
      </select>
      <select aria-label="What its link allows" onchange={e => { pickAccess = e.currentTarget.value as api.LinkAccess; }}>
        {#each LEVELS as v (v)}<option value={v} selected={v === pickAccess}>{ACCESS[v]}</option>{/each}
      </select>
      <button type="submit">Share by link</button>
    </form>
  {/if}
</section>
{#if err}<p class="err" role="alert">{err}</p>{/if}

<style>
  :global {
    .ppl .hosted-me .lead, .ppl .hosted-tokens .lead { margin: 10px 0 6px; font-size: 13.5px; }
    .ppl .hosted-tokens .note, .ppl .hosted-links .note { margin: 0 0 10px; }
    .ppl .cmd { display: flex; gap: 8px; align-items: stretch; min-width: 0; }
    .ppl .cmd code, .ppl .cmd pre { flex: 1; min-width: 0; margin: 0; padding: 8px 10px; border: 1px solid var(--border); border-radius: var(--radius-sm); background: var(--bg); font: 13px/1.4 var(--mono); overflow-wrap: anywhere; white-space: pre-wrap; user-select: all; }
    .ppl .hosted-me .meta, .ppl .minted .meta { display: block; margin: 8px 0 0; }
    .ppl .mint, .ppl .share { display: flex; flex-wrap: wrap; align-items: end; gap: 10px 12px; margin: 0 0 6px; }
    .ppl .mint .field { display: flex; flex-direction: column; gap: 4px; font-size: 12.5px; color: var(--muted); }
    .ppl .mint input { width: 220px; max-width: 100%; }
    .ppl .share { margin-top: 10px; padding-top: 10px; border-top: 1px dashed var(--border); }
    .ppl .tok + .tok { border-top: 1px dashed var(--border); }
    .ppl .hosted-links a.mname { color: var(--fg); }
    @media (max-width: 520px) {
      .ppl .cmd { flex-direction: column; }
      .ppl .cmd button { align-self: flex-start; }
      .ppl .share select { flex: 1 1 100%; }
    }
  }
</style>
