<svelte:options css="injected" />

<script lang="ts">
  // A hosted Clax's landing page, `/start/` (crates/clax-server/src/hosted.rs
  // `landing`): the shell's minimal chrome, the sign-in pages' header with a
  // Sign in link (Your gallery once signed in), above the quickstart
  // page this release carries, `/start/index.html`, in the stage. No
  // threads, comments or drawer: the page is the release's own, not an
  // artifact. This is the header alone: the stage is the entry's static
  // frame's (`view/gallery-frame.ts`, `gallery-main.ts`), loading beside
  // the shell's code, and the header takes the frame's place. Whether the
  // browser is signed in is the daemon's block's word (`signedIn`), else
  // asked here.
  import { onMount } from "svelte";
  import ThemeSwitch from "../ui/ThemeSwitch.svelte";
  import * as api from "./api";

  let { signedIn: given = null }: { signedIn?: boolean | null } = $props();
  // svelte-ignore state_referenced_locally
  let signedIn = $state(given === true);
  onMount(() => {
    if (given === null) void api.me().then(m => { signedIn = !!m?.signed_in; }, () => {});
  });
</script>

<header class="gbar">
  <a href="/" class="home" aria-label="Gallery"><span class="mark" data-size="bar" role="img" aria-label="Clax"><svg class="mk" viewBox="0 0 40 24" aria-hidden="true" focusable="false"><path class="l" d="M2 2.5a9.5 9.5 0 0 1 0 19" fill="none" stroke-width="4.2"/><path class="r" d="M38 2.5a9.5 9.5 0 0 0 0 19" fill="none" stroke-width="4.2"/><circle cx="20" cy="12" r="2.6"/></svg></span></a><h1>Clax</h1><span class="gsp"></span><ThemeSwitch />
  {#if signedIn}
    <a class="primary lgo" href="/">Your gallery</a>
  {:else}
    <a class="primary lgo" href="/signin">Sign in</a>
  {/if}
</header>
