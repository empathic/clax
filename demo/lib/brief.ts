// The page the demo agents write together: a product brief whose sections
// each belong to one agent. Every section sits between markers
// (`<!--section:KEY-->` … `<!--/section:KEY-->`), so an agent merges its
// change into whatever version is current by replacing its own section and
// leaving the rest of the page as it found it. The content is fixed text: no
// model is called.

export interface SectionPlan {
  key: string;
  heading: string;
  /** What the agent says it is doing, per draft. */
  working: string[];
  /** The section body (HTML) after each draft. */
  drafts: string[];
}

export const BRIEF_TITLE = "Product brief: Trailhead";

export const SECTIONS: SectionPlan[] = [
  {
    key: "summary",
    heading: "Summary",
    working: ["outlining the summary", "writing the summary", "tightening the summary"],
    drafts: [
      "<p>Offline maps for volunteer search teams.</p>",
      "<p>Trailhead gives volunteer search-and-rescue teams shared maps that keep working without a signal. Each searcher's phone records where they have been; when two phones meet, they swap tracks.</p>",
      "<p>Trailhead gives volunteer search-and-rescue teams shared maps that keep working without a signal. Each searcher's phone records where they have been; when two phones meet, they swap tracks, so the incident lead sees covered ground without a radio call.</p><p class=\"k\">Ask: two engineers and one designer for one quarter.</p>",
    ],
  },
  {
    key: "users",
    heading: "Who it is for",
    working: ["listing who uses it", "describing the incident lead", "adding the field searcher"],
    drafts: [
      "<ul><li>Incident leads</li><li>Field searchers</li></ul>",
      "<ul><li><b>Incident leads</b> plan search areas on a laptop at the trailhead and need to see what has been covered.</li><li>Field searchers</li></ul>",
      "<ul><li><b>Incident leads</b> plan search areas on a laptop at the trailhead and need to see what has been covered.</li><li><b>Field searchers</b> carry a phone, often gloved, often with no signal for hours.</li></ul>",
    ],
  },
  {
    key: "proposal",
    heading: "What we build",
    working: ["sketching the proposal", "writing the sync design", "adding the lead's view"],
    drafts: [
      "<p>A phone app and a lead's map.</p>",
      "<p>A phone app records a GPS track every 15 seconds and exchanges tracks with any phone in Bluetooth range. Tracks merge by searcher and time, so an exchange never loses data.</p>",
      "<p>A phone app records a GPS track every 15 seconds and exchanges tracks with any phone in Bluetooth range. Tracks merge by searcher and time, so an exchange never loses data.</p><p>The lead's map shades each grid cell by how recently someone passed through it.</p>",
    ],
  },
  {
    key: "risks",
    heading: "Risks",
    working: ["listing risks", "weighing battery drain", "adding privacy"],
    drafts: [
      "<ul><li>Battery</li></ul>",
      "<ul><li><b>Battery.</b> GPS every 15 s costs about 6% an hour on a mid-range phone; searches run 8 hours.</li></ul>",
      "<ul><li><b>Battery.</b> GPS every 15 s costs about 6% an hour on a mid-range phone; searches run 8 hours.</li><li><b>Privacy.</b> Tracks show where volunteers were; they are deleted 30 days after the incident closes.</li></ul>",
    ],
  },
  {
    key: "milestones",
    heading: "Milestones",
    working: ["drafting milestones", "dating the pilot", "adding the field trial"],
    drafts: [
      "<ol><li>Prototype</li></ol>",
      "<ol><li><b>Week 4:</b> two phones swap tracks over Bluetooth.</li><li><b>Week 8:</b> pilot with one county team.</li></ol>",
      "<ol><li><b>Week 4:</b> two phones swap tracks over Bluetooth.</li><li><b>Week 8:</b> pilot with one county team.</li><li><b>Week 12:</b> field trial on a training search, 20 searchers.</li></ol>",
    ],
  },
  {
    key: "metrics",
    heading: "How we measure it",
    working: ["choosing metrics", "defining coverage lag", "adding adoption"],
    drafts: [
      "<p>Coverage lag.</p>",
      "<p><b>Coverage lag:</b> minutes from a searcher walking a cell to the lead's map showing it. Today it is a radio call, about 20 minutes.</p>",
      "<p><b>Coverage lag:</b> minutes from a searcher walking a cell to the lead's map showing it. Today it is a radio call, about 20 minutes.</p><p><b>Adoption:</b> share of searchers on a callout who carry the app.</p>",
    ],
  },
];

export function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

const open = (key: string) => `<!--section:${key}-->`;
const close = (key: string) => `<!--/section:${key}-->`;

/** A section as it sits on the page, with the agent that wrote it. */
export function sectionHtml(plan: SectionPlan, agent: string | null, body: string): string {
  const by = agent === null ? `<span class="by wait">waiting for an agent</span>` : `<span class="by">${escapeHtml(agent)}</span>`;
  return `${open(plan.key)}<section id="${plan.key}"><h2>${escapeHtml(plan.heading)} ${by}</h2>${body}</section>${close(plan.key)}`;
}

/** The first version: every section's heading, waiting for its agent. */
export function skeleton(plans: SectionPlan[]): string {
  const sections = plans.map((p) => sectionHtml(p, null, `<p class="todo">Not written yet.</p>`)).join("\n");
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>${escapeHtml(BRIEF_TITLE)}</title>
<style>
  :root { --bg: #fbfaf7; --fg: #1d1c1a; --muted: #6b675f; --line: #e4e0d6; --agent: #2f6f5e; --agent-tint: #e3f0eb; }
  @media (prefers-color-scheme: dark) { :root { --bg: #181816; --fg: #ecebe6; --muted: #a29d92; --line: #34322d; --agent: #7cc3ad; --agent-tint: #203a32; } }
  * { box-sizing: border-box; }
  body { margin: 0; background: var(--bg); color: var(--fg); font: 16px/1.55 ui-sans-serif, system-ui, sans-serif; }
  main { max-width: 760px; margin: 0 auto; padding: 32px 16px 64px; }
  h1 { font-size: 28px; line-height: 1.2; margin: 0 0 4px; }
  .lede { color: var(--muted); margin: 0 0 24px; }
  section { border-top: 1px solid var(--line); padding: 16px 0 8px; }
  h2 { font-size: 18px; margin: 0 0 8px; display: flex; align-items: baseline; gap: 10px; flex-wrap: wrap; }
  .by { font: 500 12px/20px ui-monospace, monospace; color: var(--agent); background: var(--agent-tint); border-radius: 999px; padding: 0 8px; }
  .by.wait { color: var(--muted); background: transparent; border: 1px dashed var(--line); }
  .todo { color: var(--muted); font-style: italic; }
  .k { font-weight: 600; }
  .revised { border-left: 3px solid var(--agent); padding-left: 10px; color: var(--muted); }
</style>
</head>
<body>
<main>
<h1>${escapeHtml(BRIEF_TITLE)}</h1>
<p class="lede">Written together by ${plans.length} agents, one section each.</p>
${sections}
</main>
</body>
</html>
`;
}

/** `html` with section `key` replaced by `section`; null when the page has no such section. */
export function replaceSection(html: string, key: string, section: string): string | null {
  const a = html.indexOf(open(key));
  const b = html.indexOf(close(key));
  if (a < 0 || b < a) return null;
  return html.slice(0, a) + section + html.slice(b + close(key).length);
}

/** The body of section `key` as `html` has it, or null. */
export function sectionOf(html: string, key: string): string | null {
  const a = html.indexOf(open(key));
  const b = html.indexOf(close(key));
  if (a < 0 || b < a) return null;
  return html.slice(a + open(key).length, b);
}

/** A section's body revised after a comment: its latest body and a note quoting the request. */
export function revisedBody(body: string, author: string, comment: string): string {
  const quote = comment.length > 160 ? `${comment.slice(0, 157)}…` : comment;
  return `${body}<p class="revised">Revised for ${escapeHtml(author)}: “${escapeHtml(quote)}”</p>`;
}
