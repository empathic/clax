// Agents' drafts as the artifact view shows them (hosted spec
// 2026-10-11-clax-dev-hosted-design §16.4): pure functions over the
// daemon's draft views, for the banner and the frame.

/** A live draft as the daemon lists it to people: the agent's handle and
 * harness, the version it changes, the paths it writes or removes, and the
 * token naming this revision in the draft URL. */
export type DraftView = { agent: string; harness: string; base_version: number; paths: string[]; note: string | null; updated_at: string; token: string };

/** How many elements of the shown draft changed, and whether they are
 * tinted (not when too many changed), as its page says. */
export type DraftChanged = { n: number; marked: boolean };

/** The message a draft page posts to its parent (the daemon's tint script). */
export const DIFF_MESSAGE = "clax:draft-diff";

/** The live drafts, oldest revision first. */
export function oldestFirst(list: DraftView[]): DraftView[] {
  return [...list].sort((a, b) => a.updated_at.localeCompare(b.updated_at) || a.agent.localeCompare(b.agent));
}

/** Where the frame shows `file` of draft `d`: under `/_clax/draft/` on the
 * artifact's site origin `origin`, each path segment encoded; the fragment
 * names the tint colour. */
export function draftSrc(d: DraftView, origin: string, file: string, tint: string): string {
  const root = `${origin}/_clax/draft/`;
  const path = file === "index.html" ? "" : file.split("/").map(encodeURIComponent).join("/");
  return `${root}${encodeURIComponent(d.agent)}/${encodeURIComponent(d.token)}/${path}#tint=%23${tint}`;
}

/** `css` (a computed `--agent`) as six hex digits, or the light theme's agent green. */
export function tintOf(css: string): string {
  const v = css.trim();
  return /^#[0-9a-f]{6}$/i.test(v) ? v.slice(1).toLowerCase() : "2f6f2a";
}

/** The count a draft page's message carries, or null for any other message. */
export function parseDiff(data: unknown): DraftChanged | null {
  if (typeof data !== "object" || data === null) return null;
  const d = data as { type?: unknown; changed?: unknown; marked?: unknown };
  if (d.type !== DIFF_MESSAGE || !Number.isInteger(d.changed) || (d.changed as number) < 0) return null;
  return { n: d.changed as number, marked: d.marked !== false };
}

const files = (d: DraftView) => (d.paths.length === 0 ? "no changes yet" : d.paths.length === 1 ? d.paths[0] : `${d.paths.length} files`);

/** What the banner says. `name` names an agent by handle; `showing` is the
 * agent whose draft the frame shows (null for none); `shownN` the version
 * shown otherwise. */
export type BannerText =
  | { kind: "drafting"; who: string; note: string | null; agent: string }
  | { kind: "many"; who: string; agents: { agent: string; who: string }[] }
  | { kind: "showing"; who: string; base: number; detail: string; others: { agent: string; who: string }[]; back: string };

export function bannerText(list: DraftView[], showing: string | null, changed: DraftChanged | null, shownN: number, name: (handle: string, harness: string) => string): BannerText | null {
  const drafts = oldestFirst(list);
  if (drafts.length === 0) return null;
  const who = (d: DraftView) => name(d.agent, d.harness);
  const current = showing === null ? undefined : drafts.find(d => d.agent === showing);
  if (current) {
    const detail = changed === null ? files(current)
      : !changed.marked ? `${changed.n} changes, too many to highlight`
      : changed.n === 1 ? "1 change highlighted" : `${changed.n} changes highlighted`;
    return {
      kind: "showing", who: who(current), base: current.base_version, detail,
      others: drafts.filter(d => d.agent !== showing).map(d => ({ agent: d.agent, who: who(d) })),
      back: `Back to v${shownN}`,
    };
  }
  if (drafts.length === 1) return { kind: "drafting", who: who(drafts[0]), note: drafts[0].note, agent: drafts[0].agent };
  return { kind: "many", who: `${drafts.length} agents`, agents: drafts.map(d => ({ agent: d.agent, who: who(d) })) };
}
