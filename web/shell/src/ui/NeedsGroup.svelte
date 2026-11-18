<script lang="ts">
  // The gallery's "needs your eyes" group (spec §8): its red-orange swatch,
  // its count, the rule for what lands in it, and its wider cards (its
  // looks are gallery.css's, which the skeleton shares). Props are read off
  // `p`. Leading the gallery, it leads with `lead` (the New artifact tile).
  import type { Snippet } from "svelte";
  import type { Artifact } from "../api";
  import { NEEDS_HEADING, NEEDS_RULE } from "../view/gallery-frame";

  const p: { list: Artifact[]; card: Snippet<[Artifact]>; lead?: Snippet } = $props();
</script>

<section class="grp needs">
  <h2><span class="sw" aria-hidden="true"></span>{NEEDS_HEADING}<small>{p.list.length}</small></h2>
  <p class="rule">{NEEDS_RULE}</p>
  <div class="cards">{@render p.lead?.()}{#each p.list as a (a.id)}{@render p.card(a)}{/each}</div>
</section>
