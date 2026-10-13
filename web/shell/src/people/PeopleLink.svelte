<svelte:options css="injected" />

<script lang="ts" module>
  /** A viewer as `GET /api/viewers/me` gives it; null before it answers. */
  export type Viewer = { public_id?: string; display_name?: string | null; person?: boolean; person_revoked_at?: string } | null | undefined;
</script>

<script lang="ts">
  // **People** in the gallery's header (spec 2026-10-08-multi-agent-multi-person
  // §17, owner's answer (a)), linking to `/people`, for the owner (`owner`:
  // the token is served) and a connected person who is not revoked. It
  // loads with the gallery's lazy module, as Inbox does, so the gallery's
  // first paint carries none of it.
  const p: { current?: boolean; owner: boolean; viewer: Viewer } = $props();
</script>

{#if p.owner || (p.viewer?.person && !p.viewer.person_revoked_at)}
<a class="people-link" href="/people" aria-label="People" aria-current={p.current ? "page" : undefined}>
  <svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="6" cy="5.5" r="2.3" /><path d="M1.8 13c.4-2.4 2.1-3.7 4.2-3.7s3.8 1.3 4.2 3.7M10.6 3.4a2.2 2.2 0 1 1 0 4.3M12 9.6c1.3.5 2 1.6 2.2 3.4" /></svg>
  <span class="word">People</span>
</a>
{/if}

<style>
  :global {
    .people-link { display: inline-flex; align-items: center; gap: 6px; flex: none; min-height: 32px; padding: 0 8px; border-radius: var(--radius-sm); font: 600 14px var(--font); color: var(--fg); text-decoration: none; white-space: nowrap; }
    .people-link:hover, .people-link[aria-current="page"] { background: var(--hover); }
    .people-link svg { width: 16px; height: 16px; fill: none; stroke: currentColor; stroke-width: 1.4; stroke-linecap: round; color: var(--muted); }
    /* Without the search box (the inbox, People), the header's links sit at its end. */
    .gbar .sub + .inbox-link, .gbar .sub + .people-link { margin-left: auto; }
    @media (pointer: coarse) { .people-link { min-height: 44px; } }
    /* At phone width beside the search box, the icon alone. */
    @media (max-width: 520px) { .gbar .search ~ .people-link .word { display: none; } }
  }
</style>
