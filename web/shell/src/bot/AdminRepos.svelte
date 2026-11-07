<svelte:options css="injected" />

<script lang="ts">
  // Admin › Repositories, the owner's alone: every repository link on the
  // instance, whoever made it, and the instance's GitHub App. Any connected
  // person can make a queue and link it, so each link shows here with its
  // addresses, its queue's maker and admins, and an owner's Disconnect. The
  // App's key reaches every installation and an installation ID proves
  // nothing about who typed it, so an App link waits here for the owner's
  // approval; a token link does too when the owner asks for that. An approval
  // names the `rev` read here, so it covers what the owner saw: each link says
  // what it will do, what makes it dangerous first and marked, whether or not
  // an approval decides anything for it. The key is
  // write-only: a PEM document is pasted (a textarea, since a password field
  // drops its line breaks), sent once and forgotten; only its digest comes
  // back. Changing the forge or the App ID clears the key and voids every App
  // approval.
  import { onMount } from "svelte";
  import { refusal } from "../people/refusal";
  import { relativeTime } from "../format";
  import { type RepoLine, type ReposView, approveRepoLink, deleteGithubApp, disconnectRepoLink, getRepos, putGithubApp, putRepoPolicy } from "./repo-api";
  import Switch from "./Switch.svelte";

  const GITHUB_API = "https://api.github.com";
  let { now }: { now?: () => Date } = $props();
  let v = $state<ReposView | null>(null);
  let err = $state<string | null>(null);
  let saved = $state("");
  let busy = $state(false);
  let appId = $state("");
  let slug = $state("");
  let apiUrl = $state("");
  let pem = $state("");

  const at = (iso: string) => relativeTime(iso, now?.() ?? new Date());
  const key = $derived(v?.app?.key);
  const who = (p: { id: string; name: string | null }) => p.name ?? p.id;
  const host = (u: string) => { try { return new URL(u).host; } catch { return u; } };
  const STATUS: Record<RepoLine["status"], string> = { ready: "ready", needs_token: "no write token yet", needs_read_token: "no read token yet", needs_ci_acceptance: "waiting for its admins to accept CI", needs_app: "no App", needs_approval: "waiting for approval" };
  /** What makes link `l` dangerous, in words for the owner. */
  const risks = (l: RepoLine) => [
    ...l.public ? ["Marked public: what ClaxBot writes goes straight to reporters. If the repository is in fact private, that leaks it."] : [],
    ...l.allow_open_egress ? ["Open network: anything steering ClaxBot could send the source anywhere."] : [],
    ...l.reviewed_previews ? ["Previews reach reporters: its admins say a person approves each deploy."] : [],
    ...l.wake_on_reply ? ["A reporter's reply can start a run: it spends credits, and sets ClaxBot working on their words."] : [],
    ...l.ci_accepted && !l.public ? ["CI may run ClaxBot's branch, with its secrets and network, before anyone reviews it."] : [],
  ];
  function take(x: ReposView) {
    v = x;
    if (x.app) { appId = String(x.app.app_id); slug = x.app.slug; apiUrl = x.app.api_url; }
  }
  const reload = () => getRepos().then(take, e => { err = refusal(e); });
  async function save(e: Event) {
    e.preventDefault();
    if (busy) return;
    const id = Number(appId.trim());
    if (!Number.isInteger(id) || id < 1) { err = "An App ID is a whole number, from the App's settings page."; return; }
    // An empty field means GitHub's own, so blanking it can move the App too.
    const api_url = apiUrl.trim().replace(/\/+$/, "") || GITHUB_API;
    const moved = !!v?.app && (id !== v.app.app_id || api_url !== v.app.api_url);
    if (moved && !confirm("Changing the App ID or forge clears the stored key and voids every App approval. Continue?")) return;
    busy = true; err = null; saved = "";
    try {
      take(await putGithubApp({ app_id: id, slug: slug.trim(), api_url, ...pem.trim() && { private_key: pem } }));
      pem = ""; saved = "Saved.";
    } catch (x) { err = refusal(x); } finally { busy = false; }
  }
  async function act(run: () => Promise<unknown>) {
    if (busy) return;
    busy = true; err = null; saved = "";
    try { await run(); } catch (x) { err = refusal(x); } finally { busy = false; }
    await reload();
  }
  const remove = () => {
    if (!confirm("Remove the GitHub App? Its key is deleted, every App approval is voided, and links using it stop working.")) return;
    void act(async () => { await deleteGithubApp(); appId = ""; slug = ""; apiUrl = ""; });
  };
  function setApproval(on: boolean) {
    const live = v?.links.filter(l => l.access === "token" && l.status === "needs_approval").length ?? 0;
    if (!on && !confirm(`Stop requiring approval of token links? ${live === 1 ? "1 link goes" : `${live} links go`} live at once.`)) return;
    void act(() => putRepoPolicy(on));
  }
  const disconnect = (l: RepoLine) => {
    if (confirm(`Disconnect ${l.repo} from ${l.queue_name}? Its tokens are removed.`)) void act(() => disconnectRepoLink(l.queue_id));
  };
  onMount(() => { void reload(); });
</script>

{#if err}<p class="err" role="alert">{err}</p>{/if}
{#if v}
  <section class="bcard" aria-labelledby="ar-links">
    <h3 id="ar-links">Repository links</h3>
    <p>Every queue's link, whoever made it. An approved link lets ClaxBot read that repository, push branches to it and open pull requests, for that queue's reports.</p>
    <p class="note" data-runtime>
      {#if v.runtime.failed}This Clax's container runtime did not start: nothing runs until its configuration is put right, and repositories go to whatever it names.
      {:else if !v.runtime.configured}This Clax has no container runtime yet: nothing is cloned until one is set up.
      {:else if v.runtime.third_party}Repositories are cloned into ephemeral containers on a third-party runtime, whose systems hold the source while a run lasts.
      {:else}Repositories are cloned into ephemeral containers on this daemon's own runtime.{/if}
      What ClaxBot reads is sent to this Clax's model provider.
    </p>
    <p class="note" data-allowlist>An allow-list only keeps a container away from hosts not on it. If it holds a host people can upload to, such as github.com, anyone who steers ClaxBot with their words could still send source there.</p>
    <div class="field ar-policy"><Switch checked={v.token_approval} label="Token links need my approval" disabled={busy} onchange={setApproval} /></div>
    {#if v.links.length}
      <ul class="ar-lines">
        {#each v.links as l (l.queue_id)}
          <li data-link={l.queue_id}>
            <span class="ar-what">
              <strong>{l.repo}</strong> · {l.access === "github_app" ? `installed on ${l.account}, installation ${l.installation_id}` : "tokens"} · {STATUS[l.status]}<br />
              Queue {l.queue_name} · made by {who(l.maker)} · admins {l.admins.map(who).join(", ") || "none"} · set by {who(l.set_by)} · {host(l.repo_url)} and {host(l.forge_api_url)}
              <span class="ar-terms" data-terms>
                {#each risks(l) as risk (risk)}<span class="ar-risk" data-risk>{risk}</span>{/each}
                {#if !l.public}<span>Private{#if !l.ci_accepted}. CI not accepted yet: nothing runs until its admins accept it{/if}.</span>{/if}
                <span>Preview {l.preview_url}, ready at {l.preview_ready_path}. Protected paths: {l.protected_paths.join(", ") || "only the defaults"}.</span>
              </span>
            </span>
            {#if l.approvable}
              {#if l.approved}
                <button type="button" class="ghost" onclick={() => void act(() => approveRepoLink(l, false))}>Withdraw</button>
              {:else}
                <button type="button" class="primary" onclick={() => void act(() => approveRepoLink(l, true))}>Approve</button>
              {/if}
            {/if}
            <button type="button" class="ghost danger" onclick={() => disconnect(l)}>Disconnect</button>
          </li>
        {/each}
      </ul>
    {:else}
      <p class="note">No queue has a repository.</p>
    {/if}
  </section>
  <section class="bcard" aria-labelledby="ar-app">
    <h3 id="ar-app">GitHub App</h3>
    <p>With an App, a queue's admins connect a repository by installing it, with no token to paste. Without one they paste tokens of their own.</p>
    <form onsubmit={save}>
      <div class="fields">
        <div class="field"><label for="ar-id">App ID</label><input id="ar-id" name="app_id" inputmode="numeric" autocomplete="off" value={appId} oninput={e => { appId = e.currentTarget.value; }} /></div>
        <div class="field"><label for="ar-slug">Slug</label><input id="ar-slug" name="slug" autocomplete="off" spellcheck="false" placeholder="my-clax" value={slug} oninput={e => { slug = e.currentTarget.value; }} /><small>From the App's public URL.</small></div>
        <div class="field"><label for="ar-api">Forge API URL</label><input id="ar-api" name="api_url" autocomplete="off" spellcheck="false" placeholder={GITHUB_API} value={apiUrl} oninput={e => { apiUrl = e.currentTarget.value; }} /><small>The App's tokens go here and nowhere else.</small></div>
      </div>
      <div class="field ar-key">
        <label for="ar-pem">Private key</label>
        <p class="note" data-key>{#if key?.set}Set, digest {key.last4}{#if key.set_at} · set {at(key.set_at)}{/if}{:else}Not set{/if}</p>
        <textarea id="ar-pem" name="private_key" rows="4" spellcheck="false" autocomplete="off" placeholder="-----BEGIN RSA PRIVATE KEY-----" value={pem} oninput={e => { pem = e.currentTarget.value; }}></textarea>
        <small>The .pem file GitHub gave you. It is sealed on saving and never shown again; leave this empty to keep the key.</small>
      </div>
      <div class="acts">
        <button type="submit" class="primary" aria-disabled={busy}>{busy ? "Saving…" : "Save"}</button>
        {#if v.app}<button type="button" class="ghost danger" onclick={remove}>Remove App</button>{/if}
        <span role="status">{#if saved}<span class="ok">{saved}</span>{/if}</span>
      </div>
    </form>
    {#if !v.kek.set}<p class="note">This Clax has no key-encryption key, so it cannot keep the App's key.</p>{/if}
    {#if v.app}<p class="note"><a href={v.app.install_url} target="_blank" rel="noopener noreferrer">The App's install page</a></p>{/if}
  </section>
{:else if !err}
  <p class="note" role="status">Loading the repositories…</p>
{/if}

<style>
  :global {
    .ar-key { margin: 12px 0; }
    .ar-policy { margin: 10px 0; }
    .ar-key textarea { width: 100%; box-sizing: border-box; font: 12.5px/1.45 var(--mono); color: var(--fg); background: var(--bg); border: 1px solid var(--border); border-radius: var(--radius-sm); padding: 6px 8px; }
    .ar-lines { list-style: none; margin: 0; padding: 0; }
    .ar-lines li { display: flex; flex-wrap: wrap; align-items: center; gap: 6px 10px; padding: 6px 0; min-width: 0; }
    .ar-lines li + li { border-top: 1px dashed var(--border); }
    .ar-what { flex: 1 1 240px; min-width: 0; font-size: 13.5px; overflow-wrap: anywhere; }
    .ar-terms { display: grid; gap: 2px; margin-top: 4px; font-size: 12.5px; color: var(--muted); }
    .ar-risk { color: var(--danger); font-weight: 600; }
    .bpg button.danger { color: var(--danger); }
  }
</style>
