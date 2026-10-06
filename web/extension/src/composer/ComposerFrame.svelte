<script lang="ts">
  // The composer over the page (spec 2026-10-05 L7): the shell's Composer,
  // in an extension page the overlay frames, talking to the worker over the
  // port named for its pick. Nothing typed here reaches the page, and the
  // page holds no credential: the worker posts. The draft stays when focus
  // leaves the frame (a page may move focus to itself mid-draft, and then
  // keys go to the page), and a notice says so until focus comes back.
  // When the worker lets go of the port unasked (Chrome stopped it), the
  // composer says so and tells the worker (`lost`), which gives comment mode
  // back on the page; the text stays until the person closes the composer
  // (`dismiss`). Cancel never throws on a closed port.
  import Composer from "../../../shell/src/ui/Composer.svelte";
  import type { Draft } from "../../../shell/src/view/composer-model";
  import { dataUrlBlob } from "../data-url";
  import { type ComposerNote, isToComposer } from "../messages";
  import { clipMessage } from "./clip-message";

  type Port = Pick<chrome.runtime.Port, "postMessage" | "onMessage" | "onDisconnect">;
  /** Sends a one-off message to the worker (`chrome.runtime.sendMessage`). */
  type Notify = (m: ComposerNote) => Promise<unknown> | void;
  const runtimeNotify: Notify = m => chrome.runtime.sendMessage(m);
  let { port, pickId, win = window, notify = runtimeNotify }: { port: Port; pickId: string; win?: Window; notify?: Notify } = $props();
  const note = (m: ComposerNote) => {
    try { void Promise.resolve(notify(m)).catch(() => {}); } catch { /* the extension is gone */ }
  };

  let draft = $state<Draft | null>(null);
  let typed = "";
  let failure = $state<string | null>(null);
  let away = $state(false);
  let gone = $state(false);
  /** Posted or cancelled: the worker letting go of the port then is expected. */
  let done = false;
  let waiting: { ok: () => void; fail: (e: Error) => void } | null = null;

  const notice = $derived(
    gone ? "Clax stopped listening to this comment. Copy your comment, then pick again."
    : failure !== null ? `Not posted: ${failure}`
    : away ? "Typing now goes to the page, not to this comment. Click the comment to keep typing."
    : null,
  );

  $effect(() => {
    const p = port;
    const id = pickId;
    p.onMessage.addListener((m: unknown) => {
      if (!isToComposer(m)) return;
      if (m.t === "draft") {
        draft = { pickId: id, anchor: m.anchor, version: 0, clip: m.clipUrl ? dataUrlBlob(m.clipUrl) : null, clipError: clipMessage(m.clipError), capturing: m.capturing };
      } else if (m.t === "posted") {
        done = true;
        waiting?.ok();
        waiting = null;
      } else {
        failure = m.message;
        waiting?.fail(new Error(m.message));
        waiting = null;
      }
    });
    p.onDisconnect.addListener(() => {
      if (done) return;
      gone = true;
      waiting?.fail(new Error("disconnected"));
      waiting = null;
      note({ t: "lost", pickId: id });
    });
    p.postMessage({ t: "ready" });
  });

  $effect(() => {
    const w = win;
    const blur = () => { if (typed.trim()) away = true; };
    const focus = () => { away = false; };
    w.addEventListener("blur", blur);
    w.addEventListener("focus", focus);
    return () => { w.removeEventListener("blur", blur); w.removeEventListener("focus", focus); };
  });

  const submit = (body: string) => new Promise<void>((ok, fail) => {
    if (gone) { fail(new Error("disconnected")); return; }
    failure = null;
    waiting = { ok, fail };
    try {
      port.postMessage({ t: "post", body });
    } catch {
      waiting = null;
      gone = true;
      fail(new Error("disconnected"));
    }
  });

  /** Cancel: over the port while the worker holds it, else as a one-off message. */
  function cancel(): void {
    done = true;
    if (!gone) {
      try { port.postMessage({ t: "cancel" }); return; } catch { gone = true; }
    }
    note({ t: "dismiss", pickId });
  }
</script>

{#if notice}<p class="notice" role="alert">{notice}</p>{/if}
{#if draft}
  <Composer {draft} onCancel={cancel} onSubmit={submit} onText={t => { typed = t; }} />
{:else}
  <p class="wait">Preparing the comment…</p>
{/if}

<style>
  /* The shell's composer floats over its stage; here it is the whole frame,
     which the overlay sizes, rounds and shadows. */
  :global(html), :global(body) { margin: 0; height: 100%; overflow: auto; background: var(--raised); color: var(--fg); }
  :global(.composer) { position: static; width: auto; border: 0; border-top: 3px solid var(--you); border-radius: 0; box-shadow: none; }
  /* The clip is the whole viewport with the pick outlined: a thumbnail of all of it. */
  :global(.composer .clip) { width: 100%; height: 64px; max-height: 64px; object-fit: contain; object-position: center; }
  .wait { margin: 16px; font-size: 14px; color: var(--muted); }
  .notice { margin: 0; padding: 6px 12px; font-size: 13px; line-height: 1.35; background: var(--comment-hl); color: var(--fg); border-bottom: 1px solid var(--border); }
</style>
