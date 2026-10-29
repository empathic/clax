<svelte:options css="injected" />

<script lang="ts">
  // A hosted Clax's landing page, `/start/` (crates/clax-server/src/hosted.rs
  // `landing`): the shell's minimal chrome, the sign-in pages' header with a
  // Sign in button (Your gallery once signed in), around the quickstart
  // page this release carries, `/start/index.html`, in the stage. No
  // threads, comments or drawer: the page is the release's own, not an
  // artifact.
  import { onMount } from "svelte";
  import ThemeSwitch from "../ui/ThemeSwitch.svelte";
  import * as api from "./api";

  let signedIn = $state(false);
  onMount(() => {
    void api.me().then(m => { signedIn = !!m?.signed_in; }, () => {});
  });
</script>

<div class="landing">
  <header class="gbar">
    <a href="/" class="home" aria-label="Gallery"><span class="mark" data-size="bar" role="img" aria-label="Clax"><svg class="mk" viewBox="0 0 40 24" aria-hidden="true" focusable="false"><path class="l" d="M2 2.5a9.5 9.5 0 0 1 0 19" fill="none" stroke-width="4.2"/><path class="r" d="M38 2.5a9.5 9.5 0 0 0 0 19" fill="none" stroke-width="4.2"/><circle cx="20" cy="12" r="2.6"/></svg></span></a><h1>Clax</h1><span class="sub">Start here</span><span class="gsp"></span><ThemeSwitch />
    {#if signedIn}
      <button class="primary" type="button" onclick={() => location.assign("/")}>Your gallery</button>
    {:else}
      <button class="primary" type="button" onclick={() => location.assign("/signin")}>Sign in</button>
    {/if}
  </header>
  <iframe class="lframe" src="/start/index.html" title="Start here: Clax in five minutes"></iframe>
</div>

<style>
  :global {
    .landing { display: flex; flex-direction: column; height: 100dvh; }
    .landing .gbar { flex: none; }
    .landing .gbar .gsp { flex: 1; }
    .landing .lframe { flex: 1; width: 100%; border: 0; display: block; background: var(--bg); }
  }
</style>
