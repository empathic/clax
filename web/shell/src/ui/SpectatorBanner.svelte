<svelte:options css="injected" />
<script lang="ts">
  // A spectator's banner (spec 2026-10-11-clax-dev-hosted-design §16.1):
  // read-only, beyond the live seats, with Join live once a seat is free.
  // Loaded with spectator mode (view/spectator.ts), off the artifact entry.
  let { code, joinable, onJoin }: { code: string; joinable: boolean; onJoin(): void } = $props();
</script>

<div class="banner spectate" role="status">
  <!-- A seat released for want of input says so, rather than that the artifact is full. -->
  <span>{code === "seat_idle" ? "You were away, so live activity is paused." : "Lots of people are here, so you're viewing: comments and live activity are paused."}</span>
  <!-- Join live shows always after a seat went idle (the viewer's next input
       asks for it anyway), else once a snapshot says a seat this viewer may take is free. -->
  {#if code === "seat_idle" || joinable}<button class="primary" onclick={onJoin}>Join live</button>{/if}
</div>

<style>
  /* In the stage's foot (StageIsland, theme.css .stage-foot), so the top of the page it covers stays readable. */
  .spectate { font-size: 13px; }
  span { min-width: 0; }
  @media (max-width: 480px) { .spectate { align-self: stretch; } }
</style>
