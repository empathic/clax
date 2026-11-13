<svelte:options css="injected" />

<script lang="ts" module>
  /** A viewer as `GET /api/viewers/me` gives it; null before it answers. */
  export type Viewer = { public_id?: string; display_name?: string | null; person?: boolean; person_revoked_at?: string } | null | undefined;
</script>

<script lang="ts">
  // The gallery header's links for a member: the owner (`owner`: the token
  // is served) and a connected person who is not revoked (spec
  // 2026-10-08-multi-agent-multi-person §17). `show` names the one this
  // instance is: **Queues** (`/queues`, the report queues), for the owner
  // and the instance's admins only (`myRole`), or **Settings**, the gear
  // at the header's end (`/settings`), for every member. While the gear
  // shows, the Settings key (Cmd+K, or Ctrl+K) opens Settings' search over
  // the page (`settings/palette.ts`). They load with the gallery's lazy
  // module, as Inbox does, so the gallery's first paint carries none of
  // it. When a Settings screen finds it is no longer the viewer's
  // (`clax:people-gone`), the link asks `GET /api/viewers/me` again and
  // follows that answer. A hosted daemon serves no token: there the owner
  // is the browser whose session `GET /api/hosted/me` names the owner's,
  // asked only when neither says the browser is a member's.
  import { onDestroy } from "svelte";
  import { isAdmin, myRole } from "../bot/me";
  import { isPaletteKey, openPalette } from "../settings/palette";
  /** `current`: the page shown (`queues`, `settings`), whose link is marked current. */
  const p: { show: "queues" | "settings"; current?: string; owner: boolean; viewer: Viewer } = $props();
  let fresh = $state<Viewer>(undefined);
  const viewer = $derived(fresh === undefined ? p.viewer : fresh);
  let hostedOwner = $state(false);
  let admin = $state(false);
  // svelte-ignore state_referenced_locally
  if (p.show === "queues") void myRole().then(r => { admin = isAdmin(r); }, () => {});
  $effect(() => {
    if (p.owner || viewer === undefined || (viewer?.person && !viewer.person_revoked_at)) return;
    void fetch("/api/hosted/me").then(r => (r.ok ? r.json() : null)).then((b: { person?: { owner?: boolean } } | null) => { hostedOwner = !!b?.person?.owner; }, () => {});
  });
  const member = $derived(p.owner || hostedOwner || (!!viewer?.person && !viewer.person_revoked_at));
  const recheck = () => { void fetch("/api/viewers/me").then(r => r.json()).then((b: { viewer?: Viewer }) => { fresh = b?.viewer ?? null; }, () => {}); };
  const key = (e: KeyboardEvent) => { if (p.show === "settings" && member && isPaletteKey(e)) { e.preventDefault(); openPalette(); } };
  addEventListener("clax:people-gone", recheck);
  addEventListener("keydown", key);
  onDestroy(() => { removeEventListener("clax:people-gone", recheck); removeEventListener("keydown", key); });
</script>

{#if member && p.show === "queues" && (p.owner || hostedOwner || admin)}
<a class="queues-link" href="/queues" aria-label="Queues" aria-current={p.current === "queues" ? "page" : undefined}>
  <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M2.5 4h11M2.5 8h11M2.5 12h7" /></svg>
  <span class="word">Queues</span>
</a>
{/if}
{#if member && p.show === "settings"}
<a class="settings-link" href="/settings" aria-label="Settings" title="Settings" aria-current={p.current === "settings" ? "page" : undefined}>
  <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M6.7 3.1L6.9 1.4L9.1 1.4L9.3 3.1L10.6 3.6L11.9 2.5L13.5 4.1L12.4 5.4L12.9 6.7L14.6 6.9L14.6 9.1L12.9 9.3L12.4 10.6L13.5 11.9L11.9 13.5L10.6 12.4L9.3 12.9L9.1 14.6L6.9 14.6L6.7 12.9L5.4 12.4L4.1 13.5L2.5 11.9L3.6 10.6L3.1 9.3L1.4 9.1L1.4 6.9L3.1 6.7L3.6 5.4L2.5 4.1L4.1 2.5L5.4 3.6Z" /><circle cx="8" cy="8" r="2.1" /></svg>
</a>
{/if}

<style>
  :global {
    .queues-link, .settings-link { display: inline-flex; align-items: center; gap: 6px; flex: none; min-height: 32px; padding: 0 8px; border-radius: var(--radius-sm); font: 600 14px var(--font); color: var(--fg); text-decoration: none; white-space: nowrap; }
    .settings-link { min-width: 32px; justify-content: center; }
    .queues-link:hover, .queues-link[aria-current="page"], .settings-link:hover, .settings-link[aria-current="page"] { background: var(--hover); }
    .queues-link svg, .settings-link svg { width: 16px; height: 16px; fill: none; stroke: currentColor; stroke-width: 1.4; stroke-linecap: round; stroke-linejoin: round; color: var(--muted); }
    .settings-link svg { width: 18px; height: 18px; }
    .settings-link:hover svg, .settings-link[aria-current="page"] svg { color: var(--fg); }
    /* Without the search box (the inbox, Settings), the header's links sit at its end. */
    .gbar .sub + .inbox-link, .gbar .sub + .queues-link { margin-left: auto; }
    @media (pointer: coarse) { .queues-link, .settings-link { min-height: 44px; min-width: 44px; } }
    /* At phone width beside the search box, the icon alone. */
    @media (max-width: 520px) { .gbar .search ~ .queues-link .word { display: none; } }
  }
</style>
