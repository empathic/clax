<script lang="ts">
  // The one modal the shell shows for a page: a capability's consent or a
  // download's confirmation. It opens with focus on the refusing button (set
  // as it is inserted, before its first paint), and "Allow" stays inert for
  // ALLOW_DELAY_MS counted from the dialog's first paint, so a keystroke or
  // click meant for the page cannot grant consent, however long the page
  // keeps the main thread busy before that paint. The page opened it, so
  // "Allow" grants only on a trusted pointer's click (`guardedAction`, the
  // tainted keyboard trail): Enter or Space on it, or any other activation
  // with no pointer, says "Click to allow" and grants nothing. A press that
  // began before "Allow" was armed never grants: a click counts only when
  // the same pointer (by its pointerId) went down after arming, judged by the
  // event's own timeStamp as well as by when it is handled, so input the
  // browser queued while the main thread was busy is not counted as fresh.
  // Escape dismisses it (neither allow nor deny). While it is open the rest
  // of the shell is inert and Tab stays among its buttons ("Don't allow"
  // alone until "Allow" is armed).
  import { ALLOW_DELAY_MS, type Ask } from "../view/prompt-queue";
  import { guardedAction } from "../view/trail";
  import { inertOutside, trapTab } from "./modal";

  let { ask }: { ask: Ask } = $props();
  let deny = $state<HTMLButtonElement>();
  let box = $state<HTMLElement>();
  let armed = $state(false);
  // When "Allow" was armed (performance.now(), the clock of event timeStamps).
  let armedAt = Infinity;
  // By pointerId: whether that pointer's latest press began after arming.
  let fresh = new Map<number, boolean>();
  // Whether the latest press of a pointer that reports no ID (a browser whose
  // click is not a PointerEvent) began after arming.
  let lastFresh = false;
  // Said when something other than a pointer's click asked for "Allow".
  let hint: string | null = $state(null);
  const after = (e: Event) => armed && e.timeStamp >= armedAt;

  // Each ask (the next one in the queue reuses the dialog) refocuses the
  // refusing button and disarms "Allow" at once, so it never paints armed,
  // then arms it ALLOW_DELAY_MS after the dialog's next paint: the second
  // animation frame runs only once the first one, which painted the dialog,
  // is done (in a hidden tab, not until the tab is shown).
  $effect(() => {
    const a = ask;
    armed = false;
    armedAt = Infinity;
    fresh = new Map();
    lastFresh = false;
    hint = null;
    deny?.focus();
    let second = 0;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const first = requestAnimationFrame(() => {
      second = requestAnimationFrame(() => {
        timer = setTimeout(() => { armedAt = performance.now(); armed = true; }, ALLOW_DELAY_MS);
      });
    });
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") a.answer("dismiss"); };
    const onPress = (e: PointerEvent) => {
      lastFresh = after(e);
      if (typeof e.pointerId === "number") fresh.set(e.pointerId, lastFresh);
    };
    addEventListener("keydown", onKey);
    addEventListener("pointerdown", onPress, true);
    return () => {
      cancelAnimationFrame(first);
      cancelAnimationFrame(second);
      clearTimeout(timer);
      removeEventListener("keydown", onKey);
      removeEventListener("pointerdown", onPress, true);
    };
  });

  /** Whether a pointer's click on "Allow" ends a press that began after it
   * was armed: that pointer's pointerdown (by pointerId) after arming, or,
   * for a click that reports no ID, the latest pointerdown after arming. */
  function grants(e: MouseEvent): boolean {
    if (!armed) return false;
    const id = (e as Partial<PointerEvent>).pointerId;
    if (typeof id === "number" && id >= 0) return fresh.get(id) === true;
    return lastFresh;
  }
  /** A click on "Allow" ends its press: that press cannot count again. */
  function spent(e: MouseEvent): void {
    const id = (e as Partial<PointerEvent>).pointerId;
    if (typeof id === "number") fresh.delete(id);
    lastFresh = false;
  }
</script>

<div class="prompt-backdrop" {@attach inertOutside}>
  <!-- Tab cycles inside the dialog; its other keys are handled above. -->
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div class="prompt" role="dialog" aria-modal="true" aria-labelledby="prompt-title" aria-describedby="prompt-body" tabindex="-1"
    bind:this={box} onkeydown={e => { if (box) trapTab(e, box); }}>
    <h2 id="prompt-title">{ask.prompt.title}</h2>
    <p id="prompt-body">{ask.prompt.body}</p>
    <div class="actions">
      <button type="button" bind:this={deny} onclick={() => ask.answer("deny")}>{ask.prompt.deny}</button>
      <button type="button" class="primary" disabled={!armed} onclick={e => {
        hint = guardedAction(e, "allow", () => {
          const ok = grants(e);
          spent(e);
          if (ok) ask.answer("allow");
        }, true);
      }}>{ask.prompt.allow}</button>
    </div>
    <p class="act-hint" role="status">{hint ?? ""}</p>
  </div>
</div>
