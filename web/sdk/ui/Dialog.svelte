<script lang="ts">
  // A modal dialog over a scrim: its title names it (aria-labelledby), the
  // first control in it takes focus when it opens, Tab stays inside it, and
  // Escape or a press on the scrim calls `onclose`. Its actions are the
  // caller's (`actions`). Styled only through Clax's tokens.
  import type { Snippet } from "svelte";

  type Props = { title: string; children: Snippet; actions?: Snippet; onclose: () => void };
  let { title, children, actions, onclose }: Props = $props();

  const id = $props.id();
  let scrim = $state<HTMLElement | null>(null);
  let box = $state<HTMLElement | null>(null);
  const focusable = () => [...(box?.querySelectorAll<HTMLElement>("button:not(:disabled), [href], input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex='-1'])") ?? [])];
  const active = () => { const root = box?.getRootNode(); return root instanceof ShadowRoot ? root.activeElement : document.activeElement; };

  // The keys and the scrim's press are listened for here, not as attributes:
  // the dialog and the scrim are no controls themselves.
  $effect(() => {
    if (!box || !scrim) return;
    (focusable()[0] ?? box).focus();
    const onkey = (e: KeyboardEvent) => {
      if (e.key === "Escape") { e.preventDefault(); onclose(); return; }
      if (e.key !== "Tab") return;
      const all = focusable();
      if (!all.length) { e.preventDefault(); return; }
      const first = all[0], last = all[all.length - 1];
      if (e.shiftKey && active() === first) { e.preventDefault(); last.focus(); }
      else if (!e.shiftKey && active() === last) { e.preventDefault(); first.focus(); }
    };
    const s = scrim;
    const onpress = (e: MouseEvent) => { if (e.target === s) onclose(); };
    box.addEventListener("keydown", onkey);
    s.addEventListener("click", onpress);
    const b = box;
    return () => { b.removeEventListener("keydown", onkey); s.removeEventListener("click", onpress); };
  });
</script>

<div class="scrim" bind:this={scrim}>
  <div class="dialog" role="dialog" aria-modal="true" aria-labelledby={id} tabindex="-1" bind:this={box}>
    <h2 {id}>{title}</h2>
    <div class="body">{@render children()}</div>
    {#if actions}<div class="actions">{@render actions()}</div>{/if}
  </div>
</div>

<style>
  .scrim { position: fixed; inset: 0; display: grid; place-items: center; padding: var(--gutter); background: var(--scrim); z-index: 50; }
  .dialog { width: min(420px, 100%); padding: 18px; background: var(--raised); color: var(--fg); border: 1px solid var(--border-hover); border-radius: var(--radius); box-shadow: var(--elev-lg); }
  .dialog:focus { outline: none; }
  h2 { margin: 0 0 8px; font: 600 15px/1.3 var(--font); }
  .body { margin: 0 0 16px; line-height: 1.6; overflow-wrap: anywhere; }
  .actions { display: flex; gap: 8px; justify-content: flex-end; }
</style>
