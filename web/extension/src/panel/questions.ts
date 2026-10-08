// The side panel's questions (spec 2026-10-06-agent-questions-and-inbox
// §9.6): the shell's question feed, which keeps the open questions and
// lets a closed one say what closed it for a moment, filled from the
// worker's messages instead of the shell's stream, and answering through
// the worker, which alone talks to the daemon.
import { ApiError, type AnswerBody, type QuestionView } from "../../../shell/src/api";
import { QuestionFeed } from "../../../shell/src/q/feed.svelte";
import type { PanelToWorker, WorkerToPanel } from "../messages";

type Close<M = Extract<PanelToWorker, { t: "q-answer" | "q-decline" | "q-release" }>> = M extends unknown ? Omit<M, "req"> : never;
export type Asker = { ask(m: Close): Promise<WorkerToPanel> };
/** Failures that say Clax did not answer, rather than that it refused. */
const UNANSWERED = new Set(["daemon_unreachable", "worker_restarted", "timeout"]);

export class PanelQuestions extends QuestionFeed {
  constructor(private readonly via: Asker, opts: ConstructorParameters<typeof QuestionFeed>[0] = {}) { super(opts); }

  override answer(id: string, b: AnswerBody): Promise<void> { return this.closing({ t: "q-answer", questionId: id, body: b }); }
  override decline(id: string): Promise<void> { return this.closing({ t: "q-decline", questionId: id }); }
  override release(id: string): Promise<void> { return this.closing({ t: "q-release", questionId: id }); }

  /** The open questions as the worker last fetched them: one closing here keeps its card. */
  list(qs: QuestionView[]): void {
    this.open = qs.filter(q => q.status === "open" && !this.recent.some(x => x.id === q.id)).sort((a, b) => a.created_at.localeCompare(b.created_at));
  }

  private async closing(m: Close): Promise<void> {
    let r: WorkerToPanel;
    try {
      r = await this.via.ask(m);
    } catch (e) {
      // The card says why: a refusal in the daemon's words, else that Clax did not answer.
      const err = e as { code?: string; message?: string };
      throw new ApiError(err.code && !UNANSWERED.has(err.code) ? 400 : 0, err.message ?? String(e), err.code ?? null);
    }
    if (r.t === "q-done") this.upsert(r.question);
  }
}
