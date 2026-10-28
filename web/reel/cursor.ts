// The camera's cursor (spec 2026-10-27-sizzle-reel-design §6.3): an arrow
// drawn over the camera's page, so a viewer sees where the local person
// points. Headless Chromium paints no pointer of its own. It lives in a
// closed shadow root on the top document, never takes a pointer event, and
// stays hidden until the director first moves it. Other people's pointers
// are not this: they are Clax's own live layers, which the shell draws.
//
// The page never moves it; the director does, through the DevTools protocol
// with no user gesture (quietEval), so the cursor grants the shell no user
// activation of its own: only the director's real mouse and keyboard input
// does, as a person's would.
import type { Page } from "@playwright/test";
import { quietEval } from "../e2e/time.ts";

/** The arrow's height, in CSS pixels. */
export const CURSOR_PX = 20;
/** How long the ring a press draws takes to fade, in ms. */
export const PRESS_MS = 300;

// Defines `reelCursor` on the top window: `to(x, y)` puts the arrow's tip at
// (x, y) in viewport pixels and shows it; `press()` draws a ring there.
const SCRIPT = `(() => {
  if (window !== window.top || window.reelCursor) return;
  let host = null, arrow = null, ring = null, at = { x: 0, y: 0 };
  const mount = () => {
    if (host && host.isConnected) return;
    host = document.createElement("div");
    host.setAttribute("style", "all: initial; position: fixed; left: 0; top: 0; width: 0; height: 0; z-index: 2147483647; pointer-events: none;");
    const root = host.attachShadow({ mode: "closed" });
    root.innerHTML = \`<style>
      :host { all: initial; }
      .arrow, .ring { position: fixed; left: 0; top: 0; pointer-events: none; display: none; }
      .arrow { width: ${Math.round(CURSOR_PX * 0.7)}px; height: ${CURSOR_PX}px; filter: drop-shadow(0 1px 1.5px rgba(0, 0, 0, 0.35)); }
      .ring { width: 28px; height: 28px; margin: -14px 0 0 -14px; border-radius: 50%; border: 2px solid rgba(20, 20, 20, 0.55); box-sizing: border-box; opacity: 0; }
      .ring.on { animation: press ${PRESS_MS}ms cubic-bezier(0.05, 0.7, 0.1, 1) forwards; }
      @keyframes press { from { opacity: 0.9; transform: var(--at) scale(0.4); } to { opacity: 0; transform: var(--at) scale(1.2); } }
    </style>
    <div class="ring"></div>
    <svg class="arrow" viewBox="0 0 14 20" xmlns="http://www.w3.org/2000/svg">
      <path d="M1 1 L1 16.2 L4.6 12.9 L7.3 18.9 L9.9 17.8 L7.3 11.9 L12.2 11.9 Z" fill="#111" stroke="#fff" stroke-width="1.3" stroke-linejoin="round"/>
    </svg>\`;
    arrow = root.querySelector(".arrow");
    ring = root.querySelector(".ring");
    document.documentElement.appendChild(host);
  };
  window.reelCursor = {
    to(x, y) {
      mount();
      at = { x, y };
      arrow.style.display = "block";
      // The path's tip is at (1, 1) of its box.
      arrow.style.transform = \`translate(\${x - 1}px, \${y - 1}px)\`;
    },
    press() {
      mount();
      ring.style.display = "block";
      ring.style.setProperty("--at", \`translate(\${at.x}px, \${at.y}px)\`);
      ring.style.transform = \`translate(\${at.x}px, \${at.y}px)\`;
      ring.classList.remove("on");
      void ring.offsetWidth;
      ring.classList.add("on");
    },
  };
})();`;

/** Adds the cursor to `page`'s top document, now and after every navigation. */
export async function installCursor(page: Page): Promise<void> {
  await page.addInitScript(SCRIPT);
  await quietEval(page, SCRIPT);
}

/** Puts the cursor's tip at (`x`, `y`) in `page`'s viewport. */
export async function cursorTo(page: Page, x: number, y: number): Promise<void> {
  await quietEval(page, `window.reelCursor && window.reelCursor.to(${Number(x)}, ${Number(y)})`);
}

/** Draws the press ring where the cursor is. */
export async function cursorPress(page: Page): Promise<void> {
  await quietEval(page, "window.reelCursor && window.reelCursor.press()");
}
