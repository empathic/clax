import type { FeedbackState, Tier } from "./threads";

// Both maps cover every tier so the types stay total, but the daemon emits
// only some pairs: `sent` names the tier the target session waits on, which is
// never `wait` or `prompt_hook`, and a `piggyback` or `wait` delivery is
// acknowledged at once, so it is never `delivered` via those. The
// `wait`/`prompt_hook` entries of WAITING_ON and the `piggyback`/`wait`
// entries of DELIVERED_VIA exist for type completeness only.
const WAITING_ON: Record<Tier, string> = {
  piggyback: "its next artifax tool call",
  stop_hook: "the end of its turn",
  prompt_hook: "your next message to it",
  wait: "its wait_for_feedback loop",
  queue: "Codex to pick up the queued message",
  inject: "Pi to take the message",
};
const DELIVERED_VIA: Record<Tier, string> = {
  piggyback: "a tool result",
  stop_hook: "the Stop hook",
  prompt_hook: "your next prompt",
  wait: "wait_for_feedback",
  queue: "codex queue",
  inject: "a Pi message",
};

export function elapsed(since: string, now: Date): string {
  const s = Math.max(0, Math.floor((now.getTime() - new Date(since).getTime()) / 1000));
  if (s < 60) return `${s} s`;
  if (s < 3600) return `${Math.floor(s / 60)} min ${s % 60} s`;
  return `${Math.floor(s / 3600)} h ${Math.floor((s % 3600) / 60)} min`;
}

/** The waiting indicator for a thread's feedback state; `null` when it was never sent. */
export function waitingLabel(s: FeedbackState | null, now: Date): string | null {
  if (!s) return null;
  switch (s.state) {
    case "sent":
      return `sent, waiting for the agent · ${elapsed(s.since, now)} · waiting on ${WAITING_ON[s.tier ?? "piggyback"]}`;
    case "delivered":
      if (s.exhausted) return "delivered, not acknowledged";
      return `delivered via ${DELIVERED_VIA[s.tier ?? "piggyback"]} · ${elapsed(s.since, now)} ago · not yet acknowledged${s.resends === 1 ? " · resent once" : s.resends ? ` · resent ${s.resends} times` : ""}`;
    case "acknowledged":
      return "seen by the agent";
    case "agent_ended":
      return "agent session ended; waiting for a new one";
  }
}

/** Whether the state's label shows elapsed time, so it needs a clock tick. */
export function hasElapsedLabel(s: FeedbackState | null): boolean {
  return !!s && (s.state === "sent" || (s.state === "delivered" && !s.exhausted));
}
