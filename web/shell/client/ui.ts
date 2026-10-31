// @empathic/clax-client/ui: the shell's components a client's side panel
// shows (the thread sidebar, thread cards, the send button, the inbox's rows
// and question lists) and the reactive state they share. Each component's
// module runs on load, so a page that needs only the composer imports
// @empathic/clax-client/composer. Part of web/shell (AGPL-3.0-only).
export { default as Sidebar } from "../src/ui/Sidebar.svelte";
export { default as ThreadCard } from "../src/ui/ThreadCard.svelte";
export { default as SendButton } from "../src/ui/SendButton.svelte";
export { ticker } from "../src/ui/ticker.svelte";
export { default as InboxRow } from "../src/q/InboxRow.svelte";
export { default as QuestionList } from "../src/q/QuestionList.svelte";
export { QuestionFeed } from "../src/q/feed.svelte";
