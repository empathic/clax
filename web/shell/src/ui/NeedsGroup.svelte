<svelte:options css="injected" />

<script lang="ts">
  // The gallery's "needs your eyes" group (spec §8): its red-orange swatch,
  // its count, the rule for what lands in it, and its wider cards. It loads
  // with the gallery's lazy module, since it shows only once this viewer's
  // attention is known. Props are read off `p`.
  import type { Snippet } from "svelte";
  import type { Artifact } from "../api";

  const p: { list: Artifact[]; card: Snippet<[Artifact]> } = $props();
</script>

<section class="grp needs">
  <h2><span class="sw" aria-hidden="true"></span>Needs your eyes<small>{p.list.length}</small></h2>
  <p class="rule">A thread you're in was addressed and you haven't looked, or there's a version or reply you haven't seen.</p>
  <div class="cards">{#each p.list as a (a.id)}{@render p.card(a)}{/each}</div>
</section>

<style>
  :global {
    .needs h2 .sw { border-radius: 0 8px 8px 0; background: var(--you); }
    .needs .cards { grid-template-columns: repeat(auto-fit, minmax(380px, 1fr)); }
    .needs .card .v { font-size: 30px; } .needs .card h3 { font-size: 17px; }
    @media (max-width: 700px) { .needs .cards { grid-template-columns: 1fr; } .needs .card .v { font-size: 22px; } .needs .card h3 { font-size: 15px; } }
  }
</style>
