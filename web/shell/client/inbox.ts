// @empathic/clax-client/inbox: the inbox's and the page questions' rows and
// lists, and the inbox's model. Apart from ./ui and ./models because a
// client loads these lazily (the extension's Inbox tab and its page
// questions are their own chunks), and a component's module runs on load.
// Part of web/shell (AGPL-3.0-only).
export { default as InboxRow } from "../src/q/InboxRow.svelte";
export { default as QuestionList } from "../src/q/QuestionList.svelte";
export { approvalOf, newestSeq } from "../src/q/inbox-model";
