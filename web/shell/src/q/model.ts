// The question card's model (spec 2026-10-06-agent-questions-and-inbox
// §5.3, §9.1): the person's draft answers, when each question is answered,
// the body the owner route takes, which preview to show, and the card's
// names for agents and closed states. Pure: no DOM.
import type { AnswerBody, QAgent, QuestionSpec, QuestionView } from "../api";

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

/** An agent's name: its harness, with the first four hex digits of its
 * handle when another agent of the same harness is among `others`. */
export function agentLabel(agent: QAgent | null, others: (QAgent | null)[]): string {
  if (!agent) return "an agent";
  const twin = others.some(o => o && o.harness === agent.harness && o.handle !== agent.handle);
  return twin ? `${agent.harness} ${agent.handle.replace(/^a_/, "").slice(0, 4)}` : agent.harness;
}

/** What a closed question says happened. */
export function closedLabel(q: QuestionView): string {
  switch (q.status) {
    case "answered": return q.answered_via === "terminal" ? "Answered in the terminal" : "Answered";
    case "declined": return "Skipped";
    case "released": return "Moved to the terminal";
    case "withdrawn": return `${agentLabel(q.agent, [])} stopped waiting`;
    default: return "Open";
  }
}

/** A header for its chip: one longer than 12 characters is cut with "…". */
export function cutHeader(h: string): string {
  const cs = [...h];
  return cs.length > HEADER ? `${cs.slice(0, HEADER - 1).join("").trimEnd()}…` : h;
}
