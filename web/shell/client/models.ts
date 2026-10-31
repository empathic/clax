// @empathic/clax-client/models: the view models a client's UI draws from:
// presence (who is here, and where), agents at work, a thread's history,
// the sidebar's sections, the send menu and unsent batches (the inbox's is
// ./inbox's, which loads lazily). Part of web/shell (AGPL-3.0-only).
export { relativeTime } from "../src/format";
export { presenceMap, roster } from "../src/view/presence-model";
export { agentNames } from "../src/view/working-model";
export { agentName, historyOf } from "../src/view/history-model";
export { unsent } from "../src/view/batch-model";
export { sidebarSections } from "../src/view/sidebar-model";
export { type AskFirsts, firstsVia, ownTarget } from "../src/view/send-menu";
