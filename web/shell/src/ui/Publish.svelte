<svelte:options css="injected" />

<script lang="ts">
  // The top bar's Publish button (docs/contract.md "Publishing to a public
  // Clax"): in the owner's shell on a local daemon signed in to a public Clax
  // (`clax remote add`), it publishes the artifact there, and once it is
  // public, links to its public copy and pushes newer versions (Push v<N>).
  // Pressing it opens a confirmation naming where it goes and as whom, with
  // a choice to let agents push new versions without asking; when someone
  // published on the public copy since, it offers a fork of the public copy
  // or a new public artifact instead. Signed in to none, it reads Publish to
  // clax.dev and signs this Clax in there first (the device sign-in of
  // `clax remote add`): the tab goes to the approval page, which sends it
  // back here with `?publish=1&clax_signin=approved|denied`; on that return
  // the button polls the sign-in once and asks to publish, or says it was
  // declined. When the daemon could not pass this page on, the dialog shows
  // the code and a link to the approval page and polls until it is
  // approved. Anywhere else (a
  // hosted daemon, another viewer) it shows nothing. At phone width the buttons are
  // hidden and the more menu lists Publish (`setPublishOpener`). Loaded after
  // the first paint (`more-menu.svelte.ts`).
  import { onMount } from "svelte";
  import { getToken } from "../api";
  import { nav } from "../nav";
  import { setUrl } from "../view/url";
  import { DEFAULT_ORIGIN, dropUnsent, hostOf, listRemotes, pollSignIn, push, remotesOf, retryLink, setDownloadOnly, startSignIn, unlink, type Link, type Mode, type Remote, type Remotes } from "./publish-api";
  import { notSentLabel, sinceLabel } from "./link-model";
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
  // The link's comments (linked threads): a confirmation waiting, and a change under way.
  let confirm = $state<"unlink" | "drop" | null>(null);
  let linkBusy = $state(false);
  // A sign-in waiting for the person's approval on the public Clax, and
  // which run of the wait is current (closing or a new sign-in ends it).
  let signing = $state<{ code: string; url: string } | null>(null);
  let run = 0;

  const link = $derived<Link | null>(info?.links[0] ?? null);
  const remote = $derived<Remote | null>(
    (link && info?.remotes.find(r => r.id === link.remote_id)) || info?.remotes[0] || null,
  );
  const host = $derived(hostOf(remote?.origin ?? link?.origin ?? DEFAULT_ORIGIN));
  const behind = $derived(link ? Math.max(0, latest - link.pushed_n) : 0);
  const action = $derived(link ? `Push v${latest}` : `Publish to ${host}`);
  // The owner's shell on a local daemon, whose remotes answered.
  const usable = $derived(info !== null);
  const unsent = $derived(info?.unsent ?? []);
  const pending = $derived(link?.pending ?? 0);
  const failed = $derived(link?.failed ?? 0);
  const failedQuestions = $derived(link?.failed_questions ?? 0);
  const since = $derived(link?.sync?.unreachable_since ?? null);
  const warn = $derived(!!since || failed > 0 || failedQuestions > 0);

  async function refresh() {
    const t = await getToken();
    // The owner's shell on a local daemon: a hosted owner's token is `cxo_`.
    if (!t || t.startsWith("cxo_")) return;
    try { info = await remotesOf(id); } catch { info = null; }
  }

  function show() { err = null; moved = false; done = null; confirm = null; follow = link?.auto ?? false; open = true; void refresh(); }

  /** Runs a change to the link's comments, then reads the link again. */
  async function change(f: () => Promise<unknown>) {
    if (linkBusy) return;
    linkBusy = true; err = null;
    try { await f(); confirm = null; await refresh(); } catch (e) { err = (e as { message?: string }).message ?? String(e); } finally { linkBusy = false; }
  }
  function close() { open = false; run++; signing = null; }

  const declined = () => `The sign-in to ${host} was declined.`;

  /** Polls remote `rid`'s sign-in, at once and then every `interval`
   * seconds, until it is decided: connected, it reads the remotes again;
   * else it says why. Run `mine` of the wait. */
  async function wait(rid: string, interval: number, mine: number) {
    for (;;) {
      const p = await pollSignIn(rid);
      if (mine !== run) return;
      if (p.status === "connected") { signing = null; await refresh(); return; }
      if (p.status !== "pending") {
        signing = null;
        err = p.status === "denied" ? declined() : `The sign-in to ${host} expired before it was approved.`;
        return;
      }
      await new Promise(r => setTimeout(r, Math.max(1, interval) * 1000));
      if (mine !== run) return;
    }
  }

  /** Signs this Clax in to the default public Clax: this tab goes to its
   * approval page, which sends it back to this page with `publish=1`; when
   * the daemon could not pass the page on, the dialog waits for the approval
   * here, then reads the remotes again. */
  async function signIn() {
    if (busy) return;
    busy = true; err = null;
    const mine = ++run;
    try {
      const q = new URLSearchParams(location.search);
      q.set("publish", "1");
      const s = await startSignIn(DEFAULT_ORIGIN, `${location.pathname}?${q}`);
      if (mine !== run) return;
      if (s.return_to_kept) { nav.assign(s.verification_url); return; }
      signing = { code: s.user_code, url: s.verification_url };
      await wait(s.remote.id, s.interval, mine);
    } catch (e) {
      if (mine === run) { signing = null; err = (e as { message?: string }).message ?? String(e); }
    } finally { busy = false; }
  }

  /** Back from the approval page (`?publish=1&clax_signin=approved|denied`):
   * drops both from the address and opens the dialog. Approved, it polls
   * the sign-in waiting for clax.dev once (and on, should it still wait);
   * denied, it says so. With no sign-in waiting (expired, or signed in
   * meanwhile), the dialog opens as it is. */
  async function cameBack(how: string | null) {
    const q = new URLSearchParams(location.search);
    q.delete("publish"); q.delete("clax_signin");
    const rest = q.toString();
    setUrl(location.pathname + (rest ? `?${rest}` : "") + location.hash);
    await refresh();
    if (!info) return;
    err = null; moved = false; done = null; confirm = null; follow = link?.auto ?? false; open = true;
    if (info.remotes.length) return;
    const mine = ++run;
    busy = true;
    try {
      const waiting = (await listRemotes()).find(r => r.status === "pending" && r.origin === DEFAULT_ORIGIN);
      if (mine !== run) return;
      if (how === "denied") {
        err = declined();
        // Its poll answers denied, and the daemon forgets it.
        if (waiting) await pollSignIn(waiting.id).catch(() => {});
        return;
      }
      if (how !== "approved" || !waiting) return;
      signing = { code: waiting.user_code ?? "", url: waiting.verification_url ?? "" };
      await wait(waiting.id, waiting.poll_interval_s ?? 5, mine);
    } catch (e) {
      if (mine === run) { signing = null; err = (e as { message?: string }).message ?? String(e); }
    } finally { busy = false; }
  }

  onMount(() => {
    const q = new URLSearchParams(location.search);
    if (q.get("publish") === "1") void cameBack(q.get("clax_signin"));
  });

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

  $effect(() => () => { run++; });
  $effect(() => {
    // A newer version refreshes what is behind.
    void latest;
    void refresh();
  });
  $effect(() => {
    setPublishOpener(usable ? { label: !link || behind > 0 ? `${action}…` : `Link to ${host}…`, open: show } : null);
    return () => setPublishOpener(null);
  });
</script>

<svelte:window onpointerdown={outside} />

{#if usable}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <span class="pub" bind:this={root} onkeydown={key}>
    {#if !link || behind > 0}
      <button type="button" class="publish hide-sm" data-shell-control="publish" aria-haspopup="dialog" aria-expanded={open} onclick={() => (open ? close() : show())}>{action}</button>
    {/if}
    {#if link && behind === 0}
      <button type="button" class="linked hide-sm" data-shell-control="publish" data-warn={warn ? "" : undefined} aria-haspopup="dialog" aria-expanded={open} title={`Its link to ${host}: versions and comments`} onclick={() => (open ? close() : show())}>Linked{#if warn}<span class="warn-dot" aria-label={since ? `${host} unreachable` : `${notSentLabel(failed, failedQuestions)} not sent`}></span>{/if}</button>
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
        {:else if !remote}
          {#if signing}
            <p>Approve this Clax on {host}: check that the page there shows <b class="pub-code">{signing.code}</b>.</p>
            <p class="muted small">Waiting for your approval… <a href={signing.url} target="_blank" rel="noopener noreferrer">Open the approval page</a></p>
            <div class="pub-acts"><button type="button" onclick={close}>Cancel</button></div>
          {:else}
            <p>Publish “{title}” on {host}. First sign this Clax in to {host} with your account there: its approval page brings you back here.</p>
            {#if err}<p class="err" role="alert">{err}</p>{/if}
            <div class="pub-acts">
              <button type="button" class="primary" disabled={busy} onclick={signIn}>Sign in to {host}</button>
              <button type="button" onclick={close}>Cancel</button>
            </div>
          {/if}
        {:else}
          {#if !link || behind > 0}
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
          {#if link}
            <section class="pub-link" aria-label="Comments">
              <p class="pub-sec">Comments with {host}</p>
              <div class="seg" role="group" aria-label="Comments with {host}">
                <button type="button" aria-pressed={!link.download_only} disabled={linkBusy} onclick={() => link.download_only && change(() => setDownloadOnly(id, link.remote_id, false))}>Two-way</button>
                <button type="button" aria-pressed={!!link.download_only} disabled={linkBusy} onclick={() => !link.download_only && change(() => setDownloadOnly(id, link.remote_id, true))}>Download only</button>
              </div>
              <p class="muted small">{link.download_only ? `Threads from ${host} come here; threads started here stay here. Replies and resolves on its threads still go up.` : `Threads and replies go both ways.`}</p>
              {#if since}<p class="err small" role="status">{host} unreachable since {sinceLabel(since)}</p>{/if}
              {#if pending > 0}<p class="muted small">Sending {pending === 1 ? "1 comment" : `${pending} comments`} to {host}…</p>{/if}
              {#if failed > 0 || failedQuestions > 0}
                <p class="err small row">Not sent: {notSentLabel(failed, failedQuestions)}<button type="button" disabled={linkBusy} onclick={() => change(() => retryLink(id, link.remote_id))}>Retry</button></p>
              {/if}
              {#if confirm === "unlink"}
                <p class="small">Unlink “{title}” from {host}? Its public copy stays there; its threads stay here, read-only.</p>
                <div class="pub-acts">
                  <button type="button" class="danger" disabled={linkBusy} onclick={() => change(() => unlink(id, link.remote_id))}>Unlink</button>
                  <button type="button" onclick={() => (confirm = null)}>Cancel</button>
                </div>
              {:else}
                <div class="pub-acts">
                  {#if behind === 0}<a class="go" href={link.url} target="_blank" rel="noopener noreferrer">Open the public page</a>{/if}
                  <button type="button" class="quiet" onclick={() => (confirm = "unlink")}>Unlink…</button>
                </div>
              {/if}
            </section>
          {/if}
          {#if unsent.length}
            <section class="pub-link" aria-label="Unsent comments">
              <p class="pub-sec">Not sent before the link ended</p>
              <ul class="unsent">
                {#each unsent as u (u.seq)}<li>{u.body ?? (u.kind === "resolve" ? "A resolve" : u.kind === "reopen" ? "A reopen" : "A comment")}</li>{/each}
              </ul>
              {#if confirm === "drop"}
                <p class="small">Drop {unsent.length === 1 ? "it" : `these ${unsent.length}`}? They stay in their threads here, but never go up.</p>
                <div class="pub-acts">
                  <button type="button" class="danger" disabled={linkBusy} onclick={() => change(() => dropUnsent(id))}>Drop them</button>
                  <button type="button" onclick={() => (confirm = null)}>Cancel</button>
                </div>
              {:else}
                <div class="pub-acts"><button type="button" class="quiet" onclick={() => (confirm = "drop")}>Drop them…</button></div>
              {/if}
            </section>
          {/if}
          {#if link && err && behind === 0}<p class="err" role="alert">{err}</p>{/if}
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
  .pub-code { font: 600 15px var(--mono); letter-spacing: .04em; }
  .pub-follow { display: flex; align-items: center; gap: 8px; font-size: 13.5px; color: var(--muted); }
  .pub-acts { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; }
  .pub-acts .go { font: 600 14px var(--font); color: var(--accent-ink); margin-right: auto; }
  .err { color: var(--danger); }
  .linked { position: relative; white-space: nowrap; }
  .warn-dot { display: inline-block; width: 7px; height: 7px; margin-left: 6px; border-radius: 50%; background: var(--danger); vertical-align: 1px; }
  .pub-link { display: flex; flex-direction: column; gap: 8px; padding-top: 10px; border-top: 1px solid var(--border); }
  .pub-link:first-child { padding-top: 0; border-top: 0; }
  .pub-sec { font-weight: 600; }
  .pub-pop .small { font-size: 13px; }
  .pub-pop .muted { color: var(--muted); }
  .pub-pop .row { display: flex; align-items: center; gap: 8px; justify-content: space-between; }
  .seg { display: inline-flex; align-self: flex-start; border: 1px solid var(--border-hover); border-radius: var(--radius-sm); overflow: hidden; }
  .seg button { border: 0; border-radius: 0; min-height: 30px; padding: 4px 12px; background: transparent; }
  .seg button + button { border-left: 1px solid var(--border-hover); }
  .seg button[aria-pressed="true"] { background: var(--accent-tint); color: var(--accent-ink); font-weight: 600; }
  .unsent { margin: 0; padding-left: 18px; font-size: 13px; color: var(--muted); }
  .unsent li { overflow-wrap: anywhere; }
  .pub-acts .danger { color: var(--danger); border-color: var(--danger); }
  .pub-acts .quiet:first-child { margin-left: auto; }
  @media (max-width: 700px) {
    .pub { position: static; }
    .pub-pop { position: fixed; left: 16px; right: 16px; top: 64px; width: auto; }
  }
</style>
