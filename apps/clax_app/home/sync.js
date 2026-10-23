// The sync badges of a published app's tile (spec 2026-10-21-app-push-pull
// §5.4): how the app stands against its public copy, by `app.sync` as the
// home capability's apps() gives it.

/** Each badge: its glyph, whether it is warm (`you`, the update's alone),
 * and what a screen reader says. */
export const SYNC_BADGES = Object.freeze({
  live: Object.freeze({ glyph: '☁', warm: false, words: 'live' }),
  ahead: Object.freeze({ glyph: '↑', warm: false, words: 'newer here' }),
  update: Object.freeze({ glyph: '↓', warm: true, words: 'update waiting' }),
  diverged: Object.freeze({ glyph: '⑂', warm: false, words: 'changed in both places' }),
  fork: Object.freeze({ glyph: '⑂', warm: false, words: 'changed here' }),
});

/** Each badge's glyph drawn as a 24-unit inline SVG in currentColor, so no
 * font has to hold ☁ ↑ ↓ ⑂. */
const DRAWN = Object.freeze({
  live: '<path fill="currentColor" d="M7 19h10.5a4.5 4.5 0 0 0 .6-8.96A6.5 6.5 0 0 0 5.6 9.1 5 5 0 0 0 7 19Z"/>',
  ahead: '<path fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round" d="M12 19V5m-6 6 6-6 6 6"/>',
  update: '<path fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round" d="M12 5v14m-6-6 6 6 6-6"/>',
  diverged: '<path fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round" d="M12 20v-7L6 6m6 7 6-7M6 6v4M6 6h4m8 0v4m0-4h-4"/>',
});
const drawn = (sync) => DRAWN[sync === 'fork' ? 'diverged' : sync];

/** `sync`'s badge glyph as an SVG element's markup, or '' for none. */
export const syncSvgOf = (sync) =>
  syncBadgeOf(sync) ? `<svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" aria-hidden="true" focusable="false">${drawn(sync)}</svg>` : '';

/** `sync`'s badge, or null for none (an app only here, or kept mine, or a
 * value this page does not know). */
export const syncBadgeOf = (sync) =>
  typeof sync === 'string' && Object.prototype.hasOwnProperty.call(SYNC_BADGES, sync) ? SYNC_BADGES[sync] : null;
