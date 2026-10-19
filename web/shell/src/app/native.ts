// The overlays the Clax app draws natively (spec 2026-10-18-professional-
// ui-design §6): loaded by `host.ts` once the app's `init` names them in
// `native`, outside the inspect view, so a browser and an app that names
// none load none of it. The stage draws no web version of a named overlay
// (`ViewState.native`); this module posts it to the app instead:
//
// - `ask`: the prompt at the head of the view's prompt queue (`ask` named),
//   with its ID, kind, words, "always" label and `armMs`; `askAnswer(id,
//   answer)` answers it (`always` is an Allow, which each prompt's path
//   keeps as it keeps any Allow); an answer for any other ID, or after
//   `askCancel`, changes nothing. `askCancel {id}` goes when the prompt
//   ends without the app's answer: its page went away (another version),
//   or the view is unloading.
// - `notice` (`notice` named): each shell notice as `{text, tone, action?}`;
//   every shell notice says something failed (`tone: "error"`). A failed
//   follow of the latest version carries Try again, which `noticeAction`
//   runs.
// - `draft` (`draft` named): what the drafts' banner would say, as
//   `{kind: "drafting", by, note?}` or `{kind: "showing", by, base,
//   detail}`, or null; `showDraft(by?)` and `hideDraft()` are its Show
//   draft and Back. With several agents drafting it names the one that
//   began first.
// - `gone`, once, when the artifact is deleted.
//
// Questions (`questions` named) stay in `host.ts`, which tracks the open
// IDs for the `questions` message.
import type { PromptAnswer } from "../caps/grants";
import type { ArtifactController, ViewState } from "../view/artifact-controller";
import { bannerText } from "../view/draft-model";
import { ALLOW_DELAY_MS, type Ask } from "../view/prompt-queue";
import { agentNames, harnessName } from "../view/working-model";
import type { DraftState, HostMessage, NativeOverlay } from "./host";

export type NativeHost = {
  win: Window;
  ctl: ArtifactController;
  /** Posts on the app's channel, with the nonce. */
  post(m: HostMessage): void;
  native: readonly NativeOverlay[];
  /** Loads the latest version again (the app's `reloadLatest`). */
  retry(): void;
  /** The text a failed follow's notice starts with. */
  followFailed: string;
};

/** The calls of `window.claxHost` this module answers (`host.ts` forwards them). */
export type NativeOverlays = {
  askAnswer(id: unknown, answer: unknown): void;
  noticeAction(id: unknown): void;
  showDraft(by?: unknown): void;
  hideDraft(): void;
  dispose(): void;
};

const ANSWERS: Record<string, PromptAnswer> = { allow: "allow", always: "allow", deny: "deny", dismiss: "dismiss" };
/** The ID of the follow notice's Try again. */
const RETRY = "retry";

export function nativeOverlays(h: NativeHost): NativeOverlays {
  const { ctl, post } = h;
  const on = (k: NativeOverlay) => h.native.includes(k);
  // The prompt posted as `ask`, until the app answers it or it is cancelled.
  let asked: Ask | null = null;
  let lastNotice: string | null = null;
  let lastDraft = "null";
  let gone = false;

  const cancelAsk = () => {
    if (!asked) return;
    post({ type: "askCancel", id: asked.id });
    asked = null;
  };
  /** Handle → the name the banner gives it ("Clax" for the app's agent). */
  const namer = (s: ViewState) => {
    const names = agentNames(s.working, s.agents);
    return (handle: string, harness: string) => names.get(handle) ?? harnessName(harness);
  };
  const draftOf = (s: ViewState): DraftState | null => {
    if (s.deleted) return null;
    const t = bannerText(s.drafts, s.showDraft, s.draftChanged, ctl.shown(s), namer(s));
    if (!t) return null;
    if (t.kind === "showing") return { kind: "showing", by: t.who, base: t.base, detail: t.detail };
    if (t.kind === "many") return { kind: "drafting", by: t.agents[0].who };
    return { kind: "drafting", by: t.who, ...(t.note ? { note: t.note } : {}) };
  };

  const onState = (v: ViewState) => {
    if (on("ask") && v.ask !== asked) {
      cancelAsk();
      if (v.ask) {
        asked = v.ask;
        const p = v.ask.prompt;
        post({ type: "ask", id: v.ask.id, kind: p.kind, title: p.title, body: p.body, allow: p.allow, deny: p.deny, ...(p.always ? { always: p.always } : {}), armMs: ALLOW_DELAY_MS });
      }
    }
    if (on("notice") && v.notice !== lastNotice) {
      lastNotice = v.notice;
      if (v.notice) post({ type: "notice", text: v.notice, tone: "error", ...(v.notice.startsWith(h.followFailed) ? { action: { id: RETRY, label: "Try again" } } : {}) });
    }
    if (on("draft")) {
      const state = draftOf(v);
      const json = JSON.stringify(state);
      if (json !== lastDraft) {
        lastDraft = json;
        post({ type: "draft", state });
      }
    }
    if (v.deleted && !gone) {
      gone = true;
      post({ type: "gone" });
    }
  };
  const unsubscribe = ctl.state.subscribe(onState);
  // The view is unloading: the prompt shown will never be answered here.
  const onHide = () => cancelAsk();
  h.win.addEventListener("pagehide", onHide);

  return {
    askAnswer: (id, answer) => {
      const a = typeof answer === "string" && Object.hasOwn(ANSWERS, answer) ? ANSWERS[answer] : null;
      if (!asked || id !== asked.id || !a) return;
      const ask = asked;
      asked = null;
      ask.answer(a);
    },
    noticeAction: id => {
      if (id === RETRY && ctl.state.get().notice?.startsWith(h.followFailed)) h.retry();
    },
    showDraft: by => {
      const s = ctl.state.get();
      const name = namer(s);
      const d = s.drafts.find(x => x.agent === by || name(x.agent, x.harness) === by) ?? s.drafts[0];
      if (d) ctl.showDraftOf(d.agent);
    },
    hideDraft: () => ctl.backFromDraft(),
    dispose: () => {
      unsubscribe();
      h.win.removeEventListener("pagehide", onHide);
      cancelAsk();
    },
  };
}
