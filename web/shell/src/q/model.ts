// The question card's model (spec 2026-10-06-agent-questions-and-inbox
// §5.3, §9.1): the person's draft answers, when each question is answered,
// the body the owner route takes, which preview to show, and the card's
// names for agents and closed states. Pure: no DOM.
import type { AnswerBody, QAgent, QOption, QuestionSpec, QuestionView } from "../api";
import { shownName } from "../view/agent-display";

/** One entry per question: the picked labels (in option order) and the
 * "Other" or free text as typed. */
export type Draft = { selected: string[]; text: string }[];

/** The longest header shown whole; a longer (mirrored) one is cut. */
const HEADER = 12;

export const emptyDraft = (q: QuestionView): Draft => q.questions.map(() => ({ selected: [], text: "" }));

const answered = (s: QuestionSpec, a: Draft[number]): boolean => {
  const text = a.text.trim() !== "";
  if (!s.options.length) return text;
  return a.selected.length > 0 || (s.other && text);
};

/** Whether each question has an answer the daemon takes. */
export const complete = (q: QuestionView, d: Draft): boolean[] => q.questions.map((s, i) => answered(s, d[i]));

/** The answer body for `d`: text trimmed, and null on a choice question
 * when blank (a single choice sends one of a label and its text). */
export function toBody(q: QuestionView, d: Draft): AnswerBody {
  return {
    answers: q.questions.map((s, i) => {
      const text = d[i].text.trim();
      if (!s.options.length) return { selected: [], text };
      if (!s.multi_select && d[i].selected.length) return { selected: [...d[i].selected], text: null };
      return { selected: [...d[i].selected], text: s.other && text ? text : null };
    }),
  };
}

const replace = (d: Draft, i: number, a: Draft[number]): Draft => d.map((x, j) => (j === i ? a : x));

/** Picks `label` on question `i`: a single choice takes it alone and drops
 * the "Other" text; a multi choice toggles it, keeping option order. */
export function pick(q: QuestionView, i: number, label: string, d: Draft): Draft {
  const s = q.questions[i];
  if (!s.options.some(o => o.label === label)) return d;
  if (!s.multi_select) return replace(d, i, { selected: [label], text: "" });
  const on = new Set(d[i].selected);
  if (on.has(label)) on.delete(label); else on.add(label);
  return replace(d, i, { selected: s.options.map(o => o.label).filter(l => on.has(l)), text: d[i].text });
}

/** Types the "Other" text (or a free-text answer) of question `i`: on a
 * single choice it replaces the picked option. */
export function typeOther(q: QuestionView, i: number, text: string, d: Draft): Draft {
  const s = q.questions[i];
  return replace(d, i, { selected: s.multi_select ? d[i].selected : [], text });
}

/** The preview beside `s`'s options: the focused option's, else the
 * selected one's, else the first option's. Null when no option has a
 * preview; "" when the option shown has none. */
export function previewOf(s: QuestionSpec, focused: string | null, a: Draft[number]): string | null {
  if (!s.options.some(o => o.preview !== undefined)) return null;
  const label = focused ?? a.selected[0] ?? s.options[0].label;
  return s.options.find(o => o.label === label)?.preview ?? "";
}

/** An agent's name: its agent name, else its harness, with the first four
 * hex digits of its handle when another agent of the same harness is
 * among `others`. */
export function agentLabel(agent: QAgent | null, others: (QAgent | null)[]): string {
  if (!agent) return "an agent";
  if (agent.name) return shownName(agent.name, agent.via);
  const twin = others.some(o => o && o.harness === agent.harness && o.handle !== agent.handle);
  return twin ? `${agent.harness} ${agent.handle.replace(/^a_/, "").slice(0, 4)}` : agent.harness;
}

/** The thread a question shows in, inside that thread's card: its thread
 * while it is open; null when it shows in the sidebar's block instead (no
 * thread, or a resolved or deleted one). */
export const inThread = (q: QuestionView): string | null => (q.thread?.open ? q.thread.id : null);

/** A card's link: to its thread (`?thread=`, "in a thread") while that is
 * open and the card is not in the thread's own card (`inCard`), and named
 * by its artifact's title unless that is the artifact shown (`here`); null
 * when it needs neither. */
export function aboutLink(q: QuestionView, here?: string | null, inCard?: string | null): { href: string; text: string } | null {
  const a = q.artifact, t = q.thread?.open && q.thread.id !== inCard && q.thread.id;
  return a && (a.id !== here || t) ? { href: `/a/${a.id}${t ? `?thread=${t}` : ""}`, text: a.id === here ? "in a thread" : t ? `${a.title} · in a thread` : a.title } : null;
}

/** What a closed question says happened. */
export function closedLabel(q: QuestionView): string {
  switch (q.status) {
    case "answered": return q.answered_via === "terminal" ? "Answered in the terminal" : "Answered";
    case "declined": return "Skipped";
    case "released": return "Moved to the terminal";
    case "withdrawn": return q.pulled ? `Sent to ${agentLabel(q.agent, [])}: it asks here next` : `${agentLabel(q.agent, [])} stopped waiting`;
    default: return "Open";
  }
}

/** The tag on a question asked on a remote too (linked questions): the
 * remote's name, its title, and the line saying how its copy stands: while
 * open, sending or not sent; once closed, "on <remote>" when the answer or
 * decline came from there. Null for none. */
export function questionLink(q: QuestionView): { label: string; title: string; line: string | null; failed: boolean } | null {
  const l = q.linked;
  if (!l) return null;
  if (l.unlinked) return { label: `${l.remote} · unlinked`, title: `Was also asked on ${l.remote}; no longer linked`, line: null, failed: false };
  const title = `Also asked on ${l.remote}`;
  if (q.status === "open") {
    if (l.state === "pending") return { label: l.remote, title, line: `Sending to ${l.remote}…`, failed: false };
    if (l.state === "failed") return { label: l.remote, title, line: `Not sent: ${l.remote} unreachable`, failed: true };
    return { label: l.remote, title, line: null, failed: false };
  }
  // Closed: the closed label says "on <remote>" when the answer came from there.
  return { label: l.remote, title, line: l.answered_there ? `on ${l.remote}` : null, failed: false };
}

/** A header for its chip: one longer than 12 characters is cut with "…". */
export function cutHeader(h: string): string {
  const cs = [...h];
  return cs.length > HEADER ? `${cs.slice(0, HEADER - 1).join("").trimEnd()}…` : h;
}

/** Answer `a` to question `s`, as the person reads it: the picked labels
 * as shown, then "Other: …" or the free text; "" for none. */
export function answerLine(s: QuestionSpec, a: { selected: string[]; text: string | null } | undefined): string {
  const parts = [...(a?.selected ?? []).map(l => { const o = s.options.find(x => x.label === l); return o ? shownLabel(o) : l; }), ...(a?.text ? [s.options.length ? `Other: ${a.text}` : a.text] : [])];
  return parts.join(", ");
}

/** The answer to question `i` of `q` (see `answerLine`); "—" for none. */
export function answerText(q: QuestionView, i: number): string {
  return answerLine(q.questions[i], q.answers?.[i]) || "—";
}

/** The first question after `from` (wrapping round) that `done` marks
 * unanswered; -1 when every one is answered. `from` -1 starts at the first. */
export function nextOpen(done: boolean[], from: number): number {
  for (let k = 1; k <= done.length; k++) {
    const i = (from + k) % done.length;
    if (!done[i]) return i;
  }
  return -1;
}

/** An option's label as shown: a recommended option's label loses a
 * trailing "(Recommended)" (any case), which a mirrored label keeps, since
 * the card shows a Recommended chip. The stored label, which keys the
 * answer, is unchanged. */
export function shownLabel(o: QOption): string {
  return (o.recommended && o.label.replace(/\s*\(recommended\)\s*$/i, "")) || o.label;
}
