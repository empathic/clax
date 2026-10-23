// ClaxBot in the artifact view (spec 2026-10-21-claxbot §6.4, §11.2, §11.3):
// what the top bar's ClaxBot button and run panel (`ArtifactBot`, loaded
// with the roster after the first paint) and the thread sidebar's Send
// (`ShellSidebar`) share. ClaxBot is listed to the Send menu as a built-in
// agent, first, live or with why it is not; choosing it is remembered per
// artifact as any agent is (`rememberTarget`), and a Send to it starts a
// run of the threads sent (`POST /api/bot/runs`), whose panel then opens.
import type { MeBot } from "./api";
import { availability } from "./model";
import { refusal } from "../people/refusal";
import { rememberTarget } from "../view/send-target";
import type { AgentView } from "../view/working-model";

/** ClaxBot's handle, as the daemon names it and a Send targets it. */
export const BOT = "claxbot";

/** What the view knows of ClaxBot for this artifact. */
class BotView {
  /** ClaxBot as the Send menu lists it, with its availability line; null where the daemon has none. */
  agent = $state<AgentView | null>(null);
  /** The caller's ClaxBot (`GET /api/me/bot`). */
  me = $state<MeBot | null>(null);
  /** Whether this viewer chose ClaxBot as the artifact's Send target. */
  chosen = $state(false);
  /** The run the panel shows, or null when it is closed. */
  run = $state<string | null>(null);
  /** Runs started from this browser since the view opened, newest first. */
  started = $state<string[]>([]);
  /** Counts the changes of state of the run shown, so the runs listed are read again. */
  changes = $state(0);
  /** The artifact's threads ClaxBot has taken (the threads of its runs):
   * sent, though not to a local agent, so never counted as unsent. */
  claimed = $state<string[]>([]);

  /** Lists ClaxBot (`live`, else unavailable for `reason`) and reads the choice remembered for `aid`. */
  list(aid: string, live: boolean, reason: MeBot["unavailable_reason"], me: MeBot | null): void {
    this.me = me;
    this.agent = { handle: BOT, harness: BOT, name: "ClaxBot", live, builtin: true, line: availability(live, reason, me) };
    let last: string | null = null;
    try { last = localStorage.getItem(`clax.sendTo.${aid}`); } catch { /* none */ }
    this.chosen = live && last === BOT;
  }
  /** Chooses ClaxBot (or another agent, `on` false) for `aid`'s Sends. */
  choose(aid: string, on: boolean): void {
    this.chosen = on;
    if (on) rememberTarget(aid, BOT);
  }
  /** Opens the panel on run `id`. */
  open(id: string): void {
    this.run = id;
  }
  close(): void { this.run = null; }
}
export const botView = new BotView();

/** The artifact's agents with ClaxBot first, when it is listed. */
export const withBot = (agents: AgentView[]): AgentView[] => (botView.agent ? [botView.agent, ...agents.filter(a => a.handle !== BOT)] : agents);

/** What a Send to ClaxBot needs of the view. */
export type SendHost = { readonly id: string; setNotice(text: string | null): void };

/** Starts a ClaxBot run of threads `ids` of `host`'s artifact, acting for
 * this viewer, and opens its panel; where a run already works on one of
 * them (409 `run_active`) that run's panel opens instead, said in the
 * notice; a refusal shows in the view's notice. True once a run is open. */
export async function sendToBot(host: SendHost, ids: string[]): Promise<boolean> {
  if (!ids.length) return false;
  try {
    const { startRun } = await import("./api");
    const { run, active } = await startRun(host.id, ids);
    host.setNotice(active ? "ClaxBot is already working on this thread: its run is open." : null);
    botView.started = [run.id, ...botView.started.filter(x => x !== run.id)];
    botView.claimed = [...new Set([...botView.claimed, ...ids])];
    botView.open(run.id);
    return true;
  } catch (e) {
    host.setNotice(`ClaxBot did not start: ${refusal(e)}`);
    return false;
  }
}
