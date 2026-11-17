// What the page's own requests tell of reaching Clax (`proofFetch` hears
// every request to the page's origin, `owner-proof-install.ts`). A request
// that found no Clax (offline, no answer, an answer that is not Clax's)
// holds the connection notice with that kind's words; any answer of Clax's
// lets it go. While it is held, a cheap route that needs no session
// (`PROBE_PATH`) is asked with backoff on the shell's clock until Clax
// answers, so the notice clears by itself; the asking stops while the page
// is hidden and starts again when it is shown. A write still waiting after
// `STUCK_MS` says Clax is not answering yet, without ending it.
import { after } from "./clock";
import { connTrouble, noticeFor, STILL_WAITING } from "./conn-notice";
import { classify, answered, type Trouble } from "./trouble";
import { STUCK_MS, backoff } from "./lifecycle";
import { visibility } from "./visibility";

/** The route asked while Clax is not answering: open on a local and a hosted daemon. */
export const PROBE_PATH = "/healthz";
/** The label of the wait before the next ask (`claxTestClock.dueIn`). */
export const PROBE_LABEL = "reach-probe";

let down: Trouble | null = null;
let tries = 0;
let cancel: (() => void) | undefined;
let watching = false;
let waits = 0;

const hidden = () => typeof document === "undefined" || visibility.hidden(document);

/** Asks `PROBE_PATH` after the next backoff, unless an ask is pending, the
 * page is hidden, or Clax answers. */
function arm(): void {
  if (cancel || down === null || hidden()) return;
  cancel = after(backoff(tries++), () => { void probe(); }, PROBE_LABEL);
}

async function probe(): Promise<void> {
  const ac = new AbortController();
  const stop = after(STUCK_MS, () => ac.abort());
  const t = await fetch(PROBE_PATH, { cache: "no-store", signal: ac.signal }).then(r => answered(r, PROBE_PATH), e => classify(e));
  stop();
  cancel = undefined;
  heard(t ?? null);
}

function watch(): void {
  if (watching || typeof document === "undefined") return;
  watching = true;
  document.addEventListener("visibilitychange", () => {
    if (hidden()) { cancel?.(); cancel = undefined; } else arm();
  });
  addEventListener("online", () => { if (down !== null) { cancel?.(); cancel = undefined; tries = 0; arm(); } });
}

/** A request to the page's origin found no Clax for reason `t`, or (null)
 * Clax answered it. Kinds other than offline, unreachable and not_clax
 * are Clax's own answers. */
export function heard(t: Trouble | null): void {
  if (t !== null && t !== "offline" && t !== "unreachable" && t !== "not_clax") t = null;
  if (t === null) {
    connTrouble("part", false, "");
    if (down === null) return;
    down = null;
    tries = 0;
    cancel?.();
    cancel = undefined;
    connTrouble("api", false, "");
    return;
  }
  down = t;
  const n = noticeFor(t);
  connTrouble("api", true, n.text, document, n.rank);
  watch();
  arm();
}

/** A write was sent: past `STUCK_MS` the notice says Clax is not answering
 * yet, until the returned function says it settled. */
export function pending(): () => void {
  const reason = `wait-${++waits}`;
  const stop = after(STUCK_MS, () => connTrouble(reason, true, STILL_WAITING, document, 1), "request-stuck");
  return () => { stop(); connTrouble(reason, false, ""); };
}
