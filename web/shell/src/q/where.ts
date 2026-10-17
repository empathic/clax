// The parts of the question surfaces that load on demand (spec
// 2026-10-16-ask-where-attention-is-design §2, §3): the terminal card, the
// sidebar's links to questions in threads, and the thread cards' slots.
// One module, so the surfaces that load it carry one import of it.
export { default as TerminalCard } from "./TerminalCard.svelte";
export { default as ThreadLinks } from "./ThreadLinks.svelte";
export { followSlots } from "./thread-slots";
