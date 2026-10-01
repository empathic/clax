<script lang="ts">
  // The one modal the shell shows for a page: a capability's consent or a
  // download's confirmation. It opens with focus on the refusing button (set
  // as it is inserted, before its first paint), and "Allow" stays inert for
  // ALLOW_DELAY_MS counted from the dialog's first paint, so a keystroke or
  // click meant for the page cannot grant consent, however long the page
  // keeps the main thread busy before that paint. A press that began before
  // "Allow" was armed never grants: a pointer click counts only when the same
  // pointer (by its pointerId) went down after arming, and a key click (Enter
  // or Space) only when its key went down after arming and was not held from
  // before. "After arming" is judged by the event's own timeStamp as well as
  // by when it is handled, so input the browser queued while the main thread
  // was busy is not counted as fresh. Escape dismisses it (neither allow nor
  // deny).
  import { ALLOW_DELAY_MS, type Ask } from "../view/prompt-queue";

  let { ask }: { ask: Ask } = $props();
  let deny = $state<HTMLButtonElement>();
  let armed = $state(false);
  // When "Allow" was armed (performance.now(), the clock of event timeStamps).
  let armedAt = Infinity;
  // By pointerId: whether that pointer's latest press began after arming.
  let fresh = new Map<number, boolean>();
  // Whether the latest press of a pointer that reports no ID (a browser whose
  // click is not a PointerEvent) began after arming.
  let lastFresh = false;
  // The latest Enter or Space press began after arming; `keyDown`: such a
  // key is down (or its click is still to come).
  let keyFresh = false;
  let keyDown = false;
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
    lastFresh = keyFresh = keyDown = false;
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
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key !== "Enter" && e.key !== " ") return;
      keyDown = true;
      keyFresh = after(e) && !e.repeat;
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

  /** Whether a click on "Allow" ends a press that began after it was armed.
   * A pointer's click (a PointerEvent with a pointerId of 0 or more) needs
   * that pointer's pointerdown after arming; a pointer click that reports no
   * ID (`detail` > 0 on a plain MouseEvent), the latest pointerdown after
   * arming; a key click, a fresh Enter or Space after arming. A click with
   * none of these (an assistive technology's activation: pointerId -1 or
   * none, no key down) counts once armed. */
  function grants(e: MouseEvent): boolean {
    if (!armed) return false;
    const id = (e as Partial<PointerEvent>).pointerId;
    if (typeof id === "number" && id >= 0) return fresh.get(id) === true;
    if (typeof id !== "number" && e.detail > 0) return lastFresh;
    if (keyDown) return keyFresh;
    return true;
  }
  /** A click on "Allow" ends its press: that press cannot count again. */
  function spent(e: MouseEvent): void {
    const id = (e as Partial<PointerEvent>).pointerId;
    if (typeof id === "number") fresh.delete(id);
    lastFresh = keyFresh = false;
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
        spent(e);
        if (ok) ask.answer("allow");
      }}>{ask.prompt.allow}</button>
    </div>
  </div>
</div>
