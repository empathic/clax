<script lang="ts">
  // The "Your name" field; saves on Enter or blur through a `NameSaver`.
  import { onMount } from "svelte";
  import type { Viewer } from "../threads";
  import { NameSaver, type SetNotice } from "../view/viewer-name-model";

  let { setNotice, onViewer }: { setNotice: SetNotice; onViewer?: (v: Viewer) => void } = $props();
  let name = $state("");
  const saver = new NameSaver(u => setNotice(u), v => onViewer?.(v));
  onMount(() => saver.load(n => { name = n; }));
</script>

<input class="viewer-name" aria-label="Your name" placeholder="Your name" maxlength="60" value={name}
  oninput={e => { name = e.currentTarget.value; saver.edit(); }} onblur={() => saver.save(name)}
  onkeydown={e => { if (e.key === "Enter") { e.preventDefault(); saver.save(name); } }} />
