// @empathic/clax-client/sync: Clax's data layer as Clax's own clients (the
// Chrome extension's worker and panel) use it: threads and their comments,
// the daemon's API types, the inbox query, and how stream deltas and
// fetches fold into what a client holds. Free of Svelte and the DOM, so a
// service worker can load it. Part of web/shell (AGPL-3.0-only): a page
// outside Clax uses @empathic/clax-sdk instead. The package's entries are
// scripts/client-entries.mjs's.
export { type FeedbackState, resolvedByLabel, type Thread, upsert, type Viewer } from "../src/threads";
export {
  type AnswerBody, ApiError, type Artifact, type InboxFilter, type InboxItem, type InboxKind, type InboxPage, type InboxSummary, type Participants,
  type QuestionView, type Version,
} from "../src/api";
export { inboxQuery } from "../src/q/api";
export { applyPresence, applyThread, type ThreadDelta } from "../src/view/deltas";
export { type ThreadChange, ThreadSync } from "../src/view/thread-sync";
export type { PresenceView } from "../src/view/presence-model";
export type { Working } from "../src/view/working-model";
export type { VersionTag } from "../src/view/history-model";
