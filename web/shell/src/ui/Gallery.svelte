<script lang="ts">
  // The gallery: every artifact as a card led by its version numeral, pinned
  // first and then the most recent, a search over them, pin and delete once
  // the token is known, and a haiku in the footer after first paint.
  import { onMount } from "svelte";
  import { type Artifact, deleteArtifact, getToken, listArtifacts, patchArtifact } from "../api";
  import { filterArtifacts, orderArtifacts } from "../view/gallery-model";
  import GalleryCard from "./GalleryCard.svelte";
  import HaikuLine from "./HaikuLine.svelte";
  import Mark from "./Mark.svelte";
  import ThemeSwitch from "./ThemeSwitch.svelte";

  let artifacts = $state<Artifact[] | null>(null);
  let error = $state<string | null>(null);
  let token = $state<string | null>(null);
  let query = $state("");
  let me = $state<string | null>(null);
  const describe = (e: unknown) => (e instanceof Error ? e.message : String(e));
  const refresh = () => listArtifacts().then(a => { error = null; artifacts = a; }, e => { error = describe(e); });
  const act = (op: () => Promise<unknown>) => op().then(refresh, e => { error = describe(e); });
  const shown = $derived(artifacts && orderArtifacts(filterArtifacts(artifacts, query)));
  onMount(() => { void refresh(); void fetch("/api/viewers/me").then(r => r.json()).then(b => { me = b?.viewer?.display_name ?? null; }, () => {}); void getToken().then(t => { token = t; }); });
</script>

<header class="gbar">
  <Mark playful /><h1>Clax</h1><span class="sub hide-sm">local artifacts{#if me}{` · seen as ${me}`}{/if}</span>
  <input type="search" class="search" placeholder="Search artifacts" aria-label="Search artifacts" bind:value={query} />
  <ThemeSwitch />
</header>
<main class="gal">
  {#if error}<p class="empty">Could not load artifacts: {error}</p>{/if}
  {#if artifacts && artifacts.length === 0}
    <div class="empty-gallery"><Mark size="hero" apart /><p>When an agent publishes a page, it lands here.</p><p class="muted">Or publish one yourself with <code>clax publish index.html</code>.</p></div>
  {/if}
  {#if artifacts && artifacts.length > 0 && shown && shown.length === 0}<p class="empty">No artifacts match your search.</p>{/if}
  {#if shown && shown.length > 0}
    <section class="grp rest">
      <div class="cards">
        {#each shown as a (a.id)}
          <GalleryCard {a} {token} onPin={() => token && act(() => patchArtifact(a.id, { pinned: !a.pinned }, token!))}
            onDelete={() => { if (token && confirm(`Delete "${a.title}"? This removes every version.`)) void act(() => deleteArtifact(a.id, token!)); }} />
        {/each}
      </div>
    </section>
  {/if}
  {#if artifacts}<HaikuLine footer />{/if}
</main>
