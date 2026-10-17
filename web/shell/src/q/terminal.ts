// The words of an in-terminal question's card and links (spec
// 2026-10-16-ask-where-attention-is-design §3). Apart from `model`, which
// loads with the question module, as only the parts that load on demand
// (the terminal card, the sidebar's links to threads) use them.
import type { QAgent, QuestionView } from "../api";
import { agentLabel } from "./model";

/** An in-terminal question's line: who is asking in the terminal. */
export const terminalLine = (q: QuestionView, others: (QAgent | null)[]): string => `${agentLabel(q.agent, others)} is asking you in the terminal`;

/** What Ask me here cannot do for Claude Code's terminal dialog. */
export const TERMINAL_ESC = "After Ask me here, the terminal's dialog stays open until you press Esc there.";
