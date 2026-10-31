<script lang="ts">
  // What a comment is about, quoted: the picked text, or the element's or
  // area's name, on the comment highlight with the people's rule beside it.
  // Styled only through Clax's tokens.
  type Props = { text: string };
  let { text }: Props = $props();
</script>

<p class="quote">{text}</p>

<style>
  .quote { margin: 0 0 8px; padding: 4px 8px; font-size: 13px; color: var(--muted); background: var(--comment-hl); border-left: 2px solid var(--you); border-radius: 0 var(--radius-xs) var(--radius-xs) 0; overflow-wrap: anywhere; }
</style>
