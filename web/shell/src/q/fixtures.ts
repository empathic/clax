// Test fixtures for the question card and the inbox: a question view with
// one question of each kind, and an inbox item of each kind.
import type { InboxItem, InboxKind, QuestionView } from "../api";

export const AGENT = { handle: "a_1f3a00", harness: "claude", project: "clax" };

/** A question view (open, asked through `ask`, about Quarterly Review): a
 * single choice with previews and a recommended option, a multi choice,
 * and a free-text question; `over` replaces fields. */
export function view(over: Partial<QuestionView> = {}): QuestionView {
  return {
    id: "01J9Q0000000000000000000AA",
    agent: AGENT,
    artifact: { id: "7q3k9mzx2b4t", title: "Quarterly Review", kind: "html" },
    source: "ask",
    status: "open",
    questions: [
      { question: "Which layout should the dashboard use?", header: "Layout", multi_select: false, other: true, options: [
        { label: "Two", description: "Charts left, table right", preview: "|a|b|", recommended: true },
        { label: "One", description: "Everything stacked", preview: "|ab|" },
      ] },
      { question: "Which sides get a sidebar?", header: "Sidebars", multi_select: true, other: false, options: [
        { label: "Left", description: "Navigation" },
        { label: "Right", description: "Details" },
      ] },
      { question: "Anything else?", header: "Notes", multi_select: false, other: true, options: [] },
    ],
    answers: null,
    answered_via: null,
    created_at: "2026-10-07T09:00:00.000Z",
    closed_at: null,
    ...over,
  };
}

const ITEM: Omit<InboxItem, "kind"> = {
  id: "01JA00000000000000000000AA",
  seq: 7,
  read: false,
  created_at: "2026-10-07T09:12:03.120Z",
  agent: AGENT,
  artifact: { id: "7q3k9mzx2b4t", title: "Quarterly Review", kind: "html", page_url: null },
  thread: null, reply: null, version: null, published: null, question: null, work: null,
  gone: false,
  url: "/a/7q3k9mzx2b4t",
};

/** An inbox item of `kind` with its kind's fields filled; `over` replaces fields. */
export function item(kind: InboxKind, over: Partial<InboxItem> = {}): InboxItem {
  const base: InboxItem = { ...ITEM, kind };
  switch (kind) {
    case "reply":
      Object.assign(base, {
        thread: { id: "01J9T0000000000000000000AA", summary: "main > h2 «Quarterly goals»", status: "open" },
        reply: { comment_id: "01J9C0000000000000000000AA", body: "Done: two columns now.\n\nThe table\tsorts too.", addressed: false },
        url: "/a/7q3k9mzx2b4t?thread=01J9T0000000000000000000AA",
      });
      break;
    case "version":
      Object.assign(base, {
        version: { n: 4, note: "Two columns, sortable table", addressed: [{ id: "t1", summary: "h2" }, { id: "t2", summary: "table" }] },
        url: "/a/7q3k9mzx2b4t/v/4",
      });
      break;
    case "published":
      Object.assign(base, {
        artifact: { id: "9x8w7v6u5t4s", title: "Sales dashboard", kind: "html", page_url: null },
        published: { description: "Weekly sales by region" },
        url: "/a/9x8w7v6u5t4s",
      });
      break;
    case "question":
      Object.assign(base, { question: view(), url: `/inbox?q=${view().id}` });
      break;
    case "finished":
      Object.assign(base, { work: { message: "Charts are in.", threads: [] } });
      break;
  }
  return { ...base, ...over };
}
