<script lang="ts">
  // A thread's comments in order, each with who wrote it over its text, ruled
  // in the colour of its kind: a person's (`you`, the people's red-orange),
  // an agent's (green), anyone else's (a neutral). Text shows as text.
  // Styled only through Clax's tokens.
  import type { ThreadComment } from "./types";

  type Props = { comments: ThreadComment[] };
  let { comments }: Props = $props();
</script>

{#each comments as c (c.id)}
  <div class="c" data-kind={c.kind}>
    <p class="who small">{c.who}</p>
    <p class="body">{c.body}</p>
  </div>
{/each}

<style>
  .c { padding-left: 8px; border-left: 2px solid var(--border-hover); }
  .c[data-kind="reporter"], .c[data-kind="you"] { border-left-color: var(--you); }
  .c[data-kind="agent"] { border-left-color: var(--agent-ink); }
  .who { margin: 0; font-size: 12px; color: var(--muted); font-weight: 600; }
  .body { margin: 0; white-space: pre-wrap; overflow-wrap: anywhere; }
</style>
