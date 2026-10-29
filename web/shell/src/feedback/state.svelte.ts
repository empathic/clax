// The shell's feedback entry points (spec 2026-10-29-feedback-loop §10.3),
// loaded with each entry: it asks `GET /_app/feedback/config` first thing,
// and only for a person who may report does it load the integration
// (`feedback.ts`, its own chunk), the overlay from the instance, and the
// header's menu (`FeedbackMenu.svelte`). For anyone else that one request is
// all: no menu, no shortcut, no overlay.
import type { Component } from "svelte";
import { type AppView, loadConfig } from "./config";
import type { Feedback } from "./feedback";

let running: Feedback | null = $state(null);
let count = $state(0);
let menu: Component | null = $state(null);
let started = false;

/** The running integration once it has started, else null. */
export const feedback = { get current(): Feedback | null { return running; } };
/** How many of the person's reports wait on them. */
export const badge = { get current(): number { return count; } };
/** The header's feedback menu once the integration runs, else null. */
export const feedbackMenu = { get current(): Component | null { return menu; } };

/** The keys sheet's row for the report shortcut, while it is bound. */
export function feedbackKeyRows(): { keys: string[]; what: string }[] {
  return running ? [{ keys: running.shortcut.keys, what: "Report a problem with Clax" }] : [];
}

/** Starts the shell's feedback, once per page: `where` names the view a
 * capture starts in (default from the page's address). A failure at any
 * step leaves the shell without it. */
export function bootFeedback(where?: () => AppView): void {
  if (started) return;
  started = true;
  void loadConfig().then(async cfg => {
    if (!cfg) return;
    const [fb, M] = await Promise.all([import("./feedback").then(m => m.startFeedback(cfg, { where })), import("./FeedbackMenu.svelte")]);
    fb.onBadge(n => { count = n; });
    running = fb;
    menu = M.default;
  }).catch(e => { console.warn("Clax feedback is off:", e); });
}
