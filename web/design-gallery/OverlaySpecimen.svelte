<script lang="ts">
  // The overlay's panels as a deployer's page shows them: a shadow root on a
  // neutral host page ("Example Store", no Clax brand: Clax is white label),
  // holding the overlay's generated tokens and its components. The host page
  // sets its own --muted, --fg and --bg in loud colours: none of them may
  // reach the panels (web/overlay/src/tokens.test.ts says why).
  import { mount, unmount } from "svelte";
  import TOKENS from "../overlay/src/tokens.css?inline";
  import ModeBar from "../overlay/src/ModeBar.svelte";
  import Reports from "../overlay/src/Reports.svelte";
  import { REPORTS } from "./fixtures";

  const noop = async () => {};

  function overlay(host: HTMLElement) {
    const root = host.attachShadow({ mode: "open" });
    const style = document.createElement("style");
    style.textContent = TOKENS;
    root.append(style);
    const cx = document.createElement("div");
    cx.className = "cx";
    root.append(cx);
    const bar = document.createElement("div");
    bar.className = "fixed-frame";
    cx.append(bar);
    const panel = document.createElement("div");
    cx.append(panel);
    const frame = document.createElement("style");
    // ModeBar is position: fixed; the frame holds it, as the viewport would.
    frame.textContent = ".fixed-frame { position: relative; height: 64px; transform: translateZ(0); } .cx { display: grid; gap: 12px; }";
    root.append(frame);
    const mounted = [
      mount(ModeBar, { target: bar, props: { mode: "element", outlined: "the Place order button", onMode: () => {}, onCancel: () => {} } }),
      mount(Reports, { target: panel, props: { reports: REPORTS, error: null, expanded: "r2", onExpand: () => {}, onReply: noop, onVerify: noop, onStillBroken: () => {}, onClose: () => {} } }),
    ];
    return () => { for (const m of mounted) unmount(m); };
  }
</script>

<div class="store">
  <p class="store-bar"><b>Example Store</b> <span>Checkout</span></p>
  <p class="store-text">The host page's own text, in its own colours. The panels below are the overlay's.</p>
  <div class="overlay-host" {@attach overlay}></div>
</div>

<style>
  /* The host page: its own design system, colliding names included. */
  .store { --muted: #ff00ff; --fg: #00a000; --bg: #ffff00; --border: #ff0000; padding: 16px; background: #fafafa; color: #222; border: 1px dashed #888; border-radius: 4px; font: 15px/1.4 Georgia, serif; }
  .store-bar { margin: 0 0 4px; display: flex; gap: 12px; }
  .store-text { margin: 0 0 12px; color: var(--muted); }
</style>
