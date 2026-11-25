// Which view the artifact view's sidebar shows, the timeline or the thread
// list, as this browser remembers it for every artifact (localStorage, read
// once with the sidebar's lazy chunk; every access may throw: private
// windows). The timeline by default, and wherever the stored choice cannot
// be read.
export type SidebarView = "timeline" | "threads";
export const SIDEBAR_KEY = "clax.sidebar";

function read(): SidebarView {
  try {
    const o: unknown = JSON.parse(localStorage.getItem(SIDEBAR_KEY) ?? "{}");
    if (o && typeof o === "object" && (o as { view?: unknown }).view === "threads") return "threads";
  } catch { /* the default */ }
  return "timeline";
}

let now: SidebarView = $state(read());

export const sidebarView = {
  get now(): SidebarView { return now; },
  /** Shows `v` and remembers it. */
  set(v: SidebarView): void {
    now = v;
    try { localStorage.setItem(SIDEBAR_KEY, JSON.stringify({ view: v })); } catch { /* this page only */ }
  },
};
