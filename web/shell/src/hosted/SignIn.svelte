<svelte:options css="injected" />

<script lang="ts">
  // A hosted daemon's sign-in pages (crates/clax-server/src/hosted.rs), by
  // path: `/setup` makes the first person with the setup secret (prefilled
  // from `?secret=`, which leaves the address bar at once); `/join/<code>`
  // shows the name an invite gives and joins under it; `/signin/<code>`
  // names whose account a one-use link signs into and signs this browser
  // in with it. Both say who the browser is signed in as now, and that the
  // button signs them out of it: the click is the confirmation the server
  // asks before it replaces a session (`replace`). `/signin` says who the
  // browser is signed in as, or how to get in. A code is redeemed only by
  // the button, never by loading the page, so a link preview spends
  // nothing. Signed in, the browser goes to the gallery. The gallery's entry
  // loads this as a chunk of its own on these paths; the header's mark is
  // written out, as importing the gallery's `Mark` here would move it out
  // of the gallery's entry into a chunk both load.
  import { onMount, tick } from "svelte";
  import ThemeSwitch from "../ui/ThemeSwitch.svelte";
  import * as api from "./api";

  type Page = { kind: "setup" } | { kind: "join"; code: string } | { kind: "signin"; code: string } | { kind: "status" };
  const page: Page = (() => {
    const p = location.pathname;
    const code = (prefix: string) => decodeURIComponent(p.slice(prefix.length).replace(/\/+$/, ""));
    if (p === "/setup") return { kind: "setup" };
    if (p.startsWith("/join/")) return { kind: "join", code: code("/join/") };
    if (p.startsWith("/signin/") && p.length > "/signin/".length) return { kind: "signin", code: code("/signin/") };
    return { kind: "status" };
  })();

  let me = $state<api.HostedMe | null>(null);
  let invite = $state<{ name: string | null } | null | false>(null);
  let link = $state<{ public_id: string; name: string | null; owner: boolean } | null | false>(null);
  let name = $state("");
  let secret = $state("");
  let busy = $state(false);
  let err = $state<string | null>(null);
  const msg = (x: unknown) => (x instanceof Error ? x.message : String(x));

  const TITLE = { setup: "Set up Clax", join: "Join Clax", signin: "Sign in", status: "Sign in" } as const;

  async function run(fn: () => Promise<unknown>) {
    if (busy) return;
    busy = true; err = null;
    try { await fn(); location.assign("/"); } catch (x) { err = msg(x); busy = false; }
  }
  const submitSetup = (e: Event) => { e.preventDefault(); void run(() => api.setup(secret, name.trim())); };
  const current = $derived(me?.person ?? null);
  const join = () => run(() => page.kind === "join" ? api.acceptInvite(page.code, current !== null) : Promise.resolve());
  const signin = () => run(() => page.kind === "signin" ? api.acceptSignin(page.code, current !== null) : Promise.resolve());
  const linkName = $derived(link ? (link.name ?? "an unnamed person") : "");
  const currentName = $derived(current?.display_name ?? "an unnamed person");
  async function signOut() {
    busy = true;
    try { await api.signOut(); me = await api.me(); } catch (x) { err = msg(x); } finally { busy = false; }
  }

  onMount(async () => {
    document.title = `${TITLE[page.kind]} · Clax`;
    if (page.kind === "setup") {
      const q = new URLSearchParams(location.search);
      const s = q.get("secret");
      if (s !== null) { secret = s; history.replaceState(null, "", "/setup"); }
    }
    try { me = await api.me(); } catch (x) { err = msg(x); }
    if (page.kind === "join") {
      try { invite = await api.peekInvite(page.code); } catch { invite = false; }
    }
    if (page.kind === "signin") {
      try { link = await api.peekSignin(page.code); } catch { link = false; }
    }
    await tick();
    document.querySelector<HTMLElement>(".signin input, .signin button.primary")?.focus();
  });
</script>

<header class="gbar">
  <span class="mark" data-size="bar" role="img" aria-label="Clax"><svg class="mk" viewBox="0 0 30 24" aria-hidden="true" focusable="false"><path class="l" d="M2 2.5a9.5 9.5 0 0 1 0 19" fill="none" stroke-width="4.2"/><path class="r" d="M28 2.5a9.5 9.5 0 0 0 0 19" fill="none" stroke-width="4.2"/><circle cx="15" cy="12" r="2.6"/></svg></span><h1>Clax</h1><span class="gsp"></span><ThemeSwitch />
</header>
<main class="signin">
  <section class="sicard" aria-labelledby="si-h">
    <h2 id="si-h">{TITLE[page.kind]}</h2>
    {#if page.kind === "setup"}
      <p>You are the first person here. You will own this Clax: you invite people, share pages and approve what reaches your agents.</p>
      <form onsubmit={submitSetup}>
        <label class="field">Your name <input name="name" autocomplete="name" required maxlength="60" value={name} oninput={e => { name = e.currentTarget.value; }} /></label>
        <label class="field">Setup secret <input name="secret" type="password" autocomplete="off" required value={secret} oninput={e => { secret = e.currentTarget.value; }} /></label>
        <p class="hint">The secret the daemon was started with (its <code>CLAX_SETUP_SECRET</code>).</p>
        <button class="primary" type="submit" disabled={!name.trim() || !secret}>Create the first person</button>
      </form>
    {:else if page.kind === "join"}
      {#if invite === null}
        <p class="muted">Reading the invite…</p>
      {:else if invite === false}
        <p>This invite is not valid. It may have been used, or its day passed: ask for a new one.</p>
      {:else}
        <p>You have been invited as <strong>{invite.name ?? "a new person"}</strong>. Joining signs this browser in; you can sign in other devices from People.</p>
        {#if current}
          <p class="now" data-testid="signed-in-now">You are signed in here as <strong>{currentName}</strong>{current.guest ? " (a guest)" : current.owner ? ", the owner" : ""}. Joining signs you out of {currentName}.</p>
        {/if}
        <button class="primary" type="button" onclick={join} disabled={busy}>Join as {invite.name ?? "a new person"}</button>
      {/if}
    {:else if page.kind === "signin"}
      {#if link === null}
        <p class="muted">Reading the link…</p>
      {:else if link === false}
        <p>This sign-in link is not valid. It may have been used, or its ten minutes passed: make a new one on People, on a device that is signed in.</p>
        {#if current}<p class="hint">You are signed in here as <strong>{currentName}</strong>.</p>{/if}
      {:else if current && current.public_id === link.public_id}
        <p>This browser is already signed in as <strong>{linkName}</strong>. The link is not needed here.</p>
        <div class="acts"><a class="go" href="/">Open the gallery</a></div>
      {:else}
        <p>This link signs this browser in as <strong>{linkName}</strong>{link.owner ? ", the owner" : ""}, once. Everything you do here afterwards is done as {linkName}. Use it only if you made it yourself.</p>
        {#if current}
          <p class="now" data-testid="signed-in-now">You are signed in here as <strong>{currentName}</strong>{current.guest ? " (a guest)" : current.owner ? ", the owner" : ""}. Signing in as {linkName} signs you out of {currentName}.</p>
        {/if}
        <button class="primary" type="button" onclick={signin} disabled={busy}>Sign in as {linkName}</button>
      {/if}
    {:else if me?.person && !me.person.guest}
      <p>You are signed in as <strong>{me.person.display_name ?? "an unnamed person"}</strong>{me.person.owner ? ", the owner" : ""}.</p>
      <div class="acts"><a class="go" href="/">Open the gallery</a><button type="button" onclick={signOut} disabled={busy}>Sign out</button></div>
    {:else if me?.person}
      <p>You are a guest named <strong>{me.person.display_name}</strong>. Guests see only the pages shared with them by link, and may comment on those that allow it.</p>
      <div class="acts"><button type="button" onclick={signOut} disabled={busy}>Sign out</button></div>
    {:else}
      <p>You are not signed in. Ask someone here for an invite, or, on a device that is signed in, make a sign-in link on People.</p>
      {#if me?.setup_open}<p class="hint">Setting up this Clax? Open <a href="/setup">/setup</a> with its secret.</p>{/if}
    {/if}
    {#if err}<p class="err" role="alert">{err}</p>{/if}
  </section>
</main>

<style>
  :global {
    .gbar .gsp { flex: 1; }
    .signin { max-width: 480px; margin: 0 auto; padding: 48px var(--gutter) 40px; }
    .signin .sicard { background: var(--card); border: 1px solid var(--border); border-radius: var(--radius); padding: 22px 24px 24px; display: flex; flex-direction: column; gap: 12px; }
    .signin h2 { margin: 0; font-size: 22px; line-height: 1.2; }
    .signin p { margin: 0; font-size: 14.5px; line-height: 1.5; overflow-wrap: anywhere; }
    .signin form { display: flex; flex-direction: column; gap: 12px; margin: 0; }
    .signin .field { display: flex; flex-direction: column; gap: 4px; font-size: 12.5px; color: var(--muted); }
    .signin .field input { width: 100%; box-sizing: border-box; }
    .signin .hint, .signin .muted { color: var(--muted); font-size: 13px; }
    .signin code { font: 12.5px var(--mono); }
    .signin button.primary { align-self: flex-start; white-space: normal; text-align: start; }
    .signin .acts { display: flex; flex-wrap: wrap; align-items: center; gap: 10px 14px; }
    .signin a { color: var(--fg); }
    .signin .err { color: var(--danger); font-size: 13.5px; }
    .signin .now { border-inline-start: 3px solid var(--accent); padding: 6px 10px; background: var(--bg); border-radius: 4px; }
    @media (max-width: 520px) {
      .signin { padding-top: 24px; }
      .signin .sicard { padding: 18px 16px 20px; }
      .signin button.primary { align-self: stretch; justify-content: center; }
    }
  }
</style>
