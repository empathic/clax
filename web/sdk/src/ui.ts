// Clax's UI kit (`@empathic/clax-sdk/ui` in Clax's own code: scripts/
// sdk-entries.mjs aliases it; not yet in the published package, whose
// declarations cannot come from .svelte files). Components styled only
// through Clax's design tokens (web/sdk/tokens/clax.tokens.json), so a
// host's token file restyles them (the overlay's `theme`). The overlay
// draws its composer, hint, panel, quotes and threads with them.
export { default as Surface } from "../ui/Surface.svelte";
export { default as Quote } from "../ui/Quote.svelte";
export { default as Thread } from "../ui/Thread.svelte";
export type { ThreadComment } from "../ui/types";
