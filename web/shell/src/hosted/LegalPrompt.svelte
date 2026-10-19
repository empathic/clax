<svelte:options css="injected" />

<script lang="ts">
  // The prompt a signed-in person sees when they have not agreed to the
  // instance's current Terms of Service and Privacy Policy: everyone who
  // signed up before the documents were turned on, and everyone after a
  // material change to them. Continue records the agreement
  // (`POST /api/hosted/legal/agree`) and closes it; the documents open in a
  // new tab. When they changed again since this page asked, it reloads to
  // show the current ones. It has no way to dismiss it without agreeing:
  // signing out is on People. While it is open the rest of the page is
  // inert and Tab stays inside it.
  import * as api from "./api";
  import { refusal } from "../people/refusal";
  import { inertOutside, trapTab } from "../ui/modal";

  let { legal, done }: { legal: api.HostedLegal; done(): void } = $props();
  let busy = $state(false);
  let err = $state<string | null>(null);
  let box = $state<HTMLElement>();

  async function agree() {
    if (busy) return;
    busy = true; err = null;
    try {
      await api.agreeLegal(legal.version);
      done();
    } catch (x) {
      if (x instanceof api.Refused && x.code === "legal_changed") { location.reload(); return; }
      err = refusal(x);
      busy = false;
    }
  }
  $effect(() => { box?.querySelector<HTMLElement>("button.primary")?.focus(); });
</script>

<div class="prompt-backdrop" {@attach inertOutside} data-testid="legal-prompt">
  <!-- Tab cycles inside the dialog. -->
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div class="prompt legal-prompt" role="dialog" aria-modal="true" aria-labelledby="legal-title" aria-describedby="legal-body" tabindex="-1"
    bind:this={box} onkeydown={e => { if (box) trapTab(e, box); }}>
    <h2 id="legal-title">Terms of Service and Privacy Policy</h2>
    <p id="legal-body">This Clax has a <a href={legal.terms_url} target="_blank" rel="noopener">Terms of Service</a> and a <a href={legal.privacy_url} target="_blank" rel="noopener">Privacy Policy</a> you have not agreed to yet. By continuing, you agree to both.</p>
    {#if err}<p class="error" role="alert">{err}</p>{/if}
    <div class="actions">
      <button type="button" class="primary" disabled={busy} onclick={agree}>Continue</button>
    </div>
  </div>
</div>

<style>
  .legal-prompt a { color: inherit; text-decoration: underline; }
</style>
