<script lang="ts">
  // The one modal the shell shows for a page: a capability's consent or a
  // download's confirmation. It opens with focus on the refusing button (set
  // as it is inserted, before its first paint), and "Allow" stays inert for
  // ALLOW_DELAY_MS counted from the dialog's first paint, so a keystroke or
  // click meant for the page cannot grant consent, however long the page
  // keeps the main thread busy before that paint. A press that began before
  // "Allow" was armed never grants: a pointer click counts only when its
  // pointerdown came after arming, and a key click (Enter or Space) only when
  // its key went down after arming and was not held from before. Escape
  // dismisses it (neither allow nor deny).
  import { ALLOW_DELAY_MS, type Ask } from "../view/prompt-queue";

  let { ask }: { ask: Ask } = $props();
  let deny = $state<HTMLButtonElement>();
  let armed = $state(false);
  // The latest pointer press, and the latest Enter or Space press, began
  // while "Allow" was armed; `keyDown`: such a key is down (or its click is
  // still to come).
  let pointerFresh = false;
  let keyFresh = false;
  let keyDown = false;

  // Each ask (the next one in the queue reuses the dialog) refocuses the
  // refusing button and disarms "Allow" at once, so it never paints armed,
  // then arms it ALLOW_DELAY_MS after the dialog's next paint: the second
  // animation frame runs only once the first one, which painted the dialog,
  // is done (in a hidden tab, not until the tab is shown).
  $effect(() => {
    const a = ask;
    armed = false;
    pointerFresh = keyFresh = false;
    deny?.focus();
    let second = 0;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const first = requestAnimationFrame(() => {
      second = requestAnimationFrame(() => {
        timer = setTimeout(() => { armed = true; }, ALLOW_DELAY_MS);
      });
    });
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") a.answer("dismiss"); };
    const onPress = () => { pointerFresh = armed; };
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key !== "Enter" && e.key !== " ") return;
      keyDown = true;
      keyFresh = armed && !e.repeat;
    };
    // Space clicks on its keyup: the press it ends is forgotten only after that click.
    const onKeyUp = (e: KeyboardEvent) => { if (e.key === "Enter" || e.key === " ") setTimeout(() => { keyDown = false; }); };
    addEventListener("keydown", onKey);
    addEventListener("pointerdown", onPress, true);
    addEventListener("keydown", onKeyDown, true);
    addEventListener("keyup", onKeyUp, true);
    return () => {
      cancelAnimationFrame(first);
      cancelAnimationFrame(second);
      clearTimeout(timer);
      removeEventListener("keydown", onKey);
      removeEventListener("pointerdown", onPress, true);
      removeEventListener("keydown", onKeyDown, true);
      removeEventListener("keyup", onKeyUp, true);
    };
  });

  /** Whether a click on "Allow" ends a press that began after it was armed:
   * a pointer click (`detail` > 0) needs its pointerdown after arming; a key
   * click, a fresh Enter or Space after arming. A click with neither (an
   * assistive technology's activation) counts once armed. */
  function grants(e: MouseEvent): boolean {
    if (!armed) return false;
    if (e.detail > 0) return pointerFresh;
    if (keyDown) return keyFresh;
    return true;
  }
</script>

<div class="prompt-backdrop">
  <div class="prompt" role="dialog" aria-modal="true" aria-labelledby="prompt-title" aria-describedby="prompt-body">
    <h2 id="prompt-title">{ask.prompt.title}</h2>
    <p id="prompt-body">{ask.prompt.body}</p>
    <div class="actions">
      <button type="button" bind:this={deny} onclick={() => ask.answer("deny")}>{ask.prompt.deny}</button>
      <button type="button" class="primary" disabled={!armed} onclick={e => {
        const ok = grants(e);
        pointerFresh = keyFresh = false;
        if (ok) ask.answer("allow");
      }}>{ask.prompt.allow}</button>
    </div>
  </div>
</div>
