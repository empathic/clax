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
  // browser is signed in as, or how to get in; `/device[?code=]` approves
  // another machine's Clax signing in as this browser's person (a device
  // code it showed), naming the machine and the address it asked from, or
  // denies it (signed out, it offers GitHub sign-in where the instance has
  // it, returning to `/device?code=<code>`); when the sign-in names itself the Clax app (`client_kind`
  // `clax-app`), it says the app may also manage the sharing of their apps. A code is redeemed only by
  // the button, never by loading the page, so a link preview spends
  // nothing. Where the instance shows Terms of Service and a Privacy
  // Policy (`me.legal`), each button that signs this browser in says that
  // doing so agrees to them, the server records it, and the page ends
  // with links to both. Every one ends with a link to Clax's source on
  // GitHub, and the header's mark links to the gallery. Signed in, the
  // browser goes to the gallery, or to `?next=` when it is a path on this
  // origin (`./next`): `/site-signin` sends a signed-out visitor of a
  // private site to `/signin?next=<its address>`, so a sign-in returns to
  // the site. The GitHub button carries it through the OAuth flow, and
  // `/signin?next=` in a browser already signed in (by a link opened in
  // another tab, say) goes on there at once (the daemon redirects it,
  // crates/clax-server/src/hosted.rs `signin_page`; the page goes on
  // itself where it was served anyway). Signed out, `/signin`
  // also lists the public gallery ("Public on Clax"), each artifact opening
  // through its link. The gallery's entry
  // loads this as a chunk of its own on these paths; the header's mark is
  // written out, as importing the gallery's `Mark` here would move it out
  // of the gallery's entry into a chunk both load.
  //
  // The page renders once, in its final shape: what it reads first (who
  // the browser is, the public gallery, the invite, link or device it
  // names) comes from the daemon's block where it answered (`boot`,
  // `./signin-boot`), and anything else is asked at mount, the card
  // showing the entry's frame's skeleton (`SIGNIN_SKELETON_HTML`,
  // `view/gallery-frame.ts`) until every answer is in. Its styles are the
  // gallery entry's (`gallery.css`), which the frame shares. A pending
  // action keeps its control in place, busy (`aria-busy`, a turning ring
  // where its label was) and disabled, and says so to assistive
  // technology; a GitHub sign-in that comes back through history is no
  // longer pending.
  import { onMount, tick } from "svelte";
  import ThemeSwitch from "../ui/ThemeSwitch.svelte";
  import * as api from "./api";
  import { forgetThenSignOut } from "./signout";
  import { refusal } from "../people/refusal";
  import { nextOfPage, loopbackReturn } from "./next";
  import { nav } from "../nav";
  import { SIGNIN_SKELETON_HTML } from "../view/gallery-frame";
  import { type SignInBoot, type SignInField, statusOf } from "./signin-boot";

  type Page = { kind: "setup" } | { kind: "join"; code: string } | { kind: "signin"; code: string } | { kind: "device" } | { kind: "status" };
  const page: Page = (() => {
    const p = location.pathname;
    const code = (prefix: string) => decodeURIComponent(p.slice(prefix.length).replace(/\/+$/, ""));
    if (p === "/setup") return { kind: "setup" };
    if (p === "/device") return { kind: "device" };
    if (p.startsWith("/join/")) return { kind: "join", code: code("/join/") };
    if (p.startsWith("/signin/") && p.length > "/signin/".length) return { kind: "signin", code: code("/signin/") };
    return { kind: "status" };
  })();

  let { boot: given = null }: { boot?: SignInBoot | null } = $props();
  // The block is read once, for the first render.
  // svelte-ignore state_referenced_locally
  const boot = given;
  // The block's answer for `f`: its body when its route answered 200,
  // `refused` for any other answer, undefined (ask the route) for none.
  const fromBlock = <T, R>(f: SignInField, body: T | null, refused: R): T | R | undefined => {
    const st = statusOf(boot, f);
    return st === null ? undefined : st === 200 && body !== null ? body : refused;
  };
  const meIn = statusOf(boot, "hosted") === 200 && boot?.hosted ? boot.hosted : undefined;
  const publicIn = fromBlock("public", boot?.public?.artifacts ?? null, [] as api.PublicArtifact[]);
  const inviteIn = fromBlock("invite", boot?.invite ?? null, false as const);
  const linkIn = fromBlock("link", boot?.link ?? null, false as const);
  // A device's code the block looked up, or that the route does not know (404, as `lookUp` reads it).
  const deviceIn = statusOf(boot, "device") === 404 ? (false as const) : statusOf(boot, "device") === 200 ? boot?.device ?? undefined : undefined;

  let me = $state<api.HostedMe | null>(meIn ?? null);
  let publicList = $state<api.PublicArtifact[]>(publicIn ?? []);
  let invite = $state<{ name: string | null } | null | false>(inviteIn ?? null);
  let link = $state<{ public_id: string; name: string | null; owner: boolean } | null | false>(linkIn ?? null);
  let name = $state("");
  // The setup secret, from `?secret=` (which leaves the address bar at mount).
  let secret = $state(page.kind === "setup" ? new URLSearchParams(location.search).get("secret") ?? "" : "");
  let busy = $state(false);
  // The control whose action is pending (its ring), while `busy`; the GitHub link's until the page is left.
  let pressed = $state<string | null>(null);
  let err = $state<string | null>(null);
  const msg = refusal;
  const next = nextOfPage();
  const github = next === null ? "/auth/github" : `/auth/github?next=${encodeURIComponent(next)}`;

  const TITLE = { setup: "Set up Clax", join: "Join Clax", signin: "Sign in", device: "Connect a Clax", status: "Sign in" } as const;

  // The device sign-in: the code as typed (from `?code=`, which leaves the
  // address bar), the machine it names once looked up, and the outcome.
  let deviceCode = $state(page.kind === "device" ? new URLSearchParams(location.search).get("code") ?? "" : "");
  let device = $state<api.DeviceRequest | null | false>(deviceIn ?? null);
  let decided = $state<"approved" | "denied" | null>(null);
  const isApp = $derived(device ? device.client_kind === "clax-app" : false);
  // Where the machine's Clax asked to be sent back to: its own loopback page.
  const back = $derived(device ? loopbackReturn(device.return_to) : null);
  // GitHub sign-in from the device page returns to it with its code
  // (`/device?code=<code>`, a path `safeNext` and the server's `safe_next`
  // take), so a person new here signs in and comes back to approve.
  const deviceGithub = $derived(`/auth/github?next=${encodeURIComponent(deviceCode.trim() ? `/device?code=${encodeURIComponent(deviceCode.trim())}` : "/device")}`);
  async function lookUp(e?: Event) {
    e?.preventDefault();
    if (busy || !deviceCode.trim()) return;
    busy = true; pressed = "continue"; err = null;
    try { device = await api.peekDevice(deviceCode.trim()); } catch (x) { device = false; if (!(x instanceof api.Refused && x.status === 404)) err = msg(x); } finally { busy = false; pressed = null; }
  }
  async function decide(approve: boolean) {
    if (busy) return;
    busy = true; pressed = approve ? "approve" : "deny"; err = null;
    try {
      await (approve ? api.approveDevice(deviceCode.trim()) : api.denyDevice(deviceCode.trim()));
      decided = approve ? "approved" : "denied";
      if (back) {
        const u = new URL(back);
        u.searchParams.set("clax_signin", decided);
        nav.assign(u.href);
      }
    } catch (x) { err = msg(x); } finally { busy = false; pressed = null; }
  }

  // Signed in, the browser goes on: the control stays busy until the page is left.
  async function run(fn: () => Promise<unknown>, control: string) {
    if (busy) return;
    busy = true; pressed = control; err = null;
    try { await fn(); location.assign(next ?? "/"); } catch (x) { err = msg(x); busy = false; pressed = null; }
  }
  const submitSetup = (e: Event) => { e.preventDefault(); void run(() => api.setup(secret, name.trim()), "setup"); };
  const current = $derived(me?.person ?? null);
  const join = () => run(() => page.kind === "join" ? api.acceptInvite(page.code, current !== null) : Promise.resolve(), "join");
  const signin = () => run(() => page.kind === "signin" ? api.acceptSignin(page.code, current !== null) : Promise.resolve(), "signin");
  const linkName = $derived(link ? (link.name ?? "an unnamed person") : "");
  const currentName = $derived(current?.display_name ?? "an unnamed person");
  const legal = $derived(me?.legal ?? null);
  async function signOut() {
    busy = true; pressed = "signout";
    try { await forgetThenSignOut(api.signOut); me = await api.me(); } catch (x) { err = msg(x); } finally { busy = false; pressed = null; }
  }
  // The GitHub link leaves the page: pending until it does, and no longer
  // when the page comes back from history.
  const leaving = (e: MouseEvent) => {
    if (e.defaultPrevented || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
    if (pressed === "github") { e.preventDefault(); return; }
    pressed = "github";
  };
  const returned = (e: PageTransitionEvent) => { if (e.persisted && pressed === "github") pressed = null; };
  // What the busy control says to assistive technology.
  const SAYS: Record<string, string> = { setup: "Setting up…", join: "Joining…", signin: "Signing in…", github: "Signing in with GitHub…", continue: "Looking up the code…", approve: "Approving…", deny: "Denying…", signout: "Signing out…" };

  // Whether the first render has every answer it reads: it shows the
  // skeleton until then (only where the block left one to ask).
  const needsDevice = () => page.kind === "device" && !!deviceCode.trim() && !!me?.person && !me.person.guest && device === null;
  const codeOf = () => page.kind === "device" ? new URLSearchParams(location.search).get("code") ?? "" : "";
  let ready = $state(meIn !== undefined
    && !(page.kind === "status" && meIn.person && next !== null)
    && (page.kind !== "status" || !!meIn.person || publicIn !== undefined)
    && (page.kind !== "join" || inviteIn !== undefined)
    && (page.kind !== "signin" || linkIn !== undefined)
    && !(page.kind === "device" && codeOf().trim() && meIn.person && !meIn.person.guest && deviceIn === undefined));

  onMount(async () => {
    document.title = `${TITLE[page.kind]} · Clax`;
    if (page.kind === "setup" && new URLSearchParams(location.search).has("secret")) history.replaceState(null, "", "/setup");
    if (page.kind === "device" && new URLSearchParams(location.search).has("code")) history.replaceState(null, "", "/device");
    if (meIn === undefined) { try { me = await api.me(); } catch (x) { err = msg(x); } }
    if (page.kind === "status" && me?.person && next !== null) { location.replace(next); return; }
    if (page.kind === "status" && !me?.person && publicIn === undefined) {
      try { publicList = (await api.publicArtifacts()).artifacts; } catch { publicList = []; }
    }
    if (needsDevice()) await lookUp();
    if (page.kind === "join" && inviteIn === undefined) {
      try { invite = await api.peekInvite(page.code); } catch { invite = false; }
    }
    if (page.kind === "signin" && linkIn === undefined) {
      try { link = await api.peekSignin(page.code); } catch { link = false; }
    }
    ready = true;
    await tick();
    document.querySelector<HTMLElement>(".signin input, .signin button.primary")?.focus();
  });
</script>

<svelte:window onpageshow={returned} />

<header class="gbar">
  <a href="/" class="home" aria-label="Gallery"><span class="mark" data-size="bar" role="img" aria-label="Clax"><svg class="mk" viewBox="0 0 40 24" aria-hidden="true" focusable="false"><path class="l" d="M2 2.5a9.5 9.5 0 0 1 0 19" fill="none" stroke-width="4.2"/><path class="r" d="M38 2.5a9.5 9.5 0 0 0 0 19" fill="none" stroke-width="4.2"/><circle cx="20" cy="12" r="2.6"/></svg></span></a><h1>Clax</h1><span class="gsp"></span><ThemeSwitch />
</header>
<main class="signin" aria-busy={ready ? undefined : "true"}>
  <section class="sicard" aria-labelledby="si-h">
    <h2 id="si-h">{TITLE[page.kind]}</h2>
    {#if !ready}
      {@html SIGNIN_SKELETON_HTML}
    {:else if page.kind === "setup"}
      <p>You are the first person here. You will own this Clax: you invite people, share pages and approve what reaches your agents.</p>
      <form onsubmit={submitSetup}>
        <label class="field">Your name <input name="name" autocomplete="name" required maxlength="60" value={name} oninput={e => { name = e.currentTarget.value; }} /></label>
        <label class="field">Setup secret <input name="secret" type="password" autocomplete="off" required value={secret} oninput={e => { secret = e.currentTarget.value; }} /></label>
        <p class="hint">The secret the daemon was started with (its <code>CLAX_SETUP_SECRET</code>).</p>
        <button class="primary busy-able" type="submit" disabled={busy || !name.trim() || !secret} aria-busy={pressed === "setup"}>Create the first person</button>
        {@render agree("setting up this Clax")}
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
        <button class="primary busy-able" type="button" onclick={join} disabled={busy} aria-busy={pressed === "join"}>Join as {invite.name ?? "a new person"}</button>
        {@render agree("joining")}
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
        <button class="primary busy-able" type="button" onclick={signin} disabled={busy} aria-busy={pressed === "signin"}>Sign in as {linkName}</button>
        {@render agree("signing in")}
      {/if}
    {:else if page.kind === "device"}
      {#if me === null}
        <!-- Only when asking failed: the alert below says why. -->
      {:else if !me.person || me.person.guest}
        {#if me.github}
          <p>Another Clax asks to publish here as you. Sign in first, and this page comes back to approve it.</p>
          {@render githubButton(deviceGithub)}
          {@render agree("signing in")}
          <p class="hint">Or open a sign-in link from a device that is signed in, then come back to this page.</p>
        {:else}
          <p>Another Clax asks to publish here as you. Sign in first: open a sign-in link from a device that is signed in, then come back to this page.</p>
        {/if}
      {:else if decided === "approved"}
        <p><strong>{device ? device.client_name : "That Clax"}</strong> is signed in as you. It shows in Settings under {isApp ? "Account, Devices" : "Agents & machines, Agent tokens"}, where you can revoke it.{back ? " Going back to it…" : " You can close this tab."}</p>
        <div class="acts"><a class="go" href={isApp ? "/settings/account#hd-h" : "/settings/agents#ht-h"}>Open Settings</a></div>
      {:else if decided === "denied"}
        <p>Denied: that Clax is not signed in.{back ? " Going back to it…" : " You can close this tab."}</p>
      {:else if device}
        <p><strong>{device.client_name}</strong>{isApp ? ", the Clax app," : ","} from {device.peer_ip || "an unknown address"}, asks to publish here as you, <strong>{me.person.display_name ?? "you"}</strong>. Check that it shows the code <code>{deviceCode.trim().toUpperCase()}</code>.</p>
        {#if !device.same_address}<p class="now" data-testid="other-address">It asked from another address than this browser's. Approve only if you started this sign-in yourself, just now, on that machine: anyone who sent you this link would publish as you.</p>{/if}
        <p class="hint" data-testid={isApp ? "device-app" : undefined}>It gets an agent token of yours: {isApp ? "what it publishes is yours, within your rights, and an app it publishes stays private until you share it." : "what it publishes is public as yours, within your rights."}{#if isApp}{" "}As the Clax app, it may also change who can see or edit the pages you made{me.person.owner ? " and, as the owner, any page on this site" : ""}, and whether anyone may comment on them.{/if} It shows on People{isApp ? " among your devices" : ""}, where you can revoke it.{#if legal}{" "}<span class="legal">What it does here is yours under the <a href={legal.terms_url} target="_blank" rel="noopener">Terms of Service</a>.</span>{/if}</p>
        <div class="acts"><button class="primary busy-able" type="button" disabled={busy} aria-busy={pressed === "approve"} onclick={() => decide(true)}>Approve</button><button class="busy-able" type="button" disabled={busy} aria-busy={pressed === "deny"} onclick={() => decide(false)}>Deny</button></div>
        {#if back}<p class="hint" data-testid="device-return">Approve, then go back to {new URL(back).host}.</p>{/if}
      {:else}
        <form onsubmit={lookUp}>
          {#if device === false}<p>No Clax is waiting with that code. It may have expired (codes last ten minutes) or been used: start again on that machine.</p>{/if}
          <label class="field">The code your Clax shows <input name="code" autocomplete="off" required maxlength="12" placeholder="WXYZ-4821" value={deviceCode} oninput={e => { deviceCode = e.currentTarget.value; }} /></label>
          <button class="primary busy-able" type="submit" disabled={busy || !deviceCode.trim()} aria-busy={pressed === "continue"}>Continue</button>
        </form>
      {/if}
    {:else if me?.person && !me.person.guest}
      <p>You are signed in as <strong>{me.person.display_name ?? "an unnamed person"}</strong>{me.person.owner ? ", the owner" : ""}.</p>
      <div class="acts"><a class="go" href="/">Open the gallery</a><button class="busy-able" type="button" onclick={signOut} disabled={busy} aria-busy={pressed === "signout"}>Sign out</button></div>
    {:else if me?.person}
      <p>You are a guest named <strong>{me.person.display_name}</strong>. Guests see only the pages shared with them by link, and may comment on those that allow it.</p>
      <div class="acts"><button class="busy-able" type="button" onclick={signOut} disabled={busy} aria-busy={pressed === "signout"}>Sign out</button></div>
    {:else}
      {#if me?.github}
        <p>Sign in with your GitHub account. New here? Signing in makes your own space: publish your pages, connect your agents, and see what others share with you.</p>
        {@render githubButton(github)}
        {@render agree("signing in")}
        <p class="hint">Or ask someone here for an invite, or, on a device that is signed in, make a sign-in link on People.</p>
      {:else}
        <p>You are not signed in. Ask someone here for an invite, or, on a device that is signed in, make a sign-in link on People.</p>
      {/if}
      {#if me?.setup_open}<p class="hint">Setting up this Clax? Open <a href="/setup">/setup</a> with its secret.</p>{/if}
    {/if}
    {#if ready}<div class="vh" role="status">{pressed ? SAYS[pressed] : ""}</div>{/if}
    {#if err}<p class="err" role="alert">{err}</p>{/if}
  </section>
  {#snippet githubButton(href: string)}
    <a class="gh primary busy-able" href={href} data-testid="github-signin" aria-busy={pressed === "github"} onclick={leaving}><svg viewBox="0 0 16 16" width="18" height="18" aria-hidden="true" focusable="false"><path fill="currentColor" d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0016 8c0-4.42-3.58-8-8-8z"/></svg>Sign in with GitHub</a>
  {/snippet}
  {#snippet agree(doing: string)}
    {#if legal}<p class="hint legal" data-testid="legal-notice">By {doing}, you agree to the <a href={legal.terms_url} target="_blank" rel="noopener">Terms of Service</a> and the <a href={legal.privacy_url} target="_blank" rel="noopener">Privacy Policy</a>.</p>{/if}
  {/snippet}
  {#if ready && page.kind === "status" && !me?.person && publicList.length}
    <section class="sicard pub" aria-labelledby="pub-h">
      <h2 id="pub-h">Public on Clax</h2>
      <ul>
        {#each publicList as a (a.artifact_id)}<li><a href={`/a/${a.artifact_id}`}>{a.title}</a></li>{/each}
      </ul>
    </section>
  {/if}
  <footer class="silegal">{#if ready}{#if legal}<a href={legal.terms_url}>Terms of Service</a>{" · "}<a href={legal.privacy_url}>Privacy Policy</a>{" · "}{/if}<a href="https://github.com/empathic/clax" target="_blank" rel="noopener">Source on GitHub</a>{/if}</footer>
</main>

