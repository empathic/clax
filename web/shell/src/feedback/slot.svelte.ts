// Where the shell's headers find the feedback menu: empty until the
// integration (`state.svelte.ts`, loaded only for a person who may report)
// fills it. It is all of the feedback code the headers carry.
import type { Component } from "svelte";

let menu: Component | null = $state(null);

/** The header's feedback menu once the integration runs, else null. */
export const feedbackMenu = { get current(): Component | null { return menu; } };

/** Fills the slot (the integration, once it runs). */
export function setFeedbackMenu(c: Component | null): void { menu = c; }
