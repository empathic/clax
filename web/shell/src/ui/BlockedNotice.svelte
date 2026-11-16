<svelte:options css="injected" />

<script lang="ts">
  // The notice that Clax's lockdown blocked the page from other sites
  // (`clax:blocked`; words in own-policy-model.ts), in the stage's foot over
  // the frame, so it moves nothing of the page. To whoever may turn the
  // content policy switch (`admin`), it names the sites and offers the
  // Permissions panel, which holds the switch (`onPermissions`, null where
  // the panel cannot open), and once the switch is on (`own`), Reload; to
  // anyone else, one line. Dismiss hides it until another document greets.
  // The origins are the page's text, rendered as text; nothing here acts
  // on them.
  import { blockedNotice } from "./own-policy-model";

  let { origins, admin, own, onPermissions, onReload, onDismiss }: {
    origins: readonly string[]; admin: boolean; own: boolean; onPermissions: (() => void) | null; onReload(): void; onDismiss(): void;
  } = $props();
  const words = $derived(blockedNotice(origins, admin, own));
</script>

{#if words}
  <div class="banner blocked" role="status" data-blocked-notice>
    <span class="what"><span>{words.text}</span>{#if words.sites}<span class="sites" data-testid="blocked-sites">{words.sites}</span>{/if}</span>
    {#if admin && own}
      <button type="button" class="primary" onclick={onReload}>Reload</button>
    {:else if admin && onPermissions}
      <button type="button" onclick={onPermissions}>Permissions…</button>
    {/if}
    <button type="button" class="ghost" aria-label="Dismiss" onclick={onDismiss}>Dismiss</button>
  </div>
{/if}

<style>
  /* In the stage's foot (StageIsland, theme.css .stage-foot). */
  .blocked { font-size: 13px; }
  .what { display: grid; gap: 2px; min-width: 0; }
  .sites { color: var(--muted); font-size: 12.5px; overflow-wrap: anywhere; }
  @media (max-width: 480px) { .blocked { align-self: stretch; flex-wrap: wrap; } .what { flex-basis: 100%; } }
</style>
