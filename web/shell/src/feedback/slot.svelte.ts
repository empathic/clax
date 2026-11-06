// Where the rest of the shell finds the feedback integration: empty until
// it (`state.svelte.ts`, loaded only for a person who may report) fills it.
// It is all of the feedback code the headers, the keys sheet and the
// artifact view carry: the menu, the keys sheet's row, and what F pressed in
// the artifact's frame (`clax:shortcut`) does.
import type { Component } from "svelte";

let menu: Component | null = $state(null);
let rows: { keys: string[]; what: string }[] = $state([]);
let onReportKey: (() => void) | null = null;

/** The header's feedback menu once the integration runs, else null. */
export const feedbackMenu = { get current(): Component | null { return menu; } };

/** The keys sheet's row for the report key, while it is bound. */
export function feedbackKeyRows(): { keys: string[]; what: string }[] { return rows; }

/** What F from the artifact's frame does, while the integration runs. */
export const reportKey = { get current(): (() => void) | null { return onReportKey; } };

/** Fills the slot (the integration, once it runs); null empties it. */
export function setFeedbackMenu(c: Component | null, keys: string[] = []): void {
  menu = c;
  rows = c ? [{ keys, what: "Report a problem with Clax" }] : [];
}

/** Sets what F from the artifact's frame does; null unbinds it. */
export function setReportKey(f: (() => void) | null): void { onReportKey = f; }
