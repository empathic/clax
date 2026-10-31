// @empathic/clax-client/ui: the shell's components a client's side panel
// shows at once (the thread sidebar, thread cards, the send button) and the
// reactive state they share (`ticker`, `QuestionFeed`). Each component's
// module runs on load, so what a client loads lazily has an entry of its
// own: ./inbox, ./composer, ./roster and ./send-menu. Part of web/shell
// (AGPL-3.0-only).
export { default as Sidebar } from "../src/ui/Sidebar.svelte";
export { default as ThreadCard } from "../src/ui/ThreadCard.svelte";
export { default as SendButton } from "../src/ui/SendButton.svelte";
export { ticker } from "../src/ui/ticker.svelte";
export { QuestionFeed } from "../src/q/feed.svelte";
