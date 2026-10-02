<script lang="ts">
  // The gallery: every artifact as a card, in the order the API lists them,
  // a search over them, and pin and delete once the token is known.
  import { onMount } from "svelte";
  import { type Artifact, deleteArtifact, getToken, listArtifacts, patchArtifact } from "../api";
  import { relativeTime } from "../format";
  import { filterArtifacts, publisherText } from "../view/gallery-model";
  import Mark from "./Mark.svelte";
  import ThemeSwitch from "./ThemeSwitch.svelte";

  let artifacts = $state<Artifact[] | null>(null);
  let error = $state<string | null>(null);
  let token = $state<string | null>(null);
  let query = $state("");
  const describe = (e: unknown) => (e instanceof Error ? e.message : String(e));
  const refresh = () => listArtifacts().then(a => { error = null; artifacts = a; }, e => { error = describe(e); });
  const act = (op: () => Promise<unknown>) => op().then(refresh, e => { error = describe(e); });
  const shown = $derived(artifacts && filterArtifacts(artifacts, query));
  onMount(() => { void refresh(); void getToken().then(t => { token = t; }); });
</script>

<header class="gbar">
  <Mark playful /><h1>Clax</h1><span class="muted hide-sm">local artifacts</span>
  <input type="search" class="search" placeholder="Search artifacts" aria-label="Search artifacts" bind:value={query} />
  <ThemeSwitch />
</header>
<main class="wrap">
  {#if error}<p class="empty">Could not load artifacts: {error}</p>{/if}
  {#if artifacts && artifacts.length === 0}
    <p class="empty">No artifacts yet. Publish one with <code>clax publish index.html</code>.</p>
  {/if}
  {#if artifacts && artifacts.length > 0 && shown && shown.length === 0}
    <p class="empty">No artifacts match your search.</p>
  {/if}
  {#if shown && shown.length > 0}
    <div class="grid">
      {#each shown as a (a.id)}
        {@const by = publisherText(a)}
        <div class="card-wrap">
          <a class="card" href={`/a/${a.id}`}>
            {#if a.pinned}<span class="pin" title="Pinned">★</span>{/if}
            <h2>{a.title}</h2>
            {#if a.description}<p>{a.description}</p>{/if}
            <div class="meta">
              <span>v{a.current_version}</span>
              <span>{relativeTime(a.updated_at)}</span>
              {#if a.owner_harness}
                <span class="publisher">{#if a.owner_live}<span class="live-dot" role="img" aria-label="session is live" title="Session is live"></span>{/if}{by}</span>
              {:else}
                <span>published from the command line</span>
              {/if}
            </div>
          </a>
          {#if token}
            {@const tk = token}
            <div class="card-tools">
              <button type="button" title={a.pinned ? "Unpin" : "Pin"} aria-label={a.pinned ? `Unpin ${a.title}` : `Pin ${a.title}`}
                onclick={() => act(() => patchArtifact(a.id, { pinned: !a.pinned }, tk))}>{a.pinned ? "★" : "☆"}</button>
              <button type="button" title="Delete"
                onclick={() => { if (confirm(`Delete "${a.title}"? This removes every version.`)) void act(() => deleteArtifact(a.id, tk)); }}>Delete</button>
            </div>
          {/if}
        </div>
      {/each}
    </div>
  {/if}
</main>
