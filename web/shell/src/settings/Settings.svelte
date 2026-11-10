<svelte:options css="injected" />

<script lang="ts">
  // Settings (`/settings`, `/settings/<screen>`): everything a person sets
  // about themselves here and, for the owner and admins, about this Clax,
  // as one list of screens (`screens.ts`) in four groups: You, People,
  // ClaxBot and System. `/settings` is the overview: the search field, and
  // every screen this browser sees with a line saying what is on it. A
  // screen's address shows it beside the navigation (at phone width, under
  // a link back to the overview). Each screen is its own chunk, the page
  // that was `/people`, `/admin`, `/me` or `/audit` before, or a part of
  // one. A screen this browser may not see shows the one it names instead,
  // else the overview with a line saying so.
  //
  // Moving between screens is in place, through the browser's history: a
  // link to a Settings address anywhere on the page, a search result, and
  // back and forward. An address ending `#<ID>` scrolls to that heading
  // and focuses it once the screen has drawn it.
  //
  // On a hosted Clax, Sign out ends the navigation and the overview, for
  // every member on every screen: it forgets the person's reporter, ends
  // the session and lands on `/signin`; a refusal shows beside it.
  import { onDestroy, onMount, tick, type Component } from "svelte";
  import { refusal } from "../people/refusal";
  import type { EventStream } from "../stream";
  import { context } from "./context";
  import { GO, openPalette } from "./palette";
  import { type Ctx, type Screen, groupsFor, idOf, screenFor } from "./screens";
  import Search from "./Search.svelte";

  let { stream }: { stream?: EventStream } = $props();
  type Body = Component<{ stream?: EventStream; part?: string; tab?: string }>;
  const people = (part: string) => ({ load: () => import("../people/People.svelte"), part });
  const admin = (tab: string) => ({ load: () => import("../bot/AdminPage.svelte"), tab });
  /** Each screen's component, and the part or tab of it the screen is. */
  const BODY: Record<string, { load: () => Promise<{ default: unknown }>; part?: string; tab?: string }> = {
    account: people("account"), agents: people("agents"), sharing: people("sharing"), people: people("people"),
    claxbot: { load: () => import("../bot/MePage.svelte") },
    confirmations: { load: () => import("./Confirmations.svelte") },
    roles: admin("people"), provider: admin("provider"), web: admin("web"), pricing: admin("pricing"), usage: admin("usage"),
    config: { load: () => import("../bot/SettingsPage.svelte") },
    remotes: { load: () => import("./Remotes.svelte") },
    audit: { load: () => import("../bot/AuditPage.svelte") },
  };

  let ctx = $state<Ctx | null>(null);
  // The address shown: its path, and a number that changes with each move,
  // so a screen opened again (another query) starts afresh.
  let path = $state(location.pathname);
  let visit = $state(0);
  let query = $state("");
  let Shown = $state<Body | null>(null);
  let root: HTMLElement | undefined = $state();
  const asked = $derived(idOf(path));
  const screen = $derived<Screen | null>(ctx && asked ? screenFor(asked, ctx) : null);
  const groups = $derived(ctx ? groupsFor(ctx) : []);
  let gone = false;
  // The Settings key as this machine writes it.
  const KEY = /Mac|iPhone|iPad/.test(navigator.platform) ? "⌘K" : "Ctrl K";

  // The screen's chunk, loaded when the screen changes.
  let loaded = "";
  $effect(() => {
    const id = screen?.id ?? "";
    if (id === loaded) return;
    loaded = id; Shown = null;
    if (id) void BODY[id].load().then(m => { if (!gone && loaded === id) Shown = m.default as Body; }, () => {});
  });
  $effect(() => { if (ctx && !screen) document.title = "Settings · Clax"; });
  // A screen shown instead of the one asked for is the address's from then on.
  $effect(() => { if (screen && asked && screen.id !== asked) { history.replaceState(history.state, "", `/settings/${screen.id}${location.search}${location.hash}`); path = location.pathname; } });

  // The heading an address's `#<ID>` names: shown and focused once drawn.
  let seeking: MutationObserver | null = null;
  function seek() {
    seeking?.disconnect(); seeking = null;
    const id = decodeURIComponent(location.hash.slice(1));
    if (!id || !root) return;
    const hit = () => {
      const el = document.getElementById(id);
      if (!el || !root?.contains(el)) return false;
      if (!el.hasAttribute("tabindex")) el.setAttribute("tabindex", "-1");
      el.classList.add("stg-hit");
      el.scrollIntoView?.({ block: "start" });
      el.focus({ preventScroll: true });
      return true;
    };
    if (hit()) return;
    seeking = new MutationObserver(() => { if (hit()) { seeking?.disconnect(); seeking = null; } });
    seeking.observe(root, { childList: true, subtree: true });
  }

  function show() { path = location.pathname; visit += 1; query = ""; void tick().then(seek); }
  /** Moves to a Settings address in place; any other address loads. */
  function go(href: string) {
    const u = new URL(href, location.href);
    if (u.origin !== location.origin || !/^\/settings(\/|$)/.test(u.pathname)) { location.assign(href); return; }
    const same = u.pathname === location.pathname && u.search === location.search;
    if (`${u.pathname}${u.search}${u.hash}` !== `${location.pathname}${location.search}${location.hash}`) history.pushState(null, "", `${u.pathname}${u.search}${u.hash}`);
    if (same) seek(); else show();
  }
  // A link to a Settings address, anywhere in Settings, moves in place.
  function clicked(e: MouseEvent) {
    if (e.defaultPrevented || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
    const a = (e.target as Element).closest?.("a[href]") as HTMLAnchorElement | null;
    if (!a || a.target || a.hasAttribute("download") || !/^\/settings(\/|$)/.test(a.getAttribute("href") ?? "")) return;
    e.preventDefault();
    go(a.href);
  }
  // Signing out (a hosted Clax): the hosted calls load when it is pressed.
  let leaving = $state(false);
  let outErr = $state<string | null>(null);
  async function signOut() {
    if (leaving) return;
    leaving = true; outErr = null;
    try {
      const [api, so] = await Promise.all([import("../hosted/api"), import("../hosted/signout")]);
      await so.forgetThenSignOut(api.signOut);
      location.assign("/signin");
    } catch (e) { outErr = refusal(e); leaving = false; }
  }
  const onGo = (e: Event) => { e.preventDefault(); go((e as CustomEvent<string>).detail); };

  onMount(() => {
    addEventListener(GO, onGo);
    void context().then(c => { if (!gone) { ctx = c; seek(); } });
  });
  onDestroy(() => { gone = true; removeEventListener(GO, onGo); seeking?.disconnect(); });
</script>

<svelte:window onpopstate={show} />

{#snippet out()}
  {#if ctx?.hosted && ctx.member}
    <div class="stg-out">
      <button type="button" class="ghost" disabled={leaving} onclick={signOut}>{leaving ? "Signing out…" : "Sign out"}</button>
      {#if outErr}<p class="stg-err" role="alert">{outErr}</p>{/if}
    </div>
  {/if}
{/snippet}

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="stg" class:on-screen={!!screen} bind:this={root} onclick={clicked}>
  {#if !ctx}
    <p class="stg-note" role="status">Loading…</p>
  {:else if screen}
    <nav class="stg-nav" aria-label="Settings">
      <a class="stg-home" href="/settings">Settings</a>
      <button type="button" class="stg-find" onclick={openPalette}>
        <svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="7" cy="7" r="4.5" /><path d="M10.5 10.5 14 14" /></svg>
        <span>Search settings</span><kbd>{KEY}</kbd>
      </button>
      {#each groups as g (g.name)}
        <h3>{g.name}</h3>
        <ul>
          {#each g.screens as s (s.id)}<li><a href={`/settings/${s.id}`} aria-current={s.id === screen.id ? "page" : undefined}>{s.title}</a></li>{/each}
        </ul>
      {/each}
      {@render out()}
    </nav>
    <div class="stg-body">
      <div class="stg-back"><a href="/settings">‹ All settings</a><button type="button" class="ghost" aria-label="Search settings" onclick={openPalette}><svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="7" cy="7" r="4.5" /><path d="M10.5 10.5 14 14" /></svg></button></div>
      {#key `${screen.id} ${visit}`}
        {#if Shown}<Shown {stream} part={BODY[screen.id].part} tab={BODY[screen.id].tab} />{:else}<p class="stg-note" role="status">Loading…</p>{/if}
      {/key}
    </div>
  {:else}
    <div class="stg-over">
      <div class="stg-head"><h2>Settings</h2></div>
      {#if !ctx.member}<p class="stg-note">Settings are for the owner of this Clax and the people they connect.</p>
      {:else if asked}<p class="stg-note">That screen is not one this browser can open here. These are the settings it can.</p>{/if}
      <Search {ctx} bind:query onGo={go} />
      {#if !query.trim()}
        {#each groups as g (g.name)}
          <section aria-labelledby={`stg-g-${g.name}`}>
            <h3 id={`stg-g-${g.name}`}>{g.name}</h3>
            <ul class="stg-cards">
              {#each g.screens as s (s.id)}<li><a href={`/settings/${s.id}`}><b>{s.title}</b><span>{s.line}</span></a></li>{/each}
            </ul>
          </section>
        {/each}
        {@render out()}
      {/if}
    </div>
  {/if}
</div>

<style>
  :global {
    .stg { min-width: 0; }
    .stg.on-screen { display: grid; grid-template-columns: 216px minmax(0, 1fr); gap: 28px; align-items: start; }
    .stg-note { margin: 0; color: var(--muted); font-size: 13px; }
    .stg-nav { position: sticky; top: 16px; display: flex; flex-direction: column; gap: 2px; min-width: 0; }
    .stg-home { font-weight: 700; font-size: 18px; color: var(--fg); text-decoration: none; padding: 4px 8px; border-radius: var(--radius-sm); }
    .stg-home:hover { background: var(--hover); }
    .stg-find { display: flex; align-items: center; gap: 8px; width: 100%; margin: 6px 0 4px; padding: 0 8px; min-height: 34px; color: var(--muted); font-weight: 500; text-align: start; }
    .stg-find span { flex: 1; min-width: 0; }
    .stg-find kbd { font: 500 11px/18px var(--mono); border: 1px solid var(--border-hover); border-radius: var(--radius-xs); padding: 0 5px; background: var(--card); }
    .stg svg { width: 16px; height: 16px; flex: none; fill: none; stroke: currentColor; stroke-width: 1.4; stroke-linecap: round; }
    .stg-nav h3 { margin: 12px 0 2px; padding: 0 8px; font-size: 12px; font-weight: 600; color: var(--muted); text-transform: uppercase; letter-spacing: .04em; }
    .stg-nav ul, .stg-cards { list-style: none; margin: 0; padding: 0; }
    .stg-nav li a { display: flex; align-items: center; min-height: 32px; padding: 2px 8px; border-radius: var(--radius-sm); color: var(--fg); text-decoration: none; font-size: 14px; }
    .stg-nav li a:hover { background: var(--hover); }
    .stg-nav li a[aria-current="page"] { background: var(--hover); font-weight: 600; box-shadow: inset 2px 0 0 var(--accent); }
    .stg-out { margin-top: 14px; padding-top: 10px; border-top: 1px solid var(--border); }
    .stg-out button { color: var(--fg); font-weight: 600; }
    .stg-err { margin: 6px 0 0; color: var(--danger); font-size: 13px; }
    .stg-body { min-width: 0; display: flex; flex-direction: column; gap: 12px; }
    .stg-back { display: none; align-items: center; justify-content: space-between; gap: 8px; }
    .stg-back a { font-weight: 600; font-size: 14px; color: var(--fg); text-decoration: none; padding: 8px 4px; }
    /* A screen's own frame fills the column from its start. */
    .stg-body .ppl, .stg-body .bpg { margin: 0; }
    /* The heading a search result or a link names. */
    .stg .stg-hit:focus { outline: 2px solid var(--accent); outline-offset: 4px; border-radius: var(--radius-xs); }
    .stg .stg-hit { scroll-margin-top: 16px; }
    .stg-over { max-width: 760px; margin: 0 auto; display: flex; flex-direction: column; gap: 16px; min-width: 0; }
    .stg-head { padding-bottom: 12px; border-bottom: 1px solid var(--border); }
    .stg-over h2 { margin: 0; font-size: 22px; line-height: 1.2; }
    .stg-over h3 { margin: 0 0 6px; font-size: 12px; font-weight: 600; color: var(--muted); text-transform: uppercase; letter-spacing: .04em; }
    .stg-cards { display: grid; grid-template-columns: repeat(auto-fill, minmax(230px, 1fr)); gap: 8px; }
    .stg-cards a { display: flex; flex-direction: column; gap: 4px; height: 100%; padding: 12px 14px; background: var(--card); border: 1px solid var(--border); border-radius: var(--radius); color: var(--fg); text-decoration: none; }
    .stg-cards a:hover { border-color: var(--border-hover); background: var(--hover); }
    .stg-cards b { font-size: 14.5px; }
    .stg-cards span { color: var(--muted); font-size: 12.5px; line-height: 1.45; }
    @media (pointer: coarse) { .stg-nav li a, .stg-find { min-height: 44px; } }
    @media (max-width: 760px) {
      .stg.on-screen { display: block; }
      .stg-nav { display: none; }
      .stg-back { display: flex; }
    }
  }
</style>
