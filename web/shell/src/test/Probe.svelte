<script lang="ts">
  let { label, onGone }: { label: string; onGone?: () => void } = $props();
  let clicks = $state(0);
  $effect(() => () => onGone?.());
</script>

<button onclick={() => clicks++}>{label} {clicks}</button>
