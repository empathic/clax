<svelte:options css="injected" />

<script lang="ts">
  // A queue's repository, for its admins: what it is linked to, whether the
  // link can be used yet, and how to reach it. The instance's GitHub App is
  // offered when it has one, with its install link, and the daemon asks the
  // forge which installation holds the repository; a token works anywhere.
  // Tokens stay out of the form: each is set and cleared on its own
  // (`KeyField`), beside the hosts it goes to, and only its metadata comes
  // back. Every change names the `rev` this panel read, and a link another
  // admin changed since is refused: the panel reads it again and says so,
  // so nobody seals a token to an address they did not see; what was typed
  // and not saved stays, and every other field shows the link as it now is.
  // A URL field holding the default stays empty, so renaming the repository
  // moves it. Its line says what a link that cannot run still needs, and a
  // private repository's admin is told what CI would do with ClaxBot's branch
  // and asked to accept it, as Clax cannot see what a repository's CI runs.
  import { onMount } from "svelte";
  import type { Queue } from "./api";
  import * as api from "../bot/repo-api";
  import { refusal } from "../people/refusal";
  import KeyField from "../bot/KeyField.svelte";

  const p: { queue: Queue; now?: () => Date } = $props();
  // svelte-ignore state_referenced_locally
  const qid = p.queue.id;
  const GITHUB_API = "https://api.github.com";
  // A run that has begun keeps what it was offered until it ends.
  const CLEAR_SAYS = "New runs stop using it; a run under way goes on with it.";
  // A refusal of one of these fields, which sit under More settings.
  const MORE = new Set(["invalid_repo_url", "invalid_forge_url", "forge_required", "repo_url_mismatch", "forge_not_instance", "invalid_ready_path", "invalid_preview_timing", "invalid_protected_path"]);
  let v = $state<api.RepoView | null>(null);
  let loading = $state(true);
  let loadErr = $state<string | null>(null);
  let err = $state<string | null>(null);
  let busy = $state(false);
  let more = $state(false);
  // The form, and what it held when last filled from the link: a field that
  // differs from that was typed here and not saved.
  const EMPTY = {
    kind: "token" as "token" | "github_app", repo: "", base: "main", repoUrl: "", forgeUrl: "", previewUrl: "", readyPath: "/healthz", namesCommit: true,
    timeoutSecs: "1200", pollSecs: "10", paths: "", isPublic: false, openEgress: false, reviewed: false, wake: false, ciAccepted: false,
  };
  let f = $state({ ...EMPTY });
  let filled = { ...EMPTY };
  // Whether a read of a link that moved on kept what was typed.
  let kept = $state(false);

  const link = $derived(v?.repo ?? null);
  const tokens = $derived(link?.access.kind === "token" ? link.access : null);
  // An App link keeps offering the App's choice, whatever became of the App.
  const offerApp = $derived(!!v?.app.configured || link?.access.kind === "github_app");
  const status = $derived.by(() => {
    if (!link) return "Not connected.";
    if (link.status === "needs_token") return `Not ready: add a write token${!link.public && !tokens?.read_token.set ? " and a read token" : ""} below.`;
    if (link.status === "needs_read_token") return "Not ready: add a read token below. ClaxBot cannot clone a private repository without one.";
    if (link.status === "needs_ci_acceptance") return "Not ready: this repository's CI needs your say-so below. Read what it would do, tick the box if that is all right, and save.";
    if (link.status === "needs_app") return "Waiting for the owner of this Clax to set up its GitHub App.";
    if (link.status === "needs_approval") return `Waiting for the owner of this Clax to approve this ${link.access.kind === "github_app" ? "repository and installation" : "repository"}.`;
    if (v?.runtime.failed) return "Details saved. Nothing can use them: this Clax's container runtime did not start.";
    if (!v?.runtime.configured) return "Details saved. Nothing can use them until this Clax has a container runtime.";
    if (v.runtime.operator_target) return "Details saved, and not in use: this Clax's operator names this queue's repository in their own configuration, and that one runs.";
    if (v.runtime.operator_only) return "Details saved, and not in use: this Clax runs only the repositories its operator names, so a repository connected here cannot run.";
    if (!link.public && !link.allow_open_egress && !v.runtime.egress_held) return "Details saved. Nothing can use them: a confidential repository needs its containers' network held to an allow-list, or open network access allowed below.";
    if (link.last_failure) return `The last try failed. ${link.last_failure}`;
    return link.last_ok_at ? "Connected." : "Details saved; nothing published yet.";
  });
  const host = (u: string) => { try { return new URL(u).host; } catch { return u; } };
  const hosts = (l: api.RepoLink) => [...new Set([host(l.repo_url), host(l.forge_api_url)])].join(" and ");

  function fill(l: api.RepoLink) {
    const token = l.access.kind === "token";
    const repoUrl = token && l.repo_url !== `https://github.com/${l.repo}.git` ? l.repo_url : "";
    filled = {
      kind: l.access.kind, repo: l.repo, base: l.base, repoUrl,
      // Off GitHub a forge must be named, so it stays in the field.
      forgeUrl: token && (repoUrl || l.forge_api_url !== GITHUB_API) ? l.forge_api_url : "",
      previewUrl: l.preview_url, readyPath: l.preview_ready_path, namesCommit: l.preview_names_commit, timeoutSecs: String(l.preview_timeout_secs), pollSecs: String(l.preview_poll_secs),
      paths: l.protected_paths.join("\n"), isPublic: l.public, openEgress: l.allow_open_egress, reviewed: l.reviewed_previews, wake: l.wake_on_reply, ciAccepted: l.ci_accepted,
    };
    f = { ...filled };
    kept = false;
  }
  function take(x: api.RepoView) { v = x; if (x.repo) fill(x.repo); }
  /** Takes `x`, every field not typed in showing what the link is now. True if anything typed stayed. */
  function merge(x: api.RepoView): boolean {
    const typed = Object.entries(f).filter(([k, val]) => val !== filled[k as keyof typeof filled]);
    take(x);
    f = { ...f, ...Object.fromEntries(typed) };
    return typed.length > 0;
  }
  async function load() {
    loading = true; loadErr = null;
    try { take(await api.getRepo(qid)); } catch (x) { loadErr = `Could not load the repository: ${refusal(x)}`; } finally { loading = false; }
  }
  // A refusal for a link that moved on: read what it is now, and say why.
  // What was typed stays, for its author to check against what now shows.
  async function stale(x: unknown) {
    if ((x as { code?: unknown }).code !== "link_changed") return;
    try { kept = merge(await api.getRepo(qid)); } catch (y) { loadErr = `Could not load the repository: ${refusal(y)}`; }
  }
  async function setToken(which: "read" | "write", k: string) {
    if (!link) return;
    try { merge(await api.putRepoToken(qid, which, k, link.rev)); } catch (x) { await stale(x); throw x; }
  }

  async function save(e: Event) {
    e.preventDefault();
    if (busy) return;
    const body: api.RepoFields = {
      repo: f.repo.trim(), base: f.base.trim(), preview_url: f.previewUrl.trim(), protected_paths: f.paths.split("\n").map(s => s.trim()).filter(Boolean),
      preview_ready_path: f.readyPath.trim(), preview_names_commit: f.namesCommit, preview_timeout_secs: Number(f.timeoutSecs), preview_poll_secs: Number(f.pollSecs),
      access: { kind: f.kind }, public: f.isPublic, allow_open_egress: f.openEgress, reviewed_previews: f.reviewed, wake_on_reply: f.wake, ci_accepted: f.ciAccepted, if_rev: link?.rev ?? null,
    };
    if (f.kind === "token" && f.repoUrl.trim()) body.repo_url = f.repoUrl.trim();
    if (f.kind === "token" && f.forgeUrl.trim()) body.forge_api_url = f.forgeUrl.trim();
    busy = true; err = null;
    try { take(await api.putRepo(qid, body)); } catch (x) {
      const code = (x as { code?: unknown }).code;
      if (typeof code === "string" && MORE.has(code)) more = true;
      const text = refusal(x);
      await stale(x);
      err = text;
    } finally { busy = false; }
  }
  async function disconnect() {
    if (busy || !link || !confirm(`Disconnect ${link.repo} from ${p.queue.name}? Its tokens are removed.`)) return;
    busy = true; err = null;
    try { await api.deleteRepo(qid, link.rev); if (v) v = { ...v, repo: null }; } catch (x) { const text = refusal(x); await stale(x); err = text; } finally { busy = false; }
  }
  onMount(() => { void load(); });
</script>

<section class="part" aria-label={`Repository of ${p.queue.name}`}>
  <h4>Repository</h4>
  {#if loadErr}
    <p class="err" role="alert">{loadErr}</p>
    <button type="button" onclick={() => void load()}>Try again</button>
  {:else if loading && !v}
    <p class="hint" role="status">Loading the repository…</p>
  {/if}
  {#if v}
    <p class="stat" role="status">{#if link}<strong>{link.repo}</strong> · {/if}{status}</p>
    <div class="hint" data-runtime role="note">
      <p>
        {#if v.runtime.failed}This Clax's container runtime did not start, so nothing runs for now. Once its operator puts that right, private source goes to whatever runtime it names, which may be a third party's.
        {:else if !v.runtime.configured}This Clax has no container runtime yet: nothing is cloned until its operator sets one up.
        {:else if v.runtime.operator_only}This Clax runs only the repositories its operator names: nothing connected here is cloned.
        {:else if v.runtime.third_party}Private source is cloned into an ephemeral container on a third-party runtime. While a run lasts, that provider's systems hold your source and the read token. They never get the write token or a model key.
        {:else}Source is cloned into an ephemeral container on this daemon's own runtime.{/if}
      </p>
      <p>What ClaxBot reads of the repository is sent to this Clax's model provider.</p>
      {#if !f.isPublic}<p data-allowlist>An allow-list only keeps a container away from hosts not on it. If it holds a host people can upload to, such as github.com, anyone who steers ClaxBot with their words could still send your source there.</p>{/if}
    </div>
    {#if !v.runtime.operator_only || link}
    <form class="rform" onsubmit={save}>
      <div class="grid">
        <label class="field">Repository <input name="repo" autocomplete="off" spellcheck="false" required placeholder="owner/name" value={f.repo} oninput={e => { f.repo = e.currentTarget.value; }} /></label>
        <label class="field">Base branch <input name="base" autocomplete="off" spellcheck="false" value={f.base} oninput={e => { f.base = e.currentTarget.value; }} /></label>
        <label class="field">Preview URL <input name="preview_url" autocomplete="off" spellcheck="false" required placeholder={"https://pr-{pr}.example.com"} value={f.previewUrl} oninput={e => { f.previewUrl = e.currentTarget.value; }} /></label>
      </div>
      <p class="hint" data-no-preview>With no working preview, ClaxBot's fix waits for a person: the reporter gets nothing until someone hands it over with a link.</p>
      <fieldset class="how">
        <legend>Reach it with</legend>
        {#if offerApp}<label><input type="radio" name="repo-access" value="github_app" checked={f.kind === "github_app"} onchange={() => { f.kind = "github_app"; }} /> This Clax's GitHub App</label>{/if}
        <label><input type="radio" name="repo-access" value="token" checked={f.kind === "token"} onchange={() => { f.kind = "token"; }} /> Tokens you paste</label>
      </fieldset>
      {#if f.kind === "github_app"}
        {#if v.app.install_url}<p class="hint"><a data-install href={v.app.install_url} target="_blank" rel="noopener noreferrer">Install the App on the repository</a> first: Clax then asks the forge where it is installed. The owner of this Clax approves each repository, since the App reaches all it is installed on.</p>{/if}
        {#if link?.access.kind === "github_app"}<p class="hint" data-account>Installed on {link.access.account}.</p>{/if}
      {/if}
      {#if !f.isPublic}
        <div class="ci">
          <label class="check"><input type="checkbox" name="ci_accepted" checked={f.ciAccepted} onchange={e => { f.ciAccepted = e.currentTarget.checked; }} /> This repository's CI may run ClaxBot's branches</label>
          <p class="hint" data-ci>When ClaxBot pushes a branch, your repository's CI may start on it by itself, before anyone has read the change. That runs what ClaxBot wrote, with the secrets and network access your CI has. Clax cannot see what your CI does, so it asks you. Tick this only if that is all right, for example because branch builds get no secrets or wait for a person. Until then ClaxBot does not work on this repository.</p>
        </div>
      {/if}
      <details class="more" open={more} ontoggle={e => { more = e.currentTarget.open; }}>
        <summary>More settings</summary>
        <div class="grid">
          {#if f.kind === "token"}
            <label class="field">Repository URL <input name="repo_url" autocomplete="off" spellcheck="false" placeholder="GitHub's address" value={f.repoUrl} oninput={e => { f.repoUrl = e.currentTarget.value; }} /></label>
            <label class="field">Forge API URL <input name="forge_api_url" autocomplete="off" spellcheck="false" placeholder={GITHUB_API} value={f.forgeUrl} oninput={e => { f.forgeUrl = e.currentTarget.value; }} /></label>
          {/if}
          <label class="field">Readiness path <input name="preview_ready_path" autocomplete="off" spellcheck="false" value={f.readyPath} oninput={e => { f.readyPath = e.currentTarget.value; }} /></label>
          <label class="field">Preview timeout, seconds <input name="preview_timeout_secs" type="number" min="1" step="1" inputmode="numeric" value={f.timeoutSecs} oninput={e => { f.timeoutSecs = e.currentTarget.value; }} /></label>
          <label class="field">Poll every, seconds <input name="preview_poll_secs" type="number" min="1" step="1" inputmode="numeric" value={f.pollSecs} oninput={e => { f.pollSecs = e.currentTarget.value; }} /></label>
        </div>
        <label class="check"><input type="checkbox" name="preview_names_commit" checked={f.namesCommit} onchange={e => { f.namesCommit = e.currentTarget.checked; }} /> The readiness answer names the commit it serves</label>
        <label class="check"><input type="checkbox" name="public" checked={f.isPublic} onchange={e => { f.isPublic = e.currentTarget.checked; }} /> Anyone may see this repository</label>
        <p class="hint">Leave this off for a private repository. ClaxBot then treats it as confidential: what it writes goes to the queue's members, and a reporter hears only that a fix is ready. Turn it on only if the source is public already.</p>
        <label class="check"><input type="checkbox" name="allow_open_egress" checked={f.openEgress} onchange={e => { f.openEgress = e.currentTarget.checked; }} /> Run a confidential repository with open network access</label>
        <p class="hint">A confidential repository needs its container's network narrowed to an allow-list. With this on it runs without one, and anything steering ClaxBot could send your source anywhere.</p>
        <label class="check"><input type="checkbox" name="reviewed_previews" checked={f.reviewed} onchange={e => { f.reviewed = e.currentTarget.checked; }} /> A person approves each preview deploy</label>
        <p class="hint">Turn this on only if your deploys really wait for a person to approve the commit. It lets the preview link reach the reporter of a confidential repository; otherwise it goes to the queue's members, since a preview is built from what ClaxBot wrote.</p>
        <label class="check"><input type="checkbox" name="wake_on_reply" checked={f.wake} onchange={e => { f.wake = e.currentTarget.checked; }} /> A reporter's reply to a waiting fix starts a run</label>
        <p class="hint">Off, only a reopen starts one. On, any reply from a reporter spends credits and sets ClaxBot working on their words.</p>
        <label class="field">Protected paths, one per line
          <textarea name="protected_paths" rows="3" spellcheck="false" placeholder="infra/" value={f.paths} oninput={e => { f.paths = e.currentTarget.value; }}></textarea>
          <span class="hint">A file, or a directory ending in /. ClaxBot's changes may not touch them, on top of .github/ and .gitmodules.</span>
        </label>
      </details>
      {#if err}<p class="err" role="alert">{err}</p>{/if}
      {#if kept}<p class="hint" data-kept>What you typed is kept, and the rest shows the link as it now is. Check it before you save.</p>{/if}
      <div class="row">
        <button class="primary" type="submit" aria-disabled={busy}>{busy ? "Saving…" : link ? "Save" : "Connect"}</button>
        {#if link}<button class="ghost danger" type="button" onclick={() => void disconnect()}>Disconnect</button>{/if}
      </div>
    </form>
    {/if}
    {#if tokens && link}
      <div class="tokens">
        <div data-token="write">
          <span class="lbl">Write token</span>
          <KeyField info={tokens.write_token} label="Write token" name="clax-repo-write-token" placeholder="github_pat_…" now={p.now} clearSays={CLEAR_SAYS} onSave={k => setToken("write", k)} onClear={async () => { merge(await api.deleteRepoToken(qid, "write")); }} />
          <p class="hint" data-hosts>Sent to {hosts(link)}.</p>
          <p class="hint">A fine-grained token for this repository alone, with Contents: Read and write and Pull requests: Read and write. It stays on this daemon and never reaches a container.</p>
          <p class="hint" data-unsigned>Commits made with a token are unsigned, and a repository that requires signatures will refuse them; use the App.</p>
        </div>
        <div data-token="read">
          <span class="lbl">Read token</span>
          <KeyField info={tokens.read_token} label="Read token" name="clax-repo-read-token" placeholder="github_pat_…" now={p.now} clearSays={CLEAR_SAYS} onSave={k => setToken("read", k)} onClear={async () => { merge(await api.deleteRepoToken(qid, "read")); }} />
          <p class="hint" data-hosts>Sent to {hosts(link)}.</p>
          <p class="hint">A second token for this repository alone, with Contents: Read: the container clones with it, so a token that can write, pasted here, goes into the container. It must not be the write token.{#if !tokens.read_token.set} Without a read token only a public repository can be cloned.{/if}</p>
        </div>
        <p class="hint">Tokens are sealed and never shown again, and a pasted token works until it expires, so set an expiry when you make it. Changing the repository or either URL clears both tokens.</p>
      </div>
    {/if}
  {/if}
</section>

<style>
  .part { margin-top: 14px; padding-top: 12px; border-top: 1px solid var(--border); min-width: 0; }
  h4 { margin: 0 0 6px; font-size: 12.5px; font-weight: 600; color: var(--muted); text-transform: uppercase; letter-spacing: .04em; }
  .stat { margin: 0 0 4px; font-size: 14px; overflow-wrap: anywhere; }
  .rform { display: flex; flex-direction: column; gap: 10px; margin-top: 10px; min-width: 0; }
  .grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(220px, 1fr)); gap: 8px 12px; min-width: 0; }
  .field { display: flex; flex-direction: column; gap: 4px; font-size: 12.5px; color: var(--muted); min-width: 0; }
  .field input { width: 100%; min-width: 0; box-sizing: border-box; }
  .field textarea { width: 100%; max-width: 480px; box-sizing: border-box; font: 13px/1.5 var(--mono); color: var(--fg); background: var(--bg); border: 1px solid var(--border); border-radius: var(--radius-sm); padding: 6px 8px; }
  .how { display: flex; flex-wrap: wrap; gap: 4px 14px; margin: 0; padding: 0; border: 0; font-size: 13.5px; min-width: 0; }
  .how legend { padding: 0; margin-bottom: 4px; font-size: 12.5px; color: var(--muted); }
  .more { min-width: 0; }
  .more > summary { cursor: pointer; font-size: 13px; color: var(--muted); margin-bottom: 8px; }
  .more > .field, .check { margin-top: 8px; }
  .check { display: flex; gap: 6px; align-items: center; font-size: 13px; }
  .hint { margin: 6px 0 0; color: var(--muted); font-size: 12.5px; overflow-wrap: anywhere; }
  .hint > p { margin: 0 0 2px; }
  .err { margin: 6px 0 0; color: var(--danger); font-size: 13px; }
  .row { display: flex; flex-wrap: wrap; gap: 8px; }
  button.danger { color: var(--danger); }
  .tokens { display: grid; gap: 12px; margin-top: 12px; min-width: 0; }
  .lbl { display: block; margin-bottom: 4px; font-size: 12.5px; font-weight: 600; color: var(--muted); }
</style>
