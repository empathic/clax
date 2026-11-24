// ClaxBot's week for the gallery's first render (its header meter and the
// usage banner, ui/BotMeter.svelte, UsageBanner.svelte): credits in words,
// the week's meter and its warn and exhausted states, and the banner. Its
// own module, so the gallery's entry carries these alone and not the rest
// of `model.ts`, which re-exports them.
import type { MeBot, Week } from "./api";

const int = new Intl.NumberFormat("en-US");

/** Milli-credits as credits: whole numbers with separators ("1,690"),
 * a fraction to two places where there is one ("0.76"). */
export function credits(milli: number): string {
  if (milli % 1000 === 0) return int.format(milli / 1000);
  const v = milli / 1000;
  return new Intl.NumberFormat("en-US", { minimumFractionDigits: Math.abs(v) < 1 ? 2 : 0, maximumFractionDigits: 2 }).format(v);
}

/** When the week's credits reset: the week's end as a UTC date ("Monday, Oct 27"). */
export const resetDay = (end: string): string =>
  new Date(end).toLocaleDateString("en-US", { weekday: "long", month: "short", day: "numeric", timeZone: "UTC" });

/** The week's meter. `weekly_milli` is the whole allowance (the free tier
 * and any lift); warn from 80 % (§7.4, the UI's own state, also given by
 * the daemon), exhausted once spent reaches it. Unenforced, the meter
 * shows the numbers and never the exhausted state's block. */
export type Meter = { spent: number; total: number; left: number; ratio: number; warn: boolean; exhausted: boolean };
export function meter(w: Week): Meter {
  const total = Math.max(0, w.weekly_milli);
  const spent = Math.max(0, w.spent_milli);
  const ratio = total ? spent / total : spent ? 1 : 0;
  const exhausted = w.exhausted || (total > 0 && spent >= total);
  return { spent, total, left: Math.max(0, total - spent), ratio: Math.min(1, ratio), warn: !exhausted && (w.warn || ratio >= 0.8), exhausted };
}

/** Whether this person's runs use their own model key. Their tokens are
 * then billed to that key and charge no credits; the lines charged on every
 * key (`applies: "always"`: the per-run fee, script compute, storage and
 * web calls) still spend the week's credits, and running out still pauses
 * ClaxBot. */
export const ownKey = (me: MeBot | null): boolean => !!me?.key.use_own && !!me.key.own?.set;

/** What credits pay for on a person's own key, in words. */
export const OWN_KEY_SPEND = "run fees, script compute, storage and web calls";

/** The week's meter in words for `me` (ClaxBot on): `label` for the gallery
 * header's link and its `aria-label`, `title` for its hover text and the
 * artifact view's. On their own key they say so, and what credits pay for. */
export function meterWords(me: MeBot): { own: boolean; label: string; title: string } {
  const m = meter(me.week);
  const used = `${credits(m.spent)} of ${credits(m.total)} credits used this week`;
  if (!ownKey(me)) return { own: false, label: `ClaxBot: ${used}`, title: "Your ClaxBot credits this week" };
  const title = `ClaxBot runs on your key, which pays for your tokens. Credits pay only for ${OWN_KEY_SPEND}.`;
  return { own: true, label: `ClaxBot on your key: ${used}, for ${OWN_KEY_SPEND}`, title };
}

/** The usage banner a page shows for `me`: none, a warning from 80 %, or
 * the exhausted state (not for an unenforced person, whom ClaxBot never
 * stops). On their own key it says what the credits paid for. */
export type Banner = { kind: "warn" | "exhausted"; text: string; detail: string } | null;
export function banner(me: MeBot | null): Banner {
  if (!me?.enabled) return null;
  const m = meter(me.week);
  const reset = resetDay(me.week.end);
  const own = ownKey(me) ? ` On your key, credits pay for ${OWN_KEY_SPEND}, not tokens.` : "";
  if (m.exhausted && !me.unenforced) {
    return { kind: "exhausted", text: "This week's credits are used up.", detail: `You've used ${credits(m.spent)} of ${credits(m.total)} credits. Your pages, comments and local agents keep working; ClaxBot is paused until ${reset}.${own}` };
  }
  if (m.warn || (m.exhausted && me.unenforced)) {
    return { kind: "warn", text: `You've used ${credits(m.spent)} of your ${credits(m.total)} credits this week.`, detail: (me.unenforced ? `Your limit is not enforced: ClaxBot keeps working. Resets ${reset}.` : `ClaxBot keeps working until you reach it. Resets ${reset}.`) + own };
  }
  return null;
}

