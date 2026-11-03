import type { Plugin } from "vite";

/** The oldest browsers the overlay's builds target (overlay-component-css.mjs). */
export const OVERLAY_TARGET: string[];
/** The string literal in overlay/src/main.ts that the components' CSS replaces (overlay-component-css.mjs). */
export const CSS_SLOT: string;
/** Moves the overlay's components' CSS into the chunk `fileName` (or the one of several names an output holds) and emits no .css file (overlay-component-css.mjs). */
export function componentCssIntoScript(fileName: string | readonly string[]): Plugin;
