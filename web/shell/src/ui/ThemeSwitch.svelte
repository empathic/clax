<script lang="ts">
  // The light and dark switch (spec §8). It is pressed while dark shows; its
  // title says what a press does. It follows the system until pressed, and a
  // press that lands on the system's scheme follows the system again.
  import { applyChoice, flip, readChoice, shownScheme, systemScheme } from "../view/theme-model";

  let choice = $state(readChoice());
  let system = $state(systemScheme());
  const shown = $derived(shownScheme(choice, system));
  const press = () => { choice = flip(choice, system); applyChoice(choice); };

  $effect(() => {
    if (typeof matchMedia !== "function") return;
    const mq = matchMedia("(prefers-color-scheme: dark)");
    const change = () => { system = mq.matches ? "dark" : "light"; };
    mq.addEventListener?.("change", change);
    return () => mq.removeEventListener?.("change", change);
  });
</script>

<button type="button" class="icon theme-switch" aria-label="Dark theme" aria-pressed={shown === "dark"} title={shown === "dark" ? "Switch to light" : "Switch to dark"} onclick={press}>
  <svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="8" cy="8" r="6.25" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="M8 1.75a6.25 6.25 0 0 1 0 12.5z" fill="currentColor"/></svg>
</button>
