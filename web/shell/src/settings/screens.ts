// Settings' screens (`/settings/<id>`), in the order the navigation lists
// them, each with who sees it and what is on it. The same list is the
// navigation, the overview at `/settings`, and what the search looks
// through: a screen's title and line, and each item's title and words.

/** What decides which screens a browser sees. */
export type Ctx = {
  /** A hosted Clax (`GET /api/hosted/me` answers), not one on this machine. */
  hosted: boolean;
  /** The owner's browser. */
  owner: boolean;
  /** The owner's or a connected person's browser. */
  member: boolean;
  /** The owner or an admin they named (ClaxBot's pages). */
  admin: boolean;
  /** ClaxBot is on this Clax. */
  bot: boolean;
};

export type Item = {
  title: string;
  /** What follows the screen's path: `#<heading ID>` or a query. */
  at?: string;
  /** Words the search also matches, never shown. */
  words?: string;
  when?: (c: Ctx) => boolean;
};

export type Screen = {
  id: string;
  group: string;
  title: string;
  /** One line under the title on the overview. */
  line: string;
  when: (c: Ctx) => boolean;
  /** The screen shown instead to a browser this one is not for. */
  instead?: string;
  /** A page outside Settings, which the search lists and the navigation does not. */
  href?: string;
  items: Item[];
};

const local = (c: Ctx) => !c.hosted;
const hostedMember = (c: Ctx) => c.hosted && c.member;

export const SCREENS: Screen[] = [
  {
    id: "account", group: "You", title: "Account", line: "Who this browser is signed in as, your other devices, and signing out.", when: hostedMember, instead: "agents",
    items: [
      { title: "This browser", at: "#hm-h", words: "signed in as who am i" },
      { title: "Sign in another device", at: "#hm-h", words: "link phone laptop" },
      { title: "Sign out", at: "#hm-h", words: "log out logout" },
      { title: "Devices", at: "#hd-h", words: "clax app revoke device" },
    ],
  },
  {
    id: "agents", group: "You", title: "Agents & machines", line: "The agents working as you, the machines they run on, what each may do, and the tokens that connect them.", when: c => c.member,
    items: [
      { title: "Agent tokens", at: "#ht-h", words: "create token api key connect claude code codex grok mcp remote agent machine", when: c => c.hosted },
      { title: "Your machines", words: "disconnect computer connected clax connect" },
      { title: "What your agents may do", words: "rights permissions narrow publish delete comment reply" },
      { title: "Your agents", words: "rename end stop running" },
      { title: "Who reaches your agents", at: "#ppl-reach", words: "approvals approved revoke comments", when: c => !c.owner },
    ],
  },
  {
    id: "sharing", group: "You", title: "Shared links", line: "The pages anyone with their link can open, and what the link allows.", when: hostedMember,
    items: [
      { title: "Shared by link", at: "#hl-h", words: "public share page can view can comment" },
      { title: "Stop sharing a page", at: "#hl-h", words: "unshare private remove link" },
    ],
  },
  {
    id: "claxbot", group: "You", title: "Your ClaxBot", line: "Your credits this week, your own model key, web access, and your runs.", when: c => c.bot,
    items: [
      { title: "Credits this week", at: "#me-w", words: "usage meter limit resets" },
      { title: "Use my own Anthropic key", at: "#me-k", words: "model key api key byok" },
      { title: "Let ClaxBot search and read the web", at: "#me-web", words: "web search fetch" },
      { title: "Your runs", at: "#me-r", words: "history" },
    ],
  },
  {
    id: "confirmations", group: "You", title: "Confirmations", line: "Whether confirming a decision takes a passkey, and your passkeys.", when: c => local(c) && c.owner,
    items: [
      { title: "Require a passkey to confirm", at: "#pk-h", words: "security touch id webauthn" },
      { title: "Passkeys", at: "#pk-h", words: "add remove passkey locked out reset" },
    ],
  },
  {
    id: "people", group: "People", title: "People", line: "Who is connected to this Clax, what each may do, invites and guests.", when: c => c.owner, instead: "agents",
    items: [
      { title: "Invite a person", at: "?invite", words: "add user member join link connect another machine" },
      { title: "Invites", at: "#ppl-invites", words: "pending revoke invite expires" },
      { title: "Connection requests", at: "#ppl-requests", words: "approve deny machine" },
      { title: "Guests", at: "#ppl-unconnected", words: "comments from people who are not connected reach your agents", when: c => c.hosted },
      { title: "People on your LAN who are not connected", at: "#ppl-unconnected", words: "comments reach your agents guests", when: local },
      { title: "A person's rights", words: "reader collaborator full custom permissions role" },
      { title: "Revoke or block a person", words: "remove delete user disconnect machine reinstate" },
      { title: "Raise an agent", words: "elevate rights every artifact" },
      { title: "People on other Clax", at: "#ppl-remote", words: "remote authors approvals public copy" },
      { title: "Revoked people", at: "#ppl-revoked", words: "history reinstate" },
    ],
  },
  {
    id: "roles", group: "People", title: "Roles & ClaxBot access", line: "Who is an admin, who may use ClaxBot, and each person's weekly credits.", when: c => c.admin,
    items: [
      { title: "Admins and members", at: "#ap-h", words: "role make admin" },
      { title: "Turn ClaxBot on for a person", at: "#ap-h", words: "flag enable access" },
      { title: "Weekly credits and lifts", at: "#ap-h", words: "raise limit plan lift reason" },
      { title: "Unenforced limit", at: "#ap-h", words: "unlimited credits" },
    ],
  },
  {
    id: "provider", group: "ClaxBot", title: "Model provider", line: "The API ClaxBot calls, its key, its models, and the caps on a run.", when: c => c.admin,
    items: [
      { title: "Endpoint", at: "#pv-ep", words: "api base url api key anthropic openai gemini mistral" },
      { title: "Models", at: "#pv-m", words: "default quick complex model id" },
      { title: "Caps", at: "#pv-c", words: "max steps credits minutes tokens per run runs at once concurrency" },
      { title: "Test connection", at: "#pv-ep", words: "check provider key" },
    ],
  },
  {
    id: "web", group: "ClaxBot", title: "Web access", line: "Whether ClaxBot may search and fetch the web, with which provider, and from where.", when: c => c.admin,
    items: [
      { title: "Web search and web fetch", at: "#aw-on", words: "turn on off internet" },
      { title: "Search provider", at: "#aw-s", words: "ceramic exa linkup results per search gateway key alias searches per run" },
      { title: "URLs a fetch may take", at: "#aw-f", words: "allowlist allowed hosts any public url fetches per run" },
      { title: "Cloudflare", at: "#aw-cf", words: "account id api token", when: local },
    ],
  },
  {
    id: "pricing", group: "ClaxBot", title: "Pricing", line: "What a credit costs, the free credits each week, and each model's rates.", when: c => c.admin,
    items: [
      { title: "Credit price and free weekly credits", at: "#pr-c", words: "usd free tier" },
      { title: "Model rate cards", at: "#pr-m", words: "price input output cached cache write catalog refresh tokenulator" },
      { title: "Add a model", at: "#pr-a", words: "price it rate card" },
      { title: "Default rates for unpriced models", at: "#pr-d", words: "require an explicit price" },
      { title: "Other cost lines", at: "#pr-l", words: "margin search fetch" },
    ],
  },
  {
    id: "usage", group: "ClaxBot", title: "Usage", line: "What each week cost: credits spent, runs, and margin, by person and by cost line.", when: c => c.admin,
    items: [
      { title: "Usage by person", at: "#us-p", words: "credits spent runs week" },
      { title: "Usage by cost line", at: "#us-l", words: "cost margin week" },
    ],
  },
  {
    id: "config", group: "System", title: "Configuration", line: "This Clax's config.toml: every field, its value, and where the value comes from.", when: c => local(c) && (c.owner || c.admin),
    items: [
      { title: "[general]", at: "#sec-general", words: "bin binary plugins" },
      { title: "[serve]", at: "#sec-serve", words: "port" },
      { title: "[sample]", at: "#sec-sample", words: "provider api key models daily cap cost max tokens stub images" },
      { title: "[toolpath]", at: "#sec-toolpath", words: "journal audit retain days segment" },
      { title: "[gallery]", at: "#sec-gallery", words: "quickstart" },
      { title: "[questions]", at: "#sec-questions", words: "terminal after seconds ask" },
      { title: "[bot]", at: "#sec-bot", words: "claxbot enabled api base url max steps credits concurrency models script web price catalog" },
      { title: "Environment variables", words: "CLAX_ env override restart" },
    ],
  },
  {
    id: "remotes", group: "System", title: "Remotes", line: "The Clax instances you answered Never for, which you can unmute.", when: c => local(c) && c.owner,
    items: [{ title: "Muted instances", at: "#sec-remotes", words: "unmute never for this instance connect" }],
  },
  {
    id: "audit", group: "System", title: "Audit log", line: "What happened here, newest first: views, changes, ClaxBot runs, credits and admin changes.", when: c => c.member,
    items: [
      { title: "Search the audit log", words: "history events who acted kind artifact when" },
      { title: "Export Toolpath", words: "provenance download" },
    ],
  },
  {
    id: "queues", group: "Elsewhere", title: "Queues", line: "Report queues: their members and service grants.", when: c => c.owner || c.admin, href: "/queues",
    items: [
      { title: "New queue", words: "create report queue origins" },
      { title: "Queue members", words: "triage admin add remove" },
      { title: "Service grants", words: "create token private key backend revoke" },
    ],
  },
  {
    id: "tags", group: "Elsewhere", title: "Tags", line: "The tags artifacts carry.", when: c => c.member, href: "/tags",
    items: [{ title: "Manage tags", words: "rename colour color delete new tag" }],
  },
];

/** The address of a screen, or of one of its items. */
export const hrefOf = (s: Screen, at = ""): string => (s.href ?? `/settings/${s.id}`) + (s.href ? "" : at);

/** The screens this browser sees, in order. */
export const screensFor = (c: Ctx): Screen[] => SCREENS.filter(s => s.when(c));

/** Settings' own screens for this browser, grouped as the navigation lists them. */
export function groupsFor(c: Ctx): { name: string; screens: Screen[] }[] {
  const out: { name: string; screens: Screen[] }[] = [];
  for (const s of screensFor(c)) {
    if (s.href) continue;
    const g = out.find(x => x.name === s.group);
    if (g) g.screens.push(s); else out.push({ name: s.group, screens: [s] });
  }
  return out;
}

/** The screen `/settings/<id>` shows this browser: that one, else the one
 * it names instead, else none (the overview says so). */
export function screenFor(id: string, c: Ctx): Screen | null {
  const s = SCREENS.find(x => x.id === id && !x.href);
  if (!s) return null;
  if (s.when(c)) return s;
  const other = s.instead ? SCREENS.find(x => x.id === s.instead) : null;
  return other?.when(c) ? other : null;
}

/** The screen a Settings path names (`/settings/<id>`), or null: the overview. */
export const idOf = (pathname: string): string | null => /^\/settings\/([a-z-]+)$/.exec(pathname)?.[1] ?? null;

/** One thing the search found: a screen, or an item on one. */
export type Hit = { key: string; title: string; where: string; href: string; screen: Screen };

const norm = (s: string) => s.toLowerCase();

/** What `query` finds among the screens this browser sees, best first:
 * every word of it must be in an entry's title, its screen's or group's
 * name, or its words. A title that starts with the query leads, then a
 * title holding a word that starts with it, then the rest, each in the
 * screens' order. Without a query: the screens themselves. */
export function search(query: string, c: Ctx): Hit[] {
  const words = norm(query).split(/\s+/).filter(Boolean);
  const hits: { hit: Hit; rank: number }[] = [];
  const q = words.join(" ");
  const add = (s: Screen, title: string, at: string, extra: string, own: boolean) => {
    const t = norm(title);
    const hay = `${t} ${norm(s.title)} ${norm(s.group)} ${norm(extra)}`;
    if (!words.every(w => hay.includes(w))) return;
    const rank = !words.length ? 0 : t.startsWith(q) ? 0 : t.split(/[^a-z0-9]+/).some(w => w.startsWith(words[0])) ? 1 : words.every(w => t.includes(w)) ? 2 : 3;
    hits.push({ rank, hit: { key: `${s.id}:${own ? "" : title}`, title, where: own ? s.group : `${s.group} › ${s.title}`, href: hrefOf(s, at), screen: s } });
  };
  for (const s of screensFor(c)) {
    add(s, s.title, "", s.line, true);
    if (words.length) for (const i of s.items) if (!i.when || i.when(c)) add(s, i.title, i.at ?? "", i.words ?? "", false);
  }
  return hits.sort((a, b) => a.rank - b.rank).map(h => h.hit);
}
