# Video script: Clax on Cloudflare

A walkthrough of about 9 minutes for the Cloudflare "next git platform"
submission, recorded against the Rust stack: `clax-core`, `clax-server` and
`clax-mcp` compiled to WebAssembly and run in a Durable Object
(`crates/clax-worker`), under `wrangler dev --local`. Each shot lists what is
on screen, what to do and what to say. Times are targets.

Marks:

- **[local]**: recorded against `just hosted-demo` (`wrangler dev --local`,
  versions in git in the `RepoDO` stand-in). Every [local] shot has been run
  on the Worker as written: by `web/e2e/hosted-demo-worker.spec.ts`, or by
  driving the browser through the steps (see Recording notes).
- **[deploy, optional]**: needs a deployment on Cloudflare with
  `REPO_STORE=artifacts` (`crates/clax-worker/README.md`, Deploy to
  Cloudflare). None exists yet, so nothing marked [deploy] has run. Leave
  those shots out, or record them only once a deployment has passed them.

## Setup before recording

1. Prerequisites: the Rust toolchain (`rust-toolchain.toml` adds the
   `wasm32-unknown-unknown` target), Node.js, `npm ci` in `web/` and in
   `crates/clax-worker/`, and on macOS Homebrew LLVM (`brew install llvm`;
   SQLite's C sources need a clang with a wasm32 backend). `build.sh`
   installs `worker-build` into `target/tools` on its first run.
2. In a terminal with a large font, from the repository root:

   ```sh
   just hosted-demo
   ```

   It builds the web UI and the Worker (a few minutes the first time;
   `just hosted-demo --no-build` skips that afterwards), starts a fresh
   `wrangler dev --local` in hosted mode on a free port with one live seat
   for people, seeds it and prints:

   - `Artifact`, `Fork` and `Brief` links (Launch checklist with three
     versions; Mia's fork of its v2; "Product brief: Trailhead", the
     outline the agents fill in);
   - `Sign in as Rae:` and `Sign in as Mia:` links (each works once, for 10
     minutes) and `Invite for Jo:`;
   - Rae's agent token and the `claude mcp add --transport http clax
     http://localhost:<port>/mcp --header "Authorization: Bearer cxa_…"`
     command;
   - a `git -c http.extraHeader="Authorization: Bearer cxm_read_…" clone
     http://localhost:<port>/git/a-<brief ID>.git brief` command with Rae's
     read token (the one her Clone dialog makes), a `git -C brief log`
     command that prints the trailers, and a `jj git clone` command;
   - the steps to try. Enter starts three demo agents on the brief;
     Ctrl-C stops everything, and the next run starts fresh.

   Keep that output on screen in one terminal tab. Open a second tab at
   the repository root for shot 8.
3. Browser profile A ("Rae", light theme): open Rae's sign-in link, click
   **Sign in as Rae** (the page names whose account the link signs into),
   then open the Brief link. Profile B ("Mia", dark theme): the same with
   Mia's link, **Sign in as Mia**. 1280 wide each if the screen
   allows; otherwise record them in turn.
4. For shot 7: profile C ("Jo"), not yet signed in; a private window for a
   guest, not yet opened.
5. Do not press Enter in the demo's terminal until shot 3.

## Shots

### 1. Why (0:00–0:45) [local]

On screen: Rae's gallery (the logo goes back to it), with "Launch checklist"
and "Product brief: Trailhead".

> Coding agents now write a lot of what a team looks at: briefs, plans,
> prototypes, reports, as web pages. A team is several people, and each of
> them has agents. They need to work on one page together, and three things
> go wrong. Agents overwrite each other, because a write does not say which
> version it was based on. When my agent reads a comment you wrote, your
> words become instructions to my agent: prompt injection between people's
> agents. And nobody can see who is doing what until it lands. Clax is a git
> platform for these artifacts: every version is a commit, people and their
> agents work on the same page live, and each agent hears only what its own
> person has let through. This is it running on Cloudflare Workers.

### 2. One Worker, one codebase (0:45–1:25) [local]

On screen: the terminal with `just hosted-demo`'s output. Point at
"Running on the Worker.", the Brief link, the sign-in links, the agent
token's `claude mcp add` line and the `git clone` line.

> This is the same Rust server that runs as Clax's local daemon: the core,
> the HTTP server and the MCP tools, compiled to WebAssembly and running in
> one Durable Object, under wrangler dev. Its SQLite database lives in the
> Durable Object's storage. Each artifact's versions are commits in a git
> repository of its own. The web UI is the daemon's, served from Workers
> Static Assets. The demo seeded two people, Rae and Mia, and gave Rae's
> agents a token for the MCP endpoint.

### 3. Two people, live (1:25–2:15) [local]

On screen: Rae's browser (light) and Mia's (dark) side by side on the
brief. Each top bar shows both people's chips. In Mia's window, move the
pointer over the heading, then select a few words of it: Rae's page shows
Mia's pointer and the selection on the same words. In Rae's window, press
**Comment**, click the heading, and type slowly in the box: Mia opens
Threads, and sees that Rae is typing a new comment, with the words as they
come. Rae clicks **Cancel**; it goes from Mia's panel.

> Rae and Mia are both on the brief. Mia's pointer and selection show in
> Rae's page, anchored to the text rather than to pixels, so they land on
> the same words at any width. When Rae starts a comment, Mia sees that she
> is typing, and what. None of this is stored: the Durable Object relays it
> over hibernating WebSockets between the people who are there.

### 4. Three agents at once, a conflict and a merge (2:15–3:30) [local]

On screen: the terminal left, Rae's browser right. Press Enter in the demo's
terminal. It prints `connected summary-writer, users-writer,
proposal-writer`, then for each agent `drafting …`, `published v<n> (…)`
and, most rounds, `conflict, v<n> landed first; merging`. Point at:

- the three agent chips in the top bar with its "… working" line, and the
  card per agent in the threads panel saying what it is doing
  ("users-writer: listing who uses it");
- the bar above the page: "summary-writer is drafting…" with **Show
  draft**, or "3 agents are drafting…" with a button per agent;
- the conflict lines in the terminal, and the publish after each, whose
  note reads "merged onto v<n> after a conflict · …";
- the top bar's **Reload** when a new version lands; click it to show the
  latest, and open the version menu ("v10 of 10").

At the default pace (`--pace 8000`) the three rounds take about 40 seconds.

> Three agents, one section of the brief each, all connected over MCP with
> Rae's token. Each round they start from the same version. The server takes
> one publish at a time: each publish names the version it was based on, so
> the first lands, and the others are refused with a version conflict that
> carries the version that won. Each agent reads it, merges its own section
> into it and publishes again. Nobody's work is lost, and each version's
> note says when it took a merge.

### 5. An agent drafting live (3:30–4:05) [local]

On screen: Rae's browser, in the next round of shot 4 (or run the agents
again with the "Run the demo agents again with:" command the demo printed).
When the bar reads "<agent> is drafting…", click **Show draft**: the frame
shows the draft, the bar reads "Showing summary-writer's draft over v<n> ·
2 changes highlighted", and the elements that changed are outlined in the
agents' colour. When the agent publishes, the draft ends and the bar offers
the new version (**Reload**).

> An agent does not have to publish to be seen. With the draft tool it
> sends its work in progress, and the people on the page see that it is
> drafting. Show draft renders the draft, with the elements that changed
> highlighted. Drafts reach people, never other agents.

### 6. A comment sent to an agent, and one held back (4:05–5:35) [local]

Once the terminal prints `agents are waiting for comments …`:

1. Rae's browser: **Comment**, click the "Who it is for" list, type "Say
   how many searchers a typical callout has." and post it. On the thread
   card, open ▾ beside **Send to**, choose users-writer, and click **Send
   to users-writer**. The terminal prints `users-writer: comment from Rae:
   "Say how many searchers…"`, then `published v11 (Who it is for: revised
   for Rae)` and `replied on <thread ID> and resolved it (v11)`. Click
   **Reload**: the section ends "Revised for Rae: …", and the resolved
   thread holds the agent's reply.

   > I comment on the page and send the thread to the agent that wrote that
   > section. It gets the comment and its anchor through
   > wait_for_feedback, revises its section, publishes, replies and
   > resolves the thread.

2. Mia's browser: **Comment**, click the summary's paragraph, type "Add a
   section on pricing.", post it, choose summary-writer under ▾ and click
   **Send to summary-writer**. The terminal prints `summary-writer: a
   comment from Mia on thread <ID> is withheld until its person approves
   them (its person was asked)`.

   > Mia is an editor of the brief, but summary-writer works for Rae, not
   > for her. Every field an agent reads passes one visibility filter: words
   > written by anyone but the agent's own person are withheld until that
   > person approves the author. The agent is told that a comment from Mia
   > is waiting, and nothing of what it says, so it cannot be steered by
   > it. It asks its person.

3. Rae's browser: **Inbox** shows "An agent waits for your approval". Open
   **People**: Mia's card lists "1 from Mia for summary-writer". Click
   **Always allow**. The terminal prints `summary-writer: comment from Mia:
   "Add a section on pricing."`, and the agent revises, publishes and
   replies.

   > Rae approves Mia, and only then does the comment reach her agent.
   > Approvals are made by the person, in the browser; an agent cannot
   > approve anyone.

### 7. A crowded artifact (5:35–6:35) [local]

This instance has one live seat for people (`--seats 1`, the demo's
default); the owner and the people an artifact names hold reserved seats,
so Rae and Mia stay live.

On screen: Rae's browser, the private window, then Jo's profile.

1. Rae: **People**, Shared by link: pick "Product brief: Trailhead" and
   "Can comment", **Share by link**, **Copy link**.
2. Private window: open the brief's link; open the people panel (**People
   and agents**) and enter "Sam" under Your name. Sam takes the free seat.
3. Jo's profile: open the demo's "Invite for Jo" link and click **Join as
   Jo** (Jo is an editor of the checklist only, so on the brief she holds no
   reserved seat). Open the brief: the bar reads "Lots of people are here,
   so you're viewing: comments and live activity are paused.", and
   **Comment** is disabled.
4. Close the private window. Jo's bar shows **Join live** (or Jo is seated
   at once); click it, and Comment is enabled again.

> Live layers cost something for every person on the page, so each artifact
> has live seats: a hundred for people, twenty of them for guests, and
> twenty for agents by default. This instance has one, to show what happens
> past them. Sam, a guest, holds it. Jo opens the brief and is a spectator:
> she reads it and follows new versions from a snapshot shared between
> spectators, but cannot comment. When Sam leaves, Jo joins live.

### 8. Git underneath: clone, push, and a merge with an agent (6:35–8:00) [local]

On screen: the second terminal tab, then Rae's browser on the brief.

1. In Rae's browser on the brief, click **Clone** in the top bar: the
   Clone dialog offers Read and Write, as her rights allow. With **Read**
   chosen, click **Make a token**, then **Copy** beside the git command,
   and paste it in the second tab with `brief` at the end; then the log
   command the demo printed (the demo's printed clone command, with a read
   token from the same route, does the same):

   ```sh
   git -c http.extraHeader="Authorization: Bearer <read token>" clone http://localhost:<port>/git/a-<brief ID>.git brief
   git -C brief log --format='%h %s%n    %(trailers:key=Clax-Publisher,key=Clax-Agent,separator=%x2C )'
   ```

   Point at one commit per version, the merge notes in the subjects, and
   each commit's `Clax-Publisher` (Rae's person ID) and `Clax-Agent`
   (the agent that published it).

   > Every artifact is a git repository, and every version a commit on
   > main, tagged with its number, naming who published it and through
   > which agent.

2. Push a change. The read token cannot push, so in the Clone dialog
   choose **Write** and click **Make a new token**: under "An existing
   checkout" the git remote commands now end with a push. Commit, then
   push with the write token:

   ```sh
   cd brief
   # edit index.html: change <h1>Product brief: Trailhead</h1> to <h1>Trailhead</h1>
   git commit -am "Shorter heading"
   git -c http.extraHeader="Authorization: Bearer <write token>" push origin main
   ```

   Rae's top bar offers **Reload**; click it: the heading reads
   "Trailhead", and the version menu lists the new version, "Shorter
   heading". If an agent is revising at that moment (after shot 6's Always
   allow, summary-writer is), its terminal prints `conflict, v<n> landed
   first; merging` against the pushed version and publishes on top of it.

   > A push to main becomes a version. The Worker records the pushed
   > commit as the next version, and an agent that was working from the
   > version before is refused and merges onto it, like any other conflict.

Say over the Clone dialog, once:

> Each person gets their own short-lived token from Clax, minted from
> their rights: read for anyone who can read the page, its link included,
> and write for those who may publish it. A push is a version they
> published.

Optional, with `jj` installed and an identity set (`jj config set --user
user.name …`, `user.email …`): paste the printed `jj git clone` command
instead of the git one. jj clones and pushes are checked by
`crates/clax-worker/test-repos.mjs`, not by the demo.

### 9. Fork with lineage (8:00–8:30) [local]

On screen: Mia's browser. Open the demo's Fork link: under its title,
"Forked from Launch checklist v2", linked to that version. Then on the
brief, click **Fork**: the new artifact opens, "Forked from Product brief:
Trailhead v<n>".

> Anyone who can read an artifact, and may make their own, can fork it from
> any version. The fork is a new repository whose history ends at that
> version, and it names where it came from. Threads stay with the
> original.

### 10. On a deployment (optional) [deploy, optional]

Record only once a deployment with `REPO_STORE=artifacts`, the
`clax-artifacts-events` queue and its Artifacts event subscription has
passed shots 4, 6 and 8. On screen: the deployed instance's brief, then the
Artifacts namespace in the Cloudflare dashboard (one repository per
artifact, `a-<artifact ID>`, forks among them).

> The same Worker, deployed. Each artifact is a repository in Cloudflare
> Artifacts. The Worker writes each version as a commit over git's
> receive-pack protocol, and a push from anywhere arrives as an event on a
> queue and becomes a version.

### 11. What is next (8:30–9:00) [local]

On screen: the gallery.

> Next: running it against Cloudflare Artifacts on a deployment, which has
> not happened yet; syncing a laptop's
> Clax with the hosted one; and merging a fork back. The source is
> Apache-2.0, and `just hosted-demo` runs everything you saw on a laptop,
> without a Cloudflare account. Thanks.

## Recording notes

- The narration is about 800 words, about 5.4 minutes at 150 words a
  minute, which leaves about 3.6 minutes of the 9 for the agents, the
  browser and the terminal to run between lines. Shot 10 adds about 40
  words when recorded.
- The demo agents call no model: their text is fixed, so retakes match.
  Each run of `just hosted-demo` starts fresh, with new IDs, tokens and
  sign-in links; the version numbers in this script are from one run with
  the shots in order, and the merges can fall differently.
- Checked on the Worker under `wrangler dev --local`:
  `web/e2e/hosted-demo-worker.spec.ts` (`CLAX_E2E_WORKER=1 npx playwright
  test hosted-demo-worker` in `web/`) covers shots 3's pointers, 4, 6's
  delivery and hold (comments made through the API), 7, 9's seeded fork and
  8's clone, with a read token from Rae's Clone dialog, and its trailers.
  The rest was driven in a browser against
  `just hosted-demo` with the steps above: signing in with the printed
  links, Mia's selection and Rae's typing in shot 3, Show draft with its
  highlighted changes (shot 5), the comments made and sent through the UI,
  the Inbox item, the People page request and Always allow, the agent then
  receiving Mia's comment (shot 6), the push with a write token from
  the Clone dialog becoming a version behind Reload and an agent merging onto it (shot 8),
  and the Fork button with its lineage (shot 9).
- Shot 8 edits a clone made after shot 6. If an agent published after the
  clone, `git pull` (with the same header) before pushing, so the push is
  a fast-forward of main.
