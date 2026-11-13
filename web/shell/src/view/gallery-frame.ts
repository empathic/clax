import { MARK_SVG } from "./mark";

// The gallery entry's static frame (`shell/index.html` carries it in
// `#app`, so the first frame painted, before any script, is the page's
// final layout): the header bar with the mark, the title, an empty
// subtitle, the empty slot the header's links mount into, the search box
// (the gallery's list only) and the theme switch, each where the mounted
// header puts it; `main` holding the skeleton; and the footer. The
// gallery's entry mounts `Gallery.svelte` in its place in one task
// (`gallery-main.ts`), which shows the same skeleton (`SKELETON_HTML`)
// until its content is in. The footer stays: the gallery adds a hosted
// Clax's legal links to it. The inline `clax-page` script marks the root
// with the page's kind (`data-page`, `gallery-start.ts` `pageKind`): the
// skeleton shows cards on the gallery and lines on the pages beside it,
// the search box shows on the gallery only, and the sign-in pages show
// none of the frame.

/** The theme switch's button as `ThemeSwitch.svelte` renders it. */
export const THEME_SWITCH_HTML = `<button type="button" class="icon theme-switch" aria-label="Dark theme" aria-pressed="false" title="Switch to dark"><svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="8" cy="8" r="6.25" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="M8 1.75a6.25 6.25 0 0 1 0 12.5z" fill="currentColor"/></svg></button>`;

/** The header bar: what `Gallery.svelte` renders before the header's answers are in. */
export const HEADER_HTML = `<header class="gbar"><a href="/" class="home" aria-label="Gallery"><span class="mark" data-size="bar" role="img" aria-label="Clax">${MARK_SVG}</span></a><h1>Clax</h1><span class="sub hide-sm"></span><span class="glinks"></span><input type="search" class="search" placeholder="Search artifacts" aria-label="Search artifacts">${THEME_SWITCH_HTML}</header>`;

/** The placeholder in `main` while its content is on its way: a heading
 * and a grid of cards (the gallery), or a heading and lines (a page beside it). */
export const SKELETON_HTML = `<div class="gskel" aria-hidden="true"><div class="sk-h"></div><div class="cards"><div class="sk-c"></div><div class="sk-c"></div><div class="sk-c"></div><div class="sk-c"></div><div class="sk-c"></div><div class="sk-c"></div></div><div class="sk-ls"><div class="sk-l"></div><div class="sk-l"></div><div class="sk-l"></div><div class="sk-l"></div></div></div>`;

/** The footer under every gallery page. */
export const FOOTER_HTML = `<nav class="glegal" aria-label="About Clax"><a href="https://github.com/empathic/clax" target="_blank" rel="noopener">Source on GitHub</a></nav>`;

/** The whole frame, as `#app` holds it in `shell/index.html`. */
export const FRAME_HTML = `${HEADER_HTML}<main class="gal">${SKELETON_HTML}</main>${FOOTER_HTML}`;

/** Mounts the page in place of the static frame's header and `main` in
 * `root` within the one task: `mountAt` mounts before the anchor given (the
 * footer, which stays, or nothing), and the frame's header and `main` go
 * once it has. What the person typed into the frame's search box, and its
 * focus, carry over to the mounted one. */
export function replaceFrame(root: HTMLElement, mountAt: (anchor: Node | undefined) => void): void {
  const footer = root.querySelector(":scope > nav.glegal") ?? undefined;
  const old = [...root.childNodes].filter(n => n !== footer);
  const typed = root.querySelector<HTMLInputElement>(":scope > .gbar > .search");
  const value = typed?.value ?? "";
  const focused = !!typed && root.ownerDocument.activeElement === typed;
  mountAt(footer);
  for (const n of old) n.remove();
  const next = root.querySelector<HTMLInputElement>(":scope > .gbar > .search");
  if (!next || !typed) return;
  if (value) {
    next.value = value;
    next.dispatchEvent(new Event("input", { bubbles: true }));
  }
  if (focused) next.focus();
}
