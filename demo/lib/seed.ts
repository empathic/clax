// Seeds a fresh Clax in hosted mode for the demo, through the routes a
// browser uses (crates/clax-server/src/hosted.rs and the API):
//
// - the first person, the owner (setup with the setup secret);
// - "Launch checklist" with three versions, shared by link for comments;
// - the product brief's outline for the demo agents (lib/brief.ts);
// - a second person, invited as an editor of both who may also create
//   artifacts, who comments on the checklist, forks its version 2 into an
//   artifact of their own and publishes a second version there through an
//   agent of theirs (an agent token over MCP: a person's API calls do not
//   publish);
// - an unused invite for a third person, an editor of the checklist only
//   (so on the brief they hold no reserved live seat);
// - an agent token of the owner's, for the demo agents;
// - a one-use sign-in link for each of the two people.

import { BRIEF_TITLE, SECTIONS, skeleton } from "./brief.ts";
import { acceptInvite, ClaxApiError, invite, McpAgent, mintAgentToken, NATIVE_MCP_PATH, setupFirstPerson, signinLink, trimBase, type PersonSession } from "./http.ts";

export interface SeedOptions {
  url: string;
  secret: string;
  owner?: string;
  editor?: string;
  invitee?: string;
  /** How many sections the brief's outline has, one per demo agent. */
  agents?: number;
  /** The MCP endpoint's path: `/mcp/json` (the default) on the native daemon, `/mcp` on the Worker. */
  mcpPath?: string;
}

export interface Seeded {
  base: string;
  checklist: { id: string; url: string; title: string };
  /** The editor's fork of the checklist's version 2, with a version of their own on it. */
  fork: { id: string; url: string; title: string };
  brief: { id: string; url: string; title: string };
  owner: { name: string; public_id: string; signin: string; session: PersonSession };
  editor: { name: string; public_id: string; signin: string; session: PersonSession };
  invite: { name: string; url: string; expires_at: string };
  token: { token: string; claude_command: string };
  agents: number;
}

interface Created {
  artifact: { id: string; title: string; head_version?: number };
  version: { n: number };
}

/** A small page of the checklist's. */
function page(title: string, items: string[], note: string): string {
  const li = items.map((i) => `<li>${i}</li>`).join("");
  return `<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>${title}</title>
<style>
  :root { --bg: #fbfaf7; --fg: #1d1c1a; --muted: #6b675f; --line: #e4e0d6; }
  @media (prefers-color-scheme: dark) { :root { --bg: #181816; --fg: #ecebe6; --muted: #a29d92; --line: #34322d; } }
  body { margin: 0; background: var(--bg); color: var(--fg); font: 16px/1.6 ui-sans-serif, system-ui, sans-serif; }
  main { max-width: 680px; margin: 0 auto; padding: 32px 16px; }
  li { padding: 6px 0; border-bottom: 1px solid var(--line); }
  .note { color: var(--muted); }
</style></head>
<body><main><h1>${title}</h1><p class="note">${note}</p><ol>${li}</ol></main></body></html>
`;
}

const file = (content: string) => ({ "index.html": { content } });

/** An editor's rights on artifacts `ids` (IDs, or `own`): read them, publish versions, and comment, reply, resolve, send and watch there. */
export function editorOf(ids: string[]): unknown[] {
  return [
    { type: "clax-artifacts", actions: ["read", "update"], locations: ids },
    { type: "clax-comments", actions: ["read", "write", "reply", "resolve", "send", "watch"], locations: ids },
  ];
}

/** What a person needs to fork and keep what they make: `create`, and an
 * editor's rights on the artifacts they created (`own`). */
export const MAKER: unknown[] = [
  { type: "clax-artifacts", actions: ["create"], locations: ["*"] },
  ...editorOf(["own"]),
];

/** An element anchor on the page's heading, as the shell sends one. */
export const HEADING_ANCHOR = JSON.stringify({
  kind: "element", selector: "body > main > h1", quote: null, prefix: null, suffix: null,
  html_hash: null, rect: null, custom_name: null, file: "index.html",
});

export async function seed(o: SeedOptions): Promise<Seeded> {
  const base = trimBase(o.url);
  const agents = o.agents ?? 3;
  if (!Number.isInteger(agents) || agents < 1 || agents > SECTIONS.length) {
    throw new Error(`--agents must be from 1 to ${SECTIONS.length}`);
  }
  let owner: PersonSession;
  try {
    owner = await setupFirstPerson(base, o.secret, o.owner ?? "Rae");
  } catch (e) {
    // Setup answers not_found for a wrong secret and once the instance has people alike.
    if (e instanceof ClaxApiError && e.code === "not_found") {
      throw new Error("Setup was refused: the secret is wrong, or the instance already has people. seed needs a fresh instance, before anyone opens /setup.");
    }
    throw e;
  }

  const title = "Launch checklist";
  const steps = ["Freeze the release branch", "Run the migration on staging", "Announce the window"];
  const a = await owner.call<Created>("POST", "/api/artifacts", { title, files: file(page(title, steps.slice(0, 2), "Draft")), note: "First draft" });
  const aid = a.artifact.id;
  await owner.call("POST", `/api/artifacts/${aid}/versions`, { files: file(page(title, steps, "Draft")), if_version: 1, note: "Add the announcement" });
  await owner.call("POST", `/api/artifacts/${aid}/versions`, {
    files: file(page(title, [...steps, "Watch the error rate for an hour"], "Ready for review")),
    if_version: 2,
    note: "Add the watch step",
  });
  // Anyone with the link may comment (as a guest, once named).
  await owner.call("PUT", `/api/hosted/artifacts/${aid}/link`, { access: "commenter" });

  // The demo agents' artifact: the brief's outline, the owner's.
  const b = await owner.call<Created>("POST", "/api/artifacts", {
    title: BRIEF_TITLE,
    files: file(skeleton(SECTIONS.slice(0, agents))),
    note: `Outline: ${agents} sections, one per agent`,
  });
  const bid = b.artifact.id;

  // The second person, an editor of both, so their comments on the brief can
  // be sent to the owner's agents, and a maker of their own artifacts.
  const editorName = o.editor ?? "Mia";
  const editor = await acceptInvite(base, (await invite(owner, editorName, [...editorOf([aid, bid]), ...MAKER])).url);
  await editor.form(`/api/artifacts/${aid}/threads`, { anchor: HEADING_ANCHOR, body: "Can we add a rollback step before the announcement?", version: "3" });
  // The editor's copy of the checklist as it was at version 2, with a step of their own.
  const f = await editor.call<Created>("POST", `/api/artifacts/${aid}/fork`, { version: 2 });
  const fid = f.artifact.id;
  const mobile = new McpAgent(base, o.mcpPath ?? NATIVE_MCP_PATH, (await mintAgentToken(editor, "mobile copy")).token, "mobile-editor");
  try {
    await (await mobile.connect()).ok("publish", {
      id: fid,
      html: page(`${title} (mobile)`, [...steps, "Submit the app build for review"], "The mobile team's copy"),
      if_version: f.version.n,
      note: "Add the app review step",
    });
  } finally {
    await mobile.close();
  }

  const inviteeName = o.invitee ?? "Jo";
  const unused = await invite(owner, inviteeName, editorOf([aid]));
  const token = await mintAgentToken(owner, "demo agents");
  const [ownerLink, editorLink] = await Promise.all([signinLink(owner), signinLink(editor)]);
  return {
    base,
    checklist: { id: aid, url: `${base}/a/${aid}`, title },
    fork: { id: fid, url: `${base}/a/${fid}`, title: f.artifact.title },
    brief: { id: bid, url: `${base}/a/${bid}`, title: BRIEF_TITLE },
    owner: { name: owner.person.display_name, public_id: owner.person.public_id, signin: ownerLink, session: owner },
    editor: { name: editor.person.display_name, public_id: editor.person.public_id, signin: editorLink, session: editor },
    invite: { name: inviteeName, url: unused.url, expires_at: unused.expires_at },
    token: { token: token.token, claude_command: token.claude_command },
    agents,
  };
}

/** What `seed` made, for a person to read. */
export function describe(s: Seeded): string {
  return `Seeded ${s.base}

Artifact   ${s.checklist.url}  (${s.checklist.title}, 3 versions, 1 thread, link access: can comment)
Fork       ${s.fork.url}  (${s.editor.name}'s fork of ${s.checklist.title} v2, 2 versions)
Brief      ${s.brief.url}  (the outline for the demo agents; ${s.editor.name} is an editor)

Sign in as ${s.owner.name}:  ${s.owner.signin}
Sign in as ${s.editor.name}:  ${s.editor.signin}
  (each link works once, for 10 minutes; open it in its own browser profile)
Invite for ${s.invite.name}:  ${s.invite.url}  (editor of ${s.checklist.title}, until ${s.invite.expires_at})

${s.owner.name}'s agent token (shown once):
  ${s.token.token}
Connect Claude Code with it:
  ${s.token.claude_command}`;
}
