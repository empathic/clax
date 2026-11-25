<svelte:options css="injected" />

<script lang="ts">
  // A failed model call in words. With `detail` (`errorDetail`): what went
  // wrong, what the provider said (untrusted text: interpolated, quoted in
  // its own block, wrapped at any width), its status, type and request ID
  // in a muted line, and, unless `next` is off, what to do next: on the
  // person's own key (`keySource` `own`) a link to check it under
  // Settings → Your ClaxBot; on the instance's, a link to Settings → Model
  // provider for an admin, else that an admin can look there. `before` opens the lead ("The last request with
  // this key failed 5 min ago:"), which then goes on in lower case.
  // With `problem` instead (a failure on the instance's key, shown to
  // someone who is not an admin), Clax's own message (`instanceProblemWords`):
  // nothing the provider said, and run `run`'s ID to quote to an admin.
  import type { ErrorDetail, KeySource } from "./api";
  import { errorDetail, instanceProblemWords } from "./model";

  let { detail = null, problem = null, run = "", admin = false, keySource, before = "", next = true }: {
    detail?: ErrorDetail | null; problem?: { busy: boolean } | null; run?: string;
    admin?: boolean; keySource?: KeySource; before?: string; next?: boolean;
  } = $props();
  const w = $derived(detail ? errorDetail(detail, admin, keySource) : null);
  const p = $derived(problem ? instanceProblemWords(problem, run) : null);
  const lead = $derived(!w ? "" : before ? `${before} ${w.lead[0].toLowerCase()}${w.lead.slice(1)}` : w.lead);
</script>

{#if p}
  <div class="merr merr-instance">
    <p class="merr-lead">{p.lead}</p>
    <p class="merr-body">{p.body}</p>
    <p class="merr-meta">{p.ref}</p>
  </div>
{:else if w}
  <div class="merr">
    <p class="merr-lead">{lead}</p>
    {#if w.said}<blockquote class="merr-said">{w.said}</blockquote>{/if}
    {#if w.meta}<p class="merr-meta">{w.meta}</p>{/if}
    {#if next && w.next}<p class="merr-next">{#if w.next.href}<a href={w.next.href}>{w.next.text}</a>{:else}{w.next.text}{/if}</p>{/if}
  </div>
{/if}

<style>
  :global {
    .merr { display: grid; gap: 4px; min-width: 0; margin-top: 2px; }
    .merr p { margin: 0; line-height: 1.5; overflow-wrap: anywhere; }
    .merr-said { margin: 0; padding-left: 8px; border-left: 2px solid var(--border); color: var(--fg); white-space: pre-wrap; overflow-wrap: anywhere; word-break: break-word; min-width: 0; }
    .merr .merr-meta { font-size: 12px; color: var(--muted); font-variant-numeric: tabular-nums; }
    .merr-next a { color: var(--accent-ink); text-decoration: underline; }
  }
</style>
