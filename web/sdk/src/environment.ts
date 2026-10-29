// The page and browser a report is captured in (spec
// 2026-10-29-feedback-loop-design §5.1, `environment`), as the browser
// tells it; the host adds `route` itself, as only it knows its routes.
import type { Environment } from "./types";
import { scrub, scrubUrl } from "./scrub";

/** What `environment` reads off a window. */
interface EnvWindow {
  location: { href: string };
  document?: { title?: string };
  innerWidth: number;
  innerHeight: number;
  devicePixelRatio?: number;
  scrollX?: number;
  scrollY?: number;
  navigator: { userAgent?: string; platform?: string; language?: string; userAgentData?: { platform?: string } };
  matchMedia?: (query: string) => { matches: boolean };
}

/** `win`'s environment now: its URL with every query value replaced
 * (`scrubUrl`), its title scrubbed, its viewport, device pixel ratio and
 * scroll, its user agent, platform (`navigator.userAgentData.platform`
 * where the browser has it, else `navigator.platform`), locale, time zone
 * and colour scheme. A field the browser does not give is left out. */
export function environment(win: Window): Environment {
  const w = win as unknown as EnvWindow;
  const env: Environment = { url: scrubUrl(w.location.href) };
  const title = w.document?.title;
  if (title) env.title = scrub(title);
  env.viewport = { w: w.innerWidth, h: w.innerHeight, dpr: w.devicePixelRatio ?? 1 };
  env.scroll = { x: w.scrollX ?? 0, y: w.scrollY ?? 0 };
  const nav = w.navigator;
  if (nav.userAgent) env.user_agent = nav.userAgent;
  const platform = nav.userAgentData?.platform || nav.platform;
  if (platform) env.platform = platform;
  if (nav.language) env.locale = nav.language;
  try {
    const tz = Intl.DateTimeFormat().resolvedOptions().timeZone;
    if (tz) env.time_zone = tz;
  } catch {
    // A runtime without time zones leaves it out.
  }
  if (typeof w.matchMedia === "function") {
    try {
      env.color_scheme = w.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
    } catch {
      // A page whose matchMedia fails leaves it out.
    }
  }
  return env;
}
