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
// search box shows on the gallery only, and the sign-in pages show none
// of the frame.
//
// The skeleton takes the shape of what replaces it. It holds every part
// the content may have, each in the content's own boxes (the groups'
// sections and headings, the cards' grids): the usage banner's place, the
// inbox summary's, the tag bar's, the empty gallery's, the two groups
// with up to `SKELETON_CARDS` cards each, and lines for a page beside
// the gallery. The inline `clax-shape` script (`shell/index.html`) shows
// the parts the page will have, before the first paint, from the daemon's
// bootstrap block where the page has one, else from the shape this
// browser last saw there (`gallery-shape.ts`), else a neutral default.

/** The theme switch's button as `ThemeSwitch.svelte` renders it. */
export const THEME_SWITCH_HTML = `<button type="button" class="icon theme-switch" aria-label="Dark theme" aria-pressed="false" title="Switch to dark"><svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="8" cy="8" r="6.25" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="M8 1.75a6.25 6.25 0 0 1 0 12.5z" fill="currentColor"/></svg></button>`;

/** The header bar: what `Gallery.svelte` renders before the header's answers are in. */
export const HEADER_HTML = `<header class="gbar"><a href="/" class="home" aria-label="Gallery"><span class="mark" data-size="bar" role="img" aria-label="Clax">${MARK_SVG}</span></a><h1>Clax</h1><span class="sub hide-sm"></span><span class="glinks"></span><input type="search" class="search" placeholder="Search artifacts" aria-label="Search artifacts">${THEME_SWITCH_HTML}</header>`;

/** The groups' headings and rule, as the content and the skeleton write them. */
export const NEEDS_HEADING = "Needs your eyes";
export const NEEDS_RULE = "A thread you're in was addressed and you haven't looked, or there's a version or reply you haven't seen.";
export const REST_HEADING = "Artifacts";
export const REST_AFTER_NEEDS = "Everything else";
export const REST_NOTE = "pinned first, then most recent";

/** The most cards a skeleton group shows: more than fill a tall window. */
export const SKELETON_CARDS = 12;

/** What `main` says while its content is on its way (a polite status). */
export const LOADING = "Loading…";

// A skeleton card has the rows of a card with nothing in them (gallery.css
// lays them out in a card's measures): its title and meta line, a marker's
// row (shown under Needs your eyes, where every card has one), and its footer.
const CARD = `<div class="sk-c"><span class="sk-hd"><span class="sk-t">&nbsp;</span><span class="sk-b"><span class="sk-v">v</span></span></span><span class="sk-m"><span class="sk-chip">&nbsp;</span></span><span class="sk-f"></span></div>`;
const cards = `<div class="cards">${CARD.repeat(SKELETON_CARDS)}</div>`;

/** The placeholder in `main` while its content is on its way: a polite
 * status for assistive technology, and the shapes, hidden from it. */
export const SKELETON_HTML = `<div class="gskel"><p class="vh" role="status">${LOADING}</p><div class="sk" aria-hidden="true">`
  + `<div class="sk-ban"></div><div class="sk-sum"></div><div class="sk-tb"></div>`
  + `<div class="sk-empty"><span class="sk-mk"></span><span class="sk-l"></span><span class="sk-l"></span><span class="sk-nt"></span></div>`
  + `<section class="grp needs sk-need"><h2><span class="sw"></span>${NEEDS_HEADING}<small></small></h2><p class="rule">${NEEDS_RULE}</p>${cards}</section>`
  + `<section class="grp rest sk-rest"><h2><span class="sw"></span><span class="sk-a">${REST_HEADING}</span><span class="sk-e">${REST_AFTER_NEEDS}</span><small>${REST_NOTE}</small></h2>${cards}</section>`
  + `<div class="sk-ls"><div class="sk-h"></div><div class="sk-l"></div><div class="sk-l"></div><div class="sk-l"></div><div class="sk-l"></div></div>`
  + `</div></div>`;

/** The footer under every gallery page. */
export const FOOTER_HTML = `<nav class="glegal" aria-label="About Clax"><a href="https://github.com/empathic/clax" target="_blank" rel="noopener">Source on GitHub</a></nav>`;

/** The whole frame, as `#app` holds it in `shell/index.html`; `main` is busy until its content is in. */
export const FRAME_HTML = `${HEADER_HTML}<main class="gal" aria-busy="true">${SKELETON_HTML}</main>${FOOTER_HTML}`;

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
