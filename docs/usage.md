# Using and developing Clax

Clax is a local server for HTML artifacts that agents publish. It stores every version of each artifact and serves a gallery and viewer in your browser, where people comment on a page and send comments back to the agent that published it.

Comments and agents' replies are Markdown, with GitHub's tables, strikethrough, task lists and bare links, and shown rendered in the browser, in the thread and in an approval card alike; so is the text of an agent's question (its previews stay verbatim in monospace). Images show as links and HTML as text, and a link opens only to an `http:`, `https:` or `mailto:` address (`docs/contract.md`, "Comments and feedback").

## Prerequisites

Rust 1.94 (pinned by `rust-toolchain.toml`), Node 22, and `just`. The tests
also need git 2.44 or later on `PATH`: the git capture tests build fixture
repositories with it, and Clax captures nothing under an older git.

## Install

Install the plugin for your harness; that is all. The plugin downloads the
Clax release it pins on first use (see "Which clax the plugins run" below).

- Claude Code: `/plugin marketplace add empathic/clax`, then
  `/plugin install clax@clax`.
- Codex: `codex plugin marketplace add empathic/clax`, then
  `codex plugin add clax@clax` (a clone works in place of
  `empathic/clax`).
- Grok Build, from a clone: `grok plugin install <clone>/plugins/clax-grok --trust`.
- Pi, from a clone: `pi install <clone>/plugins/pi`.

In Claude Code the tools work in the same session once `/plugin install`
reports the plugin active; its `SessionStart` hook, which adds the daemon
URL and any waiting comments to the session's context, runs from the next
session on. In the other harnesses, start a new session afterwards.

In Claude Code's Manual mode, each Clax tool call asks first, `status`
included, until you allow the tools: the plugin does not approve them for
you, outside its own `/clax:` commands. Add
`"mcp__plugin_clax_clax__*"` (every Clax tool), or at least the read-only
ones, to `permissions.allow` in `~/.claude/settings.json` or a project's
`.claude/settings.json`; the Claude Code plugin's README ("Allowing the
tools") has the lines.

The first start of the plugin's MCP server
downloads the pinned release (about 10 MB) into `~/.clax/bin/<version>/`,
checks it against the checksum the plugin carries, and runs it; one download
serves every harness. Hooks never download, so a session's first hooks may
do nothing while the MCP server is still downloading.

### From a clone, with your own build

```
just install
```

It builds the web UI and `clax` (see Prerequisites), installs `clax` into
`$CARGO_HOME/bin` (`~/.cargo/bin` by default; `cargo install --locked --root
"$CARGO_HOME" --path crates/clax-cli`), and runs `clax init`. When your
agents' daemon (the one for `~/.clax`) runs that `clax`, `just install`
stops it with `clax stop`, so the next agent call starts it again from the
new build. Without that, a reinstall at the same version would leave the old
build running, since clients keep a daemon of their own version. A daemon of
any other executable, such as `just watch --shared`'s, is left running and
named. `clax init` writes the Clax plugins built into that binary to
`~/.clax/marketplace/`, registers them with each harness whose CLI is on
your `PATH` (Claude Code, Codex, Pi and Grok Build), and sets the `bin`
setting in `~/.clax/config.toml` to itself, so the plugins run your build
rather than the release they pin. The registration replaces any older one,
including registrations under Clax's previous name. Start a new session in
each harness afterwards. Moving or deleting the clone afterwards changes
nothing.

For Codex, `clax init` also offers, once, to approve the Clax tools that
replace or remove data (`delete` and the `db_*` writes), which Codex
otherwise stops to ask about mid-task: it prints the lines for
`~/.codex/config.toml` and adds them when you say yes (`--yes` for
scripts). The other tools run without asking. `plugins/clax/README.md`
("Tool approval") has the details.

For Grok Build, `clax init` installs the clax-grok plugin (`grok plugin
install <dir> --trust`) whenever `grok` is on your `PATH`, including when
Grok is the only harness installed; `clax uninit` uninstalls it. Grok also
discovers the Claude Code plugin from `~/.claude`; enabled there, that copy
stands down and only clax-grok's tools (`clax_grok__publish` and the rest)
act. Grok asks before each MCP tool call; for headless runs, pass
`--always-approve` or `--allow 'MCPTool(clax_grok__*)'`.

`just uninstall` reverses it: `clax uninit` removes the registrations (and
`~/.clax/marketplace/` once no harness refers to it) and the `bin` setting
when it names the installed `clax`, the agents' daemon is stopped when it
runs that `clax`, then `cargo uninstall clax-cli` removes `~/.cargo/bin/clax`.
It leaves `~/.local/bin/clax` alone, since that one comes from `install.sh`,
and never touches your data in `~/.clax`.

### The `clax` command on PATH, without a clone

```
curl -fsSL https://github.com/empathic/clax/releases/latest/download/install.sh | bash
```

`install.sh` puts the release's `clax` in `~/.local/bin` (or
`$CLAX_INSTALL_DIR`), after checking it against the release's `SHA256SUMS`.
The plugins do not need it. `clax init` then registers that binary's plugins
and points them at it, as `just install` does. GitHub serves release files
only while the repository is public; for a private repository both
`install.sh` and the plugins' download get a 404.

### Which clax the plugins run

Every plugin (Claude Code, Codex and Grok Build through
`scripts/ensure-clax.sh`, Pi through its copy of the same script) runs, in
order:

1. `CLAX_BIN`, when set: it must be a usable `clax`, or that is the error.
2. The `bin` setting in `~/.clax/config.toml` (`$CLAX_HOME/config.toml`),
   one line, `bin = "<absolute path>"`, before any table. `clax bin set
   <path>` (or `clax bin set --this`) writes it, `clax bin clear` removes it,
   and `clax bin` shows what the plugins run and why. `clax init` sets it;
   `clax uninit` clears it.
3. The release the plugin pins, installed in `~/.clax/bin/<version>/clax`
   with its sha256 in `clax.sha256` beside it. The wrapper checks that hash,
   then `--version`, before every run, and downloads the release again when
   either fails. After installing a release it removes older version
   directories but the newest of them, which a daemon started by the
   previous plugin may still be running from.

`PATH` is never consulted. A binary named by `CLAX_BIN` or the `bin` setting
whose version differs from the plugin's runs with a warning. When a plugin
half works, `clax doctor --agent <claude|codex|pi|grok>` checks each layer;
its `binary` check shows which `clax` the plugins run and why. If the MCP
server cannot run `clax` at all, its one tool, `status`, says why, and
`~/.clax/logs/hooks.log` has the details.

## Use

```
clax publish index.html --dir site   # publish a directory; prints the artifact URL (--json gives the ID)
clax open <ID>                       # open an artifact in the browser, signing it in
clax login [path]                    # print a one-use sign-in link for a browser
clax list                            # list artifacts (--tag <tag>, repeatable: only those carrying every tag)
clax tags                            # list the tags; the owner makes them: clax tags create|edit|delete
clax tag <ID> --add draft --remove x # add or remove an artifact's tags (existing tags only)
clax read <ID> --path style.css      # print a published file (--json gives the read tool's result)
clax asset upload <ID> photo.png     # upload assets; prints each asset URL
clax comments                        # open comment threads on every artifact, newest activity first (--all adds resolved)
clax comments <ID>                   # one artifact's threads
clax comments <ID>#2                 # show thread #2 in full (also: a thread ID)
clax comments reply <ID>#2 "Thanks"  # reply as you (- reads stdin); also resolve, reopen, send
clax comments name "Alex"            # your name, in the CLI and every browser of yours
clax versions <ID>                   # versions: label, publisher, time, threads addressed
clax db get <ID> tasks t1            # the page database: get, list, query, set, update, delete, str-replace, batch
clax status                          # show whether the daemon is running and which agents are working on what
clax doctor                          # check the home directory, daemon, database, and stored files
clax doctor --fix                    # also remove stray temp files and stale rows (never live artifacts' rows)
clax doctor --agent codex            # also check each layer of a harness's plugin: claude, codex, pi or grok
clax passkeys reset                  # with the daemon stopped: remove every confirmation passkey and stop requiring one
clax stop                            # stop the daemon
clax serve --bind 0.0.0.0            # serve connected people on the LAN (stop a running daemon first)
clax connect <invite URL>            # connect this machine to someone else's Clax as a person (--status, --off)
clax people                          # (the owner) list people, their rights, machines and agents, and the People link
clax agents                          # list your agents; clax agents rename <agent> <name> renames one
clax approvals                       # list whose content reaches your agents, and what waits for you
clax init                            # register the plugins with each harness (clax uninit removes them)
clax bin                             # show which clax the plugins run (clax bin set <path> | --this, clax bin clear)
clax haiku                           # print one of ten haiku about Clax
```

The daemon starts automatically on first use. Data lives in `~/.clax`; set `CLAX_HOME` to use a different directory. The daemon listens on port 7480, or on the `[serve] port` that the home's configuration sets; `CLAX_PORT` overrides it, and `--port` overrides both. When another program already holds that port, the daemon takes the next free one of the following 20 and its URLs say so; only when all are held, or when you chose the port with `CLAX_PORT` or `--port`, does the plugins' MCP server stop and name a free port to set instead. Clax never stops what holds a port.

Pages that declare `sample` ask Claude with an Anthropic API key on your machine. The daemon reads the key from `ANTHROPIC_API_KEY` when it starts; the `[sample]` table in `config.toml` changes that:

```toml
[sample]
provider = "anthropic"               # or "stub", which echoes the prompt (tests and demos)
api_key_env = "ANTHROPIC_API_KEY"    # the environment variable that holds the key
daily_call_cap = 200                 # optional: at most this many calls per artifact a day
```

Only your own browser spends the key: the page asks you to allow it once each time it loads, and the top bar counts today's calls. A viewer on another machine never can. Without a key, or with an invalid `[sample]` table, sample is off and pages hide the feature; `clax doctor` reports which on its `sample` line. In a checkout, `just demo-room-sample` publishes a room page and a sample page in a scratch daemon, with the stub provider unless `ANTHROPIC_API_KEY` is set.

A page that declares a capability reaching past itself (the internet, Claude, Clax's controls, notifications, sharing, files, the camera or your location) gets it only once the person who answers for the artifact approves it in Clax: on your own Clax, you as its owner; on a hosted Clax, the page's maker. Never an agent or the page. When a version declares something new, or a new `net` host, the artifact's view shows an approval bar to those who may approve: Review opens a dialog naming what the version asks for and what approving lets the page do, with Approve and Not now. Until it is approved the page's `use()` resolves `null` for it and the version before keeps what was approved. The Permissions panel (More › Permissions…) lists what the page declares and what is in force, with the network hosts, the Claude grant and the reads of other artifacts' documents, each with its revoke, and a Withdraw for an approved capability. Your approval also grants the network hosts and the Claude use that waited; a hosted maker's approval leaves those to your answer the first time the page uses them. The routes are in `docs/contract.md` ("Approving capabilities").

Every setting in `config.toml` can also come from the environment: `CLAX_<TABLE>__<KEY>`, a double underscore for each level (`CLAX_SERVE__PORT=7481`, `CLAX_SAMPLE__MAX_TOKENS=8000`, `CLAX_BOT__MODELS__DEFAULT=claude-opus-5-5`), which overrides the file, which overrides the defaults (`[hosted]` alone is read from the file only). An invalid table turns off only what it configures; `clax doctor` warns about it. The `[bot]` table configures ClaxBot (its provider, models, caps and credits; `docs/contract.md`, "`config.toml`"). The local daemon's `GET /api/settings` lists every setting with the value in effect and where it came from (`default`, `file` or `env:<NAME>`), and the value pending the next start when the file now differs, and `PUT /api/settings` writes the ones the file holds back into `config.toml`, keeping its comments; the owner and the admins the owner names may use either, and a change takes effect when the daemon next starts. ClaxBot runs in the daemon once a model key is set: `[bot] api_key_env` names the variable it reads the instance key from (`ANTHROPIC_API_KEY`), or an admin sets one at `/admin` (Provider); its runs, credits and audit are under `/me`, `/admin` and `/audit` (`docs/contract.md`, "ClaxBot routes"). A `[bot]` value set in the file or the environment overrides the admin pages' one and shows read-only there.

A new artifact needs a title: `--title`, or else the page's `<title>`. Updates keep the current title unless `--title` is given.

Tags sort the gallery. The owner keeps one list of them, each a name, a colour (`gray`, `red`, `orange`, `yellow`, `green`, `teal`, `blue`, `purple`, `pink` or `brown`) and an optional description: `clax tags create design --colour blue --description "Waiting on a design pass"`, `clax tags edit design --name "Design review"` (the artifacts carrying it follow the rename), `clax tags delete design` (removes it from every artifact). Anyone who may see an artifact sees its tags. The owner tags any artifact, and a person the artifacts they made; agents apply existing tags (`clax publish page.html --tags design,draft`, or the `tag` tool) and never make one: a tag that is not on the list is refused, naming the ones that are.

To serve connected people on the LAN, stop a running daemon first, then run `clax serve --bind 0.0.0.0`. A local daemon serves this machine: from another machine it answers only connected people's machines and browsers (People, below), and refuses anyone else (403 `forbidden_host`). Each artifact's page is served on its own site, `http://<artifact ID>.localhost:<port>`, which resolves on this machine alone, so the pages themselves open only here; a connected person's browser on another machine has the inbox, People and the comments, not the pages.

To serve the internet (hosted mode), put the daemon behind a proxy that terminates TLS and keeps the `Host` header, and name its public origin in `config.toml`:

```toml
[hosted]
public_origin = "https://clax.example"   # https, or http on localhost for development
site_domain = "clax-sites.example"       # each artifact is served at https://<artifact ID>.clax-sites.example; required to serve sites, no default
setup_secret_env = "CLAX_SETUP_SECRET"   # the default: the variable the setup secret is read from
behind_cloudflare = false                # true when Cloudflare is in front: clients are counted by CF-Connecting-IP
# Terms of Service and a Privacy Policy, optional: your own documents...
terms_url = "https://clax.example/legal/terms"
privacy_url = "https://clax.example/legal/privacy"
legal_version = "2026-10-19"             # what people agree to; change it to ask everyone again
# ...or, on the service Empathic operates, the ones in docs/legal: legal = "bundled"
# The Clax app, optional: the app links that open /a/<id>/get in it, and its store pages
app_links = { apple_team_id = "ABCDE12345", android_package = "dev.empathic.clax", android_sha256 = "AB:CD:…" }
stores = { ios = "https://apps.apple.com/app/…", android = "https://play.google.com/store/apps/details?id=dev.empathic.clax" }
```

`site_domain` is required for a hosted daemon to serve sites: there is no hosted default (`<ID>.localhost` is the local daemon's). The site domain is a domain of its own, neither the public host nor under it nor above it: point a wildcard DNS record (`*.clax-sites.example`) at the proxy, give it a certificate whose hosts include `*.clax-sites.example`, and have the proxy pass those hosts to the daemon too. Each artifact's page and its runtime are served on its own host, which only those its access allows can read (`crates/clax-worker/README.md`, "DNS for the public host and the site domain", describes the same on Cloudflare; `docs/hosted/public-suffix.md`, listing the domain on the Public Suffix List).

Start the daemon with the secret in that variable and open `https://clax.example/setup` to make the first person, the owner; without a secret, setup stays closed. From then on people sign in through the browser: the owner invites a person on People (a link, `/join/<code>`, good for a day), a signed-in person makes a one-use, ten-minute sign-in link for another device and agent tokens for their remote agents, and the owner shares a page by link, where anyone may read it and, once they give a name, comment as a guest. Nothing is served as "this machine" in hosted mode (the owner's browser holds a token of its session's, never the daemon token), a person's rights limit their browser as they limit their agents, a page's content is served only on its artifact's own host under `site_domain`, to those its access allows, setup, sign-in codes and guest names are rate-limited per client address, sign-ins and sign-outs are in the audit journal, and a bad `[hosted]` table stops the daemon from starting. Client addresses are the connection's peer unless `behind_cloudflare` is set: behind a proxy on the same machine every client is the proxy's loopback address, so they share one address's limits and its 128 streams and sockets, and the daemon logs a warning the first time it sees such a request. The Chrome extension works only with a local daemon, never a hosted one, and a hosted daemon shows live pages only to the owner (their browser and the daemon token). With Terms of Service and a Privacy Policy set, the sign-in, join and setup pages say that signing in agrees to them, each such sign-in records the person's agreement to `legal_version`, a signed-in person who has not agreed to it is asked once, and the gallery, People and the artifact view's more menu link both; without them, nothing is shown or asked. `legal = "bundled"` serves `docs/legal`'s documents at `/terms` and `/privacy`: they are Empathic's, for the service it operates.

An agent connects to a hosted Clax (clax.dev among them) as its person, with an agent token: on People, Agent tokens, Create token shows the token once, with the lines that add it to Claude Code (`claude mcp add --transport http clax <MCP URL> --header "Authorization: Bearer <token>"`), to Codex (an `[mcp_servers.clax]` table for `~/.codex/config.toml`) and to Grok Build (`grok mcp add --transport http clax_hosted <MCP URL> --header "Authorization: Bearer <token>"`; the name `clax_hosted` keeps it apart from the plugins' `clax` and `clax_grok` servers). Pi has no built-in MCP support, so its extension cannot take a token: it works with a Clax on your machine (or one it is connected to with `clax connect`), and that Clax publishes to clax.dev (Publishing to a public Clax, below). The plugins are for a Clax on your machine; with a token alone, an agent gets the tools without the `clax` skill or the plugin's hooks.

A new person's gallery on a hosted Clax starts with the Clax Quickstart, a pinned page of theirs that walks from sign-in to a first comment round trip. The site seeds it at the person's first sign-in (GitHub or a sign-in link) or when they accept an invite, into a gallery that never held an artifact. A local home is not seeded: its gallery starts empty, and a quickstart an earlier build seeded there stays. While nobody has touched a quickstart, each later sign-in (on a hosted Clax) and each start (for one a local home holds) moves it to the release this Clax ships, as a new version labelled `Quickstart <release>`; the first comment thread on it, a version or a change of its capabilities by anyone else, or an approval or withdrawal by a person stops its updates for good. `[gallery] quickstart = false` in `config.toml` (or `CLAX_GALLERY__QUICKSTART=false`) turns seeding and updates off.

The same page is a hosted Clax's landing page: a visitor who is not signed in and opens the site's root (`https://clax.dev/`) is sent to `/start/`, where the site shows the shell's minimal chrome (its header, with Sign in, or Your gallery once signed in; no threads or comments) around the quickstart this release carries, served as it ships at `/start/index.html`. Its Sign in with GitHub signs in on that site, in the same window. The page's reel (`/start/reel.mp4` and `reel-poster.jpg`) is served from wherever the Worker's `REEL_URL` and `REEL_POSTER_URL` name: Cloudflare Stream on clax.dev, the dev server itself under `just clax-dev-local`; without them the page shows no player. The gallery's other pages (`/inbox`, `/people`, …) still send such a visitor to `/signin`. A build without the quickstart, or a daemon with `[gallery] quickstart = false`, has no landing page: the root sends a signed-out visitor to `/signin`, and `/start/` is 404.

`app_links` and `stores` concern the Clax app, and every key is optional. `apple_team_id` (10 uppercase letters and digits, the team that signs the app) serves `/.well-known/apple-app-site-association` for `<team>.dev.empathic.clax`; `android_package` and `android_sha256` (the signing certificate's SHA-256 fingerprint, 32 hex bytes joined by `:`), given together, serve `/.well-known/assetlinks.json`. With them, a Get in Clax link (`/a/<id>/get`) opens in the Clax app on iPhone, iPad and Android; without them those files are 404 and the link opens the get page in the browser. `stores.ios` and `stores.android`, `https://` URLs, are offered on that page to someone without the app. App links work only for the domains the app's build declares, clax.dev alone; any other hosted Clax's get page offers Open in Clax (`clax:get?url=…`, on every platform) and Download .clax. The Cloudflare Worker reads the same settings from `APPLE_TEAM_ID`, `ANDROID_PACKAGE`, `ANDROID_SHA256`, `IOS_STORE_URL` and `ANDROID_STORE_URL` (`crates/clax-worker/README.md`).

### People

Other people can connect to your Clax, each with their own machines and
agents. Use a trusted LAN: Clax traffic is not encrypted, so anyone on the
network can read pages and comments as they travel, and a sniffed browser
cookie acts as its person in the browser.

- **Inviting.** Serve on the LAN, open People (`/people`) in your browser
  and choose **Invite a person**. Give them the command it shows, `clax
  connect http://<your LAN address>:<port>/i/<code>`: it works once, within
  24 hours. They run it on their machine, with Clax installed. An invite
  can also add another machine of yours.
- **Asking.** Someone can instead run `clax connect http://<IP>:<port>`.
  It shows a code and waits up to 10 minutes; a connection request comes
  to your inbox, and you approve it there with the code their machine
  shows, a name and the rights to give them (Reader, Collaborator or
  Full).
- **After connecting.** `clax connect` opens their browser signed in as
  them and asks whether your comments and pages may reach their agents.
  Their agents then publish, reply, resolve and write page data with the
  rights you gave, each on a key that never leaves their machine. `clax
  connect --status` checks the connection and `clax connect --off`
  disconnects it.
- **Agents.** Every agent has a name (from its repository and branch),
  shown wherever it appears, with its person beside it when it is someone
  else's. `@<name>` in a comment sends the thread to that agent. People
  shows each person's agents. An agent's person, or you, can rename it.
  Its person can narrow what it may do. You can raise it anywhere, and the
  creator of an artifact can raise it on that artifact.
- **Approvals.** What another person wrote is withheld from your agents
  until you approve them: for one thread, one page or everywhere. When a
  thread is sent to your agent, or your agent asks, an approval card comes
  to your inbox and shows above the thread. The card offers "Allow this
  thread", "Always allow … on this page", "Always allow … everywhere" or
  "Decline", and the thread reads "waiting for your approval" until you
  decide. People lists who reaches your agents and lets you revoke an
  approval. Approving happens only in a browser: `clax approvals` lists,
  and prints the People link.
- **Viewers who are not connected.** A browser on another machine that is
  not a connected person's is refused. Comments from viewers who are not
  connected people (a hosted daemon's guests) reach your agents without
  asking; the switch in People turns that off.
- **Revoking.** Revoking a person or one of their machines in People ends
  their agents' sessions at once. Their browser on another machine is
  refused from then on, and what they wrote stays theirs.

### Publishing to a public Clax

Your Clax can publish artifacts to a hosted one (a daemon in hosted mode, or
the Cloudflare Worker), where they get a public link; nothing changes for
your local Clax or its plugins until you add one.

```
clax remote add https://clax.example   # sign this Clax in there; approve it in the browser
clax remote list                       # which public Clax instances it is signed in to, and as whom
clax push <ID>                         # publish an artifact there, or push its new versions
clax push <ID> --to clax.example       # name the public Clax when there are several
clax push <ID> --follow                # also let agents push its new versions without asking
clax push <ID> --fork                  # the public copy moved: push onto a fork of it
clax push <ID> --new                   # or publish it again as a new public artifact
clax remote comments <ID> download-only  # keep threads started here local (two-way is the default)
clax remote unlink <ID>                # end an artifact's link; its threads stay here, read-only
clax remote remove clax.example        # forget it, revoking its token there
```

`clax remote add` asks the public Clax for a sign-in naming this machine,
prints a code and opens its approval page. Signed in there as the person to
publish as, check the code and approve: this Clax receives an agent token of
yours, which your People page there lists (named `Clax on <host name>`) and
revokes. Codes last ten minutes and work once.

An agent cannot add a remote (`clax remote add` under a harness answers
"This is for people, not agents: open … in a browser; agents: use `clax
remote request <url>`."). It asks instead:

```bash
clax remote request https://clax.example/a/7q3k9mzx2b4t --reason "to read the launch plan"
```

The person sees one card in the inbox (and in the agent's thread when it
was working in one): "<agent> wants to connect to <name> (<origin>) to
read <url>", with **Connect** (this Clax's device sign-in, approved on the
other instance in a new tab), **Not now** (no new request for a day) and
**Never for this instance** (muted until unmuted in Settings, Remotes).
The command asks as the agent's own Clax session, which it finds from the
harness (its session variable, else its process), or `--session`; it waits
(600 s, 50 s under Codex, or `--wait`) and prints the answer: `connected`,
`dismissed`, `muted`, `not_now` or `expired`, or `open` while the person
has not answered. Agents with the MCP tools use
`connect_instance` instead.

### Reading another Clax from an agent

Give an agent a page's URL on another Clax instance (clax.dev, a team's
own, or a colleague's) and its tools work on it there. Before this Clax is
connected to that instance, `read`, `comments_read` and `watch` answer
`remote_sign_in_required`, naming the instance and the `connect_instance`
tool; a private page there answers anyone else not found, with a hint to
sign in or to connect. The agent then calls `connect_instance` with the
URL and a reason, and waits:

1. The connection request appears as a card in your inbox, naming the
   agent, the instance and the page.
2. You press **Connect** in your browser. While any agent of yours is
   live, the daemon token alone cannot answer it: it is your browser's
   decision (and, with passkeys turned on in Settings, your passkey's).
3. A tab opens on the other instance's device sign-in. Signed in there as
   the person to read as, check the code and approve.
4. `connect_instance` answers `connected`.

From then on the agent reads that instance's pages through your Clax, as
the person who approved: `read`, `open` and the comment tools answer as
for a local artifact, with the page's address there and the instance's
name, and anything that would change a page there (`publish`, `db_*`,
`delete`, …) answers `not_on_remote`. `watch` follows the page: the agent
appears there by its name, so a person there can send it a thread, and
what they send reaches it here as a local comment does (`wait_for_feedback`,
the `follow_command` the watch answers, channel notifications). Revoking
this machine's token on your People page there signs it out; the next
call answers `remote_sign_in_required` again.

Nothing of this needs the other instance to be on the internet: one on
this machine (`just clax-dev-local`) connects over `http`, as loopback is
allowed; any other is `https`.

A push publishes every version the public copy has not had, in order, with
its label, note and files. The public versions are yours there: each names
the local agent that made it, and the ones you published by hand read as
yours by hand. An asset a pushed page loads by the address Clax gave it
(`/_blob/<ID>`) is uploaded to the public copy and the page's references
rewritten to its address there; other assets stay on your machine. A push whose answer was lost can be sent
again: the public Clax answers the version it already made. When an
automatic push (`--follow`) is refused, your inbox says why.
If someone published on the public copy since your last push, the push is
refused and nothing is overwritten: push onto a fork of it, or as a new
artifact.

Once pushed, the artifact's comment threads are linked both ways: a comment
on the public copy shows up on your local page tagged with where it came
from and who wrote it there, reaches your agents as a local comment does
(`wait_for_feedback`, channel notifications), and their replies and resolves
go back to the public thread, once each. Threads you start locally go up
too, as you, unless the link is download only (a thread started while it
is download only stays local, even once it is two-way again). While the
public Clax cannot be reached, what you write waits, marked "Sending…",
then "Not sent" with Retry after five minutes; nothing local waits on it.

On an artifact's page, Publish to `<host>` does the same in the browser
(and Push v`<N>` once it is public, beside a link to its public copy). Your
Clax shows it on every artifact, even before it is signed in anywhere: then
it reads Publish to clax.dev, and pressing it signs this Clax in there
first, as `clax remote add https://clax.dev` does: the tab goes to the
approval page there, and once you approve it comes back to the artifact
with the Publish dialog open, asking to publish (or saying the sign-in was
declined). A public Clax too old to send you back leaves the approval page
to a link in the dialog, which waits for your approval. An
agent publishes with the `publish_public` tool, which asks you first: the
question waits in your inbox beside the artifact, and only your answer in
the browser or the extension counts. Choose Publish and push new versions
to let that artifact's later versions go out without asking again.

### Apps on clax.dev, from the Clax app

The Clax app publishes your apps to a hosted Clax (clax.dev, or another),
shares them there, and gets them onto your other devices and other
people's. `docs/contract.md` ("Sharing apps", **Linked apps**; "Publishing
to a public Clax", **Apps from the Clax app**) has the details.

- **Connect.** Settings › Sharing › clax.dev › Connect (it also starts the
  first time you choose Publish to clax.dev… or Get my apps). The app
  shows a code and opens the approval page; signed in there, check the
  code and approve. The app is then a device of yours there: People lists
  it under Devices, where you can revoke it, and it may also change who can
  see your pages and whether anyone may comment on them. Disconnect
  forgets clax.dev here and asks it to revoke the device; when clax.dev
  can't be reached, the app is disconnected all the same and the device
  stays listed there, under People › Devices, until you revoke it there.
  Your public copies stay up.
- **Publish.** Publish to clax.dev… on an app's long-press menu, or on its
  Overview. The first publish shows what goes up (the version, its files)
  and what stays here (earlier versions, threads, your API key, and your
  data unless you turn on Include my data; later versions go up without
  your data). The app goes up Restricted,
  only you until you share it, with comments off. Later versions go up
  only when you publish them (tap the "↑ v<N> here" pill on the app's
  screen, or pick a version in Versions), or each as it lands with
  Publish new versions set to Automatically. Unpublish… on Overview deletes the public
  copy; copies people installed keep working without updates.
- **Share.** Share… (after publishing, on Overview and the long-press
  menu) works as the Share dialog on clax.dev, and changes the same
  setting: add people by GitHub login with Can view, Can comment or Can
  edit, set General access (Restricted, Anyone with the link, Public), copy
  the link, and turn Comments on or off. People with Can edit may publish
  new versions too; only you, as its owner or maker, share it.
- **Get.** Get my apps (in the home screen's More menu and its empty state;
  on the Mac also File › Get My Apps… and the home toolbar)
  lists the apps you published there and the ones shared with you, to
  pick and bring down; they come without their data. A Get in Clax link
  does the same for one app. Each app you get is linked to its public
  copy: the app checks it when it comes to the front (at most every 15
  minutes) and when you open the app, and an update shows ↓ and "v<N> is
  ready" with Update. Nothing changes until you choose Update, or turn on
  Update automatically for that app, which still asks when an update needs
  a new permission. An update that fails its checks on your data leaves
  your app as it was and, when you may comment there, offers Tell @owner.
- **Someone else's app.** Before Claude first reads an app someone else
  wrote, to change it or to combine versions, the app asks "Claude reads
  what <author> wrote": an app's code and content can sway what Claude
  does, so continue only if you trust its author. It asks once per app
  and author, and again if someone else publishes it. When you change an
  app you got, its Overview card reads ⑂ Changed here: it is your fork
  until you take an update, and an update offers Update (replaces your changes),
  Keep mine, or Bring v<N> into mine, where Claude adds the update to your
  version. **Publish fork…** puts your version up as a new app of your own,
  named "<title> (yours)" unless you rename it.
- **Changed in both places.** When a version is published on the copy
  (from another device or the web) after one you have not published here,
  its Overview card reads ⑂ Changed in both places (the pill on its
  screen, Diverged), and Resolve… offers: ask Claude to combine them, take the public version (yours stay in Versions), or
  publish yours as a new app.

On clax.dev, a published app's page shows its byline ("v8 by @editor ·
by @owner"), installs and forks, and **Get in Clax**, which opens its get
page, `/a/<id>/get`. On a phone with the app the link opens it in the
app; on a computer the page shows a QR code to scan with the phone, Open
in Clax (which the Mac app takes), Download .clax, and the store pages
for someone without the app. A page made on the web, with no version
published from the app, is not an app: its get page says so.

## Use from an agent

Each harness gets the same thirty-nine tools (`publish`, `read`, `list`, `delete`, `open`, `pin`, `unpin`, `tag`, `tags`, `fork`, `asset_upload`, `status`, `comments_read`, `comments_reply`, `comments_resolve`, `comments_handoff`, `watch`, `wait_for_feedback`, `working`, `draft`, `git_remote`, `ask`, `where_to_ask`, `connect_instance`, `publish_public`, `db_get`, `db_list`, `db_query`, `db_set`, `db_update`, `db_delete`, `db_str_replace`, `db_batch`, `queue_list`, `queue_claim`, `queue_renew`, `queue_release`, `queue_reply`, `queue_resolve`) and the `clax` skill. Install the plugin for each harness (see Install); plugin details: [plugins/claude-code/README.md](../plugins/claude-code/README.md), [plugins/clax/README.md](../plugins/clax/README.md), [plugins/pi/README.md](../plugins/pi/README.md), [plugins/clax-grok/README.md](../plugins/clax-grok/README.md).

The plugins run `CLAX_BIN`, else the `bin` setting, else the release they
pin (see "Which clax the plugins run"). `status` and `clax doctor --agent`
report the binary and its version. `~/.clax/logs/hooks.log` has a line for
every hook run, every MCP start and every install.

Comments wake an idle Codex or Pi session on their own. An idle Claude Code
session wakes in one of two ways. Launched with
`claude --dangerously-load-development-channels plugin:clax@clax` (Claude
Code channels, a research preview: CLI only, claude.ai or Console login,
and on Team and Enterprise an Owner must turn channels on), Clax sends it
a notice through the channel. Otherwise the skill keeps a background
`clax feedback follow --once` running after a publish, and its exit wakes
the session. A notice only points at the comment. The comment itself still
arrives once, through the next clax tool call, the end of the turn, or
`wait_for_feedback`. `status` and `clax doctor --agent claude` show which
path a session uses.

## Documentation

[docs/contract.md](contract.md) is the contract for agents and integrators: every tool's arguments, results and error codes, how sessions are identified per harness, the page contract, the security model, and what is not yet available.

[docs/kubernetes.md](kubernetes.md) deploys hosted Clax instances on Kubernetes, each on its shard pod's volume and scaled across pods (`clax-fleet`, `deploy/k8s`).

## Upgrading

Pages viewed before this version may still be cached by the browser (older daemons marked supporting HTML pages immutable, and Chrome can keep serving such a copy inside the artifact frame even after "Clear site data"). After upgrading from such a daemon, clear "Cached images and files" for "All time" once, at chrome://settings/clearBrowserData. From this version on, every HTML page is revalidated on each load and the bridge URL names its version, so upgrades never need this.

### From an earlier source install

Earlier versions guessed which `clax` to run: from `PATH`, `~/.local/bin`,
`~/.clax/bin`, or a checkout's `target/`. Now the plugins run `CLAX_BIN`,
else the `bin` setting, else the release they pin, and never look on `PATH`.
In the checkout, run `just install`, which also sets the `bin` setting to
the installed build.
It re-registers every harness from `~/.clax/marketplace/`, which replaces
registrations that pointed at an old or moved checkout, and removes those
under Clax's previous name. Then remove binaries that nothing should run:
check `which -a clax`, and delete an old `~/.local/bin/clax` or
`~/.clax/bin/clax`. The plugins no longer read `CLAX_SOURCE_DIR`; unset it
wherever you set it. Check with `clax doctor --agent <claude|codex|pi|grok>`.

A newer daemon is never replaced by an older `clax`. After installing an
older version, run `clax stop` once. An older daemon is replaced by the
Claude Code and Codex plugins' MCP server and by `clax serve`, not by Pi or
the other CLI commands, so after an upgrade that came from `install.sh`, a
machine that uses only Pi or the CLI keeps the older daemon until `clax
stop`. `just install` stops it itself.

## Developing Clax

Checks and builds run with Bazel when Bazelisk is installed and with Cargo
and pnpm otherwise; `docs/bazel.md` has which recipe runs what, the
`-native` twins, `just bazel-sync` and `just bazel-gc`.

The Rust workspace is layered, each crate depending only on the crates
above it: `clax-core` (shared types and pure rules), then `clax-store` (the
SQLite store), `clax-sample`, `clax-mcp` and `clax-hooks`; `clax-realtime`
(in-memory delivery); `clax-http` (what every route shares); `clax-server`
(the routes and `serve()`); `clax-commands` (every subcommand);
`clax-embedded` (the files the binary carries); and `clax-cli`, the binary
that wires them together. Test support is `clax-testkit` (the in-process
test daemon, for integration tests only) and `clax-fake-exe`. Beside
`clax-core` sit `clax-rt` (the async runtime seam: tokio natively, a
Worker's event loop on wasm32) and `clax-git` (git objects, packs and smart
HTTP, without I/O), with `clax-content-git` (artifact content kept in git)
over both; `clax-bot-sandbox` (the contract of ClaxBot's code-mode
sandbox and the conformance suite every sandbox passes, for both targets)
depends on no workspace crate, and `clax-bot-js` (the native sandbox, over
embedded QuickJS) on it alone; `clax-bot` (ClaxBot's run loop: the
`clax` script API, prompts, the step state machine and its driver) sits
over `clax-store`, `clax-sample` and `clax-bot-sandbox`, `clax-http` over
it, and `clax-server` over `clax-bot-js` too, an optional dependency
behind its default `native-sandbox` feature (without it, as in the
crates.io package, scripts answer `unavailable`); the hosted Worker,
`crates/clax-worker`, is a workspace of its
own over the daemon crates. `clax-fleet` (Clax instances across
processes, the server of the Kubernetes deployment, `docs/kubernetes.md`)
sits beside `clax-cli`, over `clax-server` and `clax-embedded`. `scripts/check-crate-layers.py`, a quality
gate, holds the rule crate by crate (its `ALLOWED` map) and fails a
dependency out of order.

Two loops, both on a home of their own, `~/.clax-dev`, whose daemon listens on
port 7481 (its `config.toml` says `[serve] port = 7481`, or the port in
`CLAX_DEV_PORT`, written by whichever of `just watch` and `just dev` runs
first). Your agents' home, `~/.clax`, their daemon on 7480, and the `clax`
they have installed are never touched.

**The daemon and the web UI: `just watch`.** An auto-reloading server at
http://localhost:7481, which can be left running. A Rust change rebuilds and
restarts the daemon; a web change rebuilds `web/dist` (reload the browser).
Extra arguments go to `serve` (`just watch --bind 0.0.0.0`). Ctrl-C stops
everything. `CLAX_DEV_PORT=<port> just watch` uses another port, and a
`CLAX_HOME` you set is used as the home. `just watch --shared` serves the
agents' home, `~/.clax`, on 7480 instead: stop their daemon first with
`clax stop`, and expect an agent to start its own daemon there while a Rust
change rebuilds. `just dev` with no harness, or with an option first
(`just dev --shared`), runs `just watch`. `just serve`, `just stop` and
`just doctor` run the checkout's build on `~/.clax-dev` (`just serve` on
7481). `just watch` needs `cargo-watch` (`cargo install cargo-watch`).

**An agent on your working tree: `just dev claude|codex|grok|pi`.** It builds
`clax`, copies it into a temporary directory put first on `PATH` (removed
when the session ends), and starts the harness on `~/.clax-dev`:

```
just dev claude
just dev codex
just dev grok
just dev pi
just dev claude --resume  # extra arguments go to the harness
```

- Claude Code loads `plugins/claude-code` straight from the checkout
  (`--plugin-dir`), and an installed `clax@clax` is disabled for that
  session.
- Pi loads `plugins/pi/src/clax.ts` and its skill from the checkout. It runs
  with `-ne` so an installed Clax package does not load twice, which turns
  off your other Pi extensions for that session too.
- Codex cannot load a plugin from a directory, so `just dev codex` runs your
  installed Clax plugin, with your own `~/.codex` as it is, on the fresh
  build and `~/.clax-dev`. To try plugin, skill or hook changes in Codex, run
  `just install`.
- Grok works like Codex: `just dev grok` runs your installed clax-grok
  plugin, with your own `~/.grok`, on the fresh build and `~/.clax-dev`. To
  try plugin, skill or hook changes in Grok, run `just install`.

**A second person: `scripts/dev-second-person.sh <invite>`.** To try People
(`/people`) and the other multi-person surfaces alone, run the dev daemon on
the LAN (`just watch --bind 0.0.0.0`), open People as the owner
(http://localhost:7481/people), choose **Invite a person**, and pass the
command's invite URL to the script. It connects a scratch Clax home (never
`~/.clax` or the dev home) as that person's machine, prints the browser link
that makes a browser that person (open it at the LAN address: there the
pages themselves do not open, as an artifact's site, `<ID>.localhost`, is
this machine's), and registers one agent of theirs. `scripts/dev-second-person.sh --off <home>`
disconnects it and removes the home.

**Connecting to a dev clax.dev: `just remote-e2e`.** The end-to-end check of
reading another Clax (`demo/remote-e2e.ts`): it builds the web UI, the
Worker and the daemon, starts the Worker with `just clax-dev-local`'s
settings and a daemon on a scratch home under `target/remote-e2e/`, each on
a free port, and walks the path above with a scripted agent (`clax mcp`):
`connect_instance`, Connect from an owner browser session, the device
approval there, a `read` of a private page, a `watch`, and a comment there
reaching `clax feedback follow --once`. Each step prints a line; it exits 0
when all pass, and stops everything it started (`--no-build` reuses the
last builds, `--keep` keeps the run directory).

Edit the plugin, skill or hooks, then start a new `just dev` session to pick
them up. A dev-home daemon left by an earlier `just dev`, whose temporary
binary is gone, is stopped at the next `just dev` or `just watch`, which says
so; a `just watch` daemon is left alone, and `~/.clax` and port 7480 are never
touched. `just install` puts the working tree in front of your everyday agents
(their daemon restarts on the new build at their next call), and `just
uninstall` takes it away again.

- `just help` (or bare `just`) lists every recipe with a description.
- `just test-affected` (`scripts/affected-tests.sh`) runs the Bazel tests
  downstream of your changes, chosen by the build graph: the working
  tree's edits, staged or not, and new files, against `HEAD`. A base
  (`just test-affected main`) adds every commit since its merge-base; a
  range (`<sha>^..<sha>`) takes its commits alone, without the working
  tree. Each changed file is its own label, a
  changed BUILD file or a removed file selects its package, and the tests
  are those that depend on them, less the `manual` ones. A change to the
  build itself (`MODULE.bazel`, its lockfile and the files it names,
  `.bazelrc`, a `.bzl` file, `bazel/`, `Cargo.lock`) runs every test. It
  names the files no target uses, which select nothing; `--list` prints
  the tests without running them, and other arguments go to `bazel test`.
- `just check` formats the Rust code, then runs every quality gate.
- `just ci` runs the same gates CI runs, without formatting.
- `just perf` runs the perf benchmarks. Performance is measured on demand
  and gates nothing: no check runs them, and they report a measure over its
  budget without failing.

Each crate's integration tests are one test binary, `tests/integration.rs`, with
a module per file in `tests/`; the few that must not share a process with
the rest have a second binary (`clax-cli/tests/descriptors.rs`). On macOS every new test
binary is assessed on its first run, after each rebuild. A new file in `tests/`
runs once a binary's root declares it (`mod name;`); a test checks that each
one is. To run
one file's tests: `cargo nextest run -p clax-server -E 'test(/^api_batch::/)'`,
or `cargo test -p clax-server --test integration api_batch::`.

`scripts/quality_gates.sh` runs every check CI runs: the justfile, plugin wrapper (`scripts/test-ensure-clax.sh`, `just wrapper-test`), release script, release command, release installer and dev script tests (`scripts/test-release.sh`, `scripts/test-release-script.sh` (`scripts/release.sh` against fakes of `gh`, `just` and Bazel), `scripts/test-install.sh` (`just install-test`), `scripts/test-dev.sh`), the Clax app's build script test (`scripts/test-build-app-clax.sh`, a dry run with no build), and plugin structure tests (`scripts/test-plugins.sh`, which also checks that the four skill copies and `docs/contract.md` share the page contract word for word, that the skill copies share the comment loop word for word, that the plugins wire their Stop and prompt hooks, that the workspace, the plugin manifests, the Pi package and the plugins' wrapper's `CLAX_VERSION` carry one version (`scripts/check-version.sh`), that the four plugins carry the same wrapper and pin the newest `v*` release tag, that the Rust and Pi tool descriptions match `plugins/pi/test/fixtures/contract.json`, and, through `scripts/sync-skill-tools.py --check`, that each skill's generated tool block and the tool lists in `docs/contract.md` and the READMEs name exactly that fixture's tools), the web lint (`oxlint`, configured in `web/.oxlintrc.json`), web typecheck and unit tests, the web build (before the Rust tests and the release build, which serve or embed it), `cargo fmt`, clippy with `-D warnings` over every target, the Rust tests (`cargo nextest`, or `cargo test` with a warning where nextest is not installed: `cargo install --locked cargo-nextest`), the comment-loop smoke (`scripts/smoke-comment-loop.sh`: a scripted Claude Code session, no model, through tiers 1, 2, 4 and 5 with a fake `codex`), the Pi extension's typecheck and tests, the Worker's checks (`crates/clax-worker`, whose format check, tests (the SQLite VFS's among them) and clippy are the workspace's: `scripts/worker-checks.sh`; `just worker-test` runs them all, under Bazel, `just worker-test-native` with Cargo and pnpm): `test-deploy-config.mjs`, then `test-wasm.mjs`, which builds the wasm Worker into `target/clax-worker/build`, writes through it under `wrangler dev --local` on free ports with fresh state under `target/clax-worker/wasm-test/`, restarts wrangler and checks the data and `PRAGMA integrity_check`, then `test-sandbox.mjs`, ClaxBot's sandbox conformance suite against the Worker Loader sandbox on that build (`crates/clax-worker/src/sandbox/`); and, after the web e2e tests, `e2e.mjs`, which runs the Clax server in its Durable Object under `wrangler dev --local` with the web UI from `web/dist` and checks in two browsers that a published artifact shows in the gallery and opens, that a comment reaches the other browser live over the stream's WebSocket, also after the server is rebuilt as after hibernation, and that everything is there after a restart), the Clax app's gate (`apps/clax_app/tool/check.sh`, skipped where `flutter` is not installed; see "The Clax app" below), and the Playwright end-to-end tests of the web UI. `just web-test` runs the web lint, typecheck, and unit tests.

The web UI is built first (`scripts/build-web.sh`, which keeps the modification times of output it did not change, so an unchanged web UI does not rebuild the release binary that embeds it) and its unit tests run next, alone, since some are timing-sensitive. The other gates then run in concurrent lanes (web e2e then the Worker's e2e, the Rust tests, clippy and `cargo fmt`, web lint and typecheck, the release build, Pi, the scripts, the Worker, the wasm32 check, the Clax app), the longest first, each printing a line as it finishes; a failed gate prints its output. The Rust tests are built before clippy runs, so they start as soon as they can. `pnpm install --frozen-lockfile` runs in `web/`, `plugins/pi/` and `crates/clax-worker/` only when `node_modules` was not installed from the current `pnpm-lock.yaml` by the same Node and pnpm (a hash in `node_modules/.clax-install-stamp`; `CLAX_PNPM_INSTALL=always` ignores it). One debug `clax` is built for the whole run and given to the tests that cannot name it and to the browser tests' daemons as `CLAX_TEST_BIN` (`just test` does the same for the Rust tests), and one release `clax` to the perf gates as `CLAX_PERF_BIN`. The perf gates then run alone, one at a time. The run ends with each gate's time and the total; a slow gate is reported, never failed. On a warm build cache and an otherwise idle machine, the whole run takes under two minutes.

The perf benchmarks, run on demand by `just perf` and by no check: each prints its measures beside its budgets, scaled by the run's own idle latency, and reports a measure over budget without failing:

- daemon latency (`scripts/perf-daemon.sh`): a release `clax` on a scratch home seeded with 300 artifacts, 2,400 threads and 7,200 comments by one viewer; while a gallery tab refreshes, ten galleries load, 12 MB `docs:batch` writes, long polls and event streams, and 16 MB publishes run in turn, `/healthz`, `/a/<id>`, `/c/<id>/v/1/` and `/api/artifacts/<id>` must keep a p95 of 50 ms and a max of 250 ms, and the gallery's list and attention requests alone, each timed against a fixed SQLite calibration read that the daemon runs on its own store workers, interleaved with it, must stay within about 1.4 times their measured ratios to it, so either query getting twice as slow fails on a fast machine, a slow one or a loaded one; medians over three rounds of 3 s windows per load (0.5 s in the quick mode), limits scaled by the run's own idle latency, and under the ten galleries to at least twice the galleries' own mean request latency; with two people connected by `clax connect` (6,000 comments by four authors, 50 approvals each), a person's agent's threads read and feedback poll (half the rows withheld) within 2 ms and 1 ms of the single-owner baseline that `scripts/perf-vis-baseline.sh <commit>` measures, the gateway's median signature verification within 150 µs, and a signed client polling beside every load; budgets in `scripts/perf-daemon-budget.json`;
- realtime clients (`scripts/perf-clients.sh`): 1,000 `/api/stream` clients on one release daemon, judged on delivery latency, cheap requests under load, memory per client, idle CPU and a client that never reads; a 16 s idle and a 15 s load window (3 s and 9 s in the quick mode: the load makes at least 40 writes, so one write held up by its commit cannot set the delivery p95 alone), budgets in `scripts/perf-clients-budget.json`;
- time to usable (`cd web && pnpm run perf`): link to first paint and to comment mode in a fresh tab, and link to the shell's first `ready` to the Clax app's host (`/a/<id>?app=1`, the end of the app's say → usable time), per frame mode, against `web/perf/budget.json`; the quick mode (`CLAX_PERF_QUICK=1`) takes 5 samples per mode instead of 9.

The numbers are kept in a perf log, one JSON line per run: the commit and
date, the runner, its CPU count and load average, and every measure (the
benchmarks above and the bundle bytes) beside its budget. Each benchmark
writes its measures to `CLAX_PERF_JSON` when it is set, and
`scripts/perf-log.sh record` makes the line. `just perf` prints it and
appends it to `target/perf-log.jsonl`, and never pushes. CI's `perf-log`
job runs the benchmarks after every green push to main, and its
`perf-log-publish` job appends the line to `perf/log.jsonl` on the orphan
branch `perf-log`; a number never fails either. `scripts/perf-log.sh show` fetches that branch and prints each
measure's trend over the last runs (`--runs N`, default 10; `--file
target/perf-log.jsonl` for the local log), marking a value over its
budget.

`clax-core`, `clax-rt`, `clax-mcp`, `clax-server` and `clax-content-git`
also build for `wasm32-unknown-unknown`, the target a Worker runs Rust as. `just
check-wasm` (`scripts/check-wasm.sh`) checks them there and fails on any
warning; arguments, when given, go to `cargo check` in place of that crate
list. The target alone decides what is built (`cfg(target_arch =
"wasm32")`, no cargo feature); `clax-server`'s crate documentation says
what the daemon alone runs and how the server reaches its runtime through
`clax-rt`. Bundled SQLite
for that target is C built by a clang with a wasm32 backend, which Apple's
clang lacks: the script takes `$LLVM_BIN`, else Homebrew's LLVM (`brew
install llvm`), else the `clang` on `PATH`, else the newest Debian or
Ubuntu `/usr/lib/llvm-<N>`, and fails saying what is missing (that LLVM, or
the target: `rustup target add wasm32-unknown-unknown`). `.cargo/config.toml` sets the
`getrandom_backend="wasm_js"` cfg for the target. On it the store runs in
single-connection mode (`Store::open_on`), and what needs a local machine is
built on unix only: the tool-call journal's real file system
(`toolpath::segment::StdFs`), git discovery (`gitctx::find_git` finds none),
`file://` links in exports, and file modes on the home's directories and
private files. The gates run this check in a lane of their own (`wasm32`),
so a machine running them needs that LLVM and the target.

One run of the gates at a time per checkout: a run holds a flock(2) on
`quality-gates.flock` in the checkout's git directory, a second run in the
same checkout waits for it, and the lock goes with its run however that run
ends, a kill -9 included. Separate worktrees still run in parallel.

`scripts/verify-harnesses.sh` (manual) checks `clax init` and `clax uninit`
against the real `claude`, `codex` and `pi` CLIs, entirely inside a scratch
root it deletes on exit, and prints a PASS/FAIL table. It tests `CLAX_BIN`,
or a fresh `cargo build`, never your installed `clax`; it refuses to run when
a scratch directory would fall inside your home, never reads or copies an
auth file, and runs Clax on a scratch port. The Pi session check, which needs
a model, runs only with `VERIFY_PI_SESSION=1` and a provider key you export.

### The SDK package

The reporter client, the embeddable overlay and the service client are
also built as an npm package, `@empathic/clax-sdk`, into `web/dist-sdk`
(`web/scripts/build-sdk.mjs`): ES modules with Svelte and the overlay's
other dependencies bundled, and declarations under `types/`. It has three
entries: `@empathic/clax-sdk` (`ReporterClient`, `ClaxError` and the queue
routes' types), `@empathic/clax-sdk/overlay` (`overlay`, the same overlay
as `/_clax/overlay.js`) and `@empathic/clax-sdk/service` (`ServiceClient`,
for a backend only). Its license is LGPL-2.1-only, that of `web/sdk`,
`web/overlay` and `web/bridge`.

- `just web` builds it beside the web build (`//web:dist_sdk` under Bazel,
  copied into `web/dist-sdk`); `pnpm run build:sdk` in `web/` builds it
  alone. `pnpm run build`, the web UI the binary embeds, does not build it,
  so a failure of the package's build fails no build of `clax`.
- `node scripts/build-sdk.mjs --watch`, in `web/`, rebuilds it on every
  change to its sources. `just watch` runs it natively; with Bazel it does
  not, so run `just web` again after a change (or the watch command, with
  `web/node_modules` installed).
- `//web:sdk_package` (`pnpm run test:sdk` after a build) checks the
  package as an app gets it, and `//web:bundle_size` holds its overlay
  entry to the `sdkOverlay` ceiling.
- A release publishes it to npm at the release's version
  (`.github/workflows/publish-sdk.yml`, step 21 of `just release`;
  "Releasing" below).

To use the checkout's build from another app while developing, link the
directory, which keeps the package's own name:

```
"@empathic/clax-sdk": "link:/path/to/clax/web/dist-sdk"
```

in that app's `package.json` `dependencies` (pnpm and bun both read
`link:`), then install. A link is a symlink to the directory, so a rebuild
reaches the app without reinstalling; reload it, or restart its dev server
if that server cached the package.

### The Clax app

`apps/clax_app` is a Flutter app for Android, macOS, iPhone and iPad. It
runs Clax's core, the store and router `clax serve` runs, as a library
inside its own process (`crates/clax-embed`, behind four C functions the
app calls by FFI), with a home of its own under the app's support
directory (`clax-home/`): nothing listens on a port and no other program
runs. It shows each app full screen in a WebView that loads from a scheme
the app answers, and runs its own agent: the Claude API with your key,
or, in a desktop debug build, Claude Code. What it relies on from the
core and the shell is in `docs/contract.md` ("The Clax app's
agent", "The Clax app", "Sharing apps", "Device capabilities in the Clax
app", "Reading another app's documents", "Fetching from the network",
"Security model").

**Toolchains.** With Bazel (as for the rest of Clax) nothing needs
installing but Bazelisk and, for macOS, Xcode: Bazel fetches Flutter
3.47.7 and every pub package of `pubspec.lock`, the Android SDK (build
tools 36.0.0, platform 36, platform tools), NDK r28c, a JDK, the Maven
libraries the plugins need and the Rust targets, each by checksum, into
the cache every worktree shares. `docs/bazel.md`, "The Clax app", says
what builds what and how to upgrade each piece. The one input from this
machine is the Android debug key, `~/.android/debug.keystore`, which signs
the APK so it installs over one already on a phone.

The `-native` twins use Flutter, Gradle and Xcode as installed: Flutter
(stable; 3.47 is what it is built with), Xcode for macOS, and for Android a
JDK 17, the Android SDK (platform 36, build tools, platform tools,
command-line tools) and NDK 28.2.13676358. Point Flutter at them once, and
accept the SDK licences:

```
flutter config --jdk-dir "$(ls -d ~/Library/Java/jdk-17*/Contents/Home | head -1)"
flutter config --android-sdk ~/Library/Android/sdk
flutter doctor --android-licenses
flutter doctor
```

`scripts/build-app-clax.sh` (the native build of the embedded core)
takes the NDK from `ANDROID_NDK_HOME`, else
`~/Library/Android/sdk/ndk/28.2.13676358`, and adds the Rust targets
`aarch64-linux-android` and `x86_64-linux-android` itself.

**Recipes.** Each runs Bazel when it is installed and has a `-native` twin
(`just app-test-native`, `just app-apk-native`, …) that runs the native
tools as before; `scripts/run.sh --dry-run <recipe>` says what one would
run.

- `just app-test` runs the app's checks: `dart analyze` over the app's
  Dart (`//apps/clax_app:dart_analyze`, by
  `apps/clax_app/analysis_options.yaml`), each test file of `test/` as its
  own target (`//apps/clax_app:<dir>_<file>`; the `*_real_test.dart` files
  against the host's build of the embedded core), the prompt copies
  (`//apps/clax_app:prompt_sync`, `scripts/sync-app-prompt.sh --check`,
  which checks that the prompt assets in `apps/clax_app/assets/prompt`
  still copy `docs/contract.md`'s "Page contract" and "Runtime
  capabilities" sections and the served type definitions), the APK's and
  the macOS app's contents (`:apk_test`, `:macos_app_test`) and the
  version's (`:version_sync`). All of them are in `just ci`. After changing
  either contract section, run `scripts/sync-app-prompt.sh` and commit the
  assets with it. `just app-test-native` runs `apps/clax_app/tool/check.sh`
  (`flutter analyze`, `flutter test`, and the prompt check); its
  `*_real_test.dart` files run only when `CLAX_EMBED_LIB` names a host
  build of the core, and skip otherwise.
- `just app-integration` runs the integration tests on macOS, one app per
  file of `integration_test/` (`//apps/clax_app:integration_<name>`): the
  full loop (a scripted agent making and changing an app on the embedded
  core and inspector), app bundles end to end, the scheme's origins
  (`scheme_spike_test.dart`), and the Mac's window chrome (`mac_chrome_test.dart`: the toolbar, menus,
  Settings and Inbox windows, alert and sheets captured in light and dark,
  its menu text against `integration_test/goldens/main_menu.txt`, and
  clicks, drags and keys sent as AppKit events). Each opens the app's
  window and takes up to a few minutes, so they are not in `just ci`; run
  it in the background. The chrome test writes its PNGs to
  `$CLAX_MAC_SHOTS_DIR` (`--test_env=CLAX_MAC_SHOTS_DIR=$PWD/target/flutter-app/mac`)
  or the test's outputs. It needs no Screen Recording permission: it reads
  only Clax's own windows. On an unlocked, awake display with Clax able to
  come to the front, its captures come from the screen and its keys and
  clicks go through AppKit's event queue, and every check runs; on a locked
  or sleeping display (an agent's session) the windows are drawn from their
  layers instead (no materials or Liquid Glass, menus blank), events go to
  the views directly, and the key checks are skipped, as its log says. `just app-integration-native`
  runs `flutter test <file> -d macos` for each file, into
  `apps/clax_app/build/macos`.
- `just app-integration --ios` runs the simulator ones instead, the files
  `integration_test/ios_*_test.dart` (`//apps/clax_app:integration_ios_<name>`):
  a cold start with the core in the app's process, a cycle through the
  background, a make paused when the background time is spent, and a
  `.clax` opened in the app. Each installs its own debug app fresh on "Clax
  iPhone" (`just app-ios` creates it), booting it when it is not running
  and shutting it down after; `apps/clax_app/tool/ios-integration-run.sh`
  takes the screenshots a test asks for into the test's outputs
  (`bazel-testlogs/apps/clax_app/integration_ios_<name>/test.outputs/`).
  Bazel only.
- `just app-eval [--model ID] [--prompts a,b] [--max-spend USD] [--run
  NAME]` is the live build eval, run by hand and never by a gate: each
  prompt of `apps/clax_app/tool/eval/prompts.json` is built by the app's
  own agent on the Messages API, on a scratch home of the embedded core, to the end of
  its build. It needs `ANTHROPIC_API_KEY` and spends real money: up to $6 a
  build, refused before anything runs when that times the prompts is over
  `--max-spend` ($30 by default). Each run's files, screenshots, plan,
  per-run costs and a results table go to `target/flutter-app/eval/<run>/`.
  It builds the macOS debug app, so do not run it beside another macOS
  build or `just app-integration`.
- `just app-apk` builds the release APK, for arm64-v8a and x86_64 with the
  embedded core (`libclax_embed.so`) of each, and prints its path (`//apps/clax_app:apk`,
  `bazel-bin/apps/clax_app/apk.apk`). It is signed with the debug key,
  which is enough to install it on your own phone.
  `just app-apk-native` builds the core for Android with Cargo into
  `jniLibs/` and the APK with Gradle, at
  `apps/clax_app/build/app/outputs/flutter-apk/app-release.apk`.
- `just app-phone` builds HEAD's APK and installs it on the plugged-in
  phone (`scripts/app-phone.sh`, a dry run without
  `--i-know-what-i-am-doing`), with the SDK's `adb` Bazel fetches; a
  checkout with changes is built from a clean copy of HEAD. It then opens
  the app and waits, up to `APP_PHONE_READY_WAIT` seconds (300; a first
  launch after an upgrade may migrate and back up the store first), for
  the core's `clax-core: ready pid=<PID>` line in `adb logcat --pid` (a
  release build is not debuggable, so its `core.json` cannot be read with
  `run-as`), and checks the app keeps that PID.
- `just app-macos` builds the release (sandboxed) app, `Clax.app`, and
  prints its path (`//apps/clax_app:macos_app`, a zip);
  `just app-macos-native` builds it with Xcode, at
  `apps/clax_app/build/macos/Build/Products/Release/Clax.app`.
- `scripts/build-app-clax.sh [android|macos|all] [--debug]` builds the
  embedded core natively, with Cargo, and places it where the native app
  builds pick it up: `apps/clax_app/build-native/macos/libclax_embed.dylib`
  (universal, arm64 and x86_64, joined with `lipo`, as Xcode builds a
  Release app; with `--debug` the host CPU only), and stripped
  `apps/clax_app/android/app/src/main/jniLibs/{arm64-v8a,x86_64}/libclax_embed.so`
  (all git-ignored); the `-native` recipes run it first. With Bazel the
  APK and the macOS app build it themselves
  (`//crates/clax-embed:clax_embed_android_arm64`,
  `:clax_embed_android_x86_64`, `:clax_embed_macos`). A release build (the
  default) embeds `web/dist`, so it builds the web UI first. Each library
  is renamed into place, never copied over one that may be in use.

No test and no gate calls a model: the agent is tested against a scripted
backend and a fake Messages API server. Runs against a live model are
manual. A debug build given `--dart-define=CLAX_SCRIPTED_AGENT=<file>`
answers with the scripted results in that file (a JSON array of edit
results, or `{"error": "<message>"}` items) instead of a model, which drives
the whole app without one.

**The kit.** The agent's `add_kit` copies `apps/clax_app/assets/kit/`
(see its `README.md`) into an app; `kitApi` in
`lib/agent/tools/kit_tool.dart` describes it to the model, in the tool's
result and in the system prompt. The map part (`map.js`) loads its tiles
through `net`; `web/e2e/map.spec.ts` drives it in the app's view against
tiles the test answers at the shell's `net:fetch` request, the consent
dialog included.

**The home app.** The home screen is an app of its own, built from
`apps/clax_app/home/` (`index.html`, `home.js`, `home.css`, its tests, and
`manifest.json` naming its title, capabilities, lineage, release key,
release series and the kit files it carries) into the bundle the app
ships, `assets/home/home.clax`, with its release record
`assets/home/release.json`. On the first run the app installs it and
records its ID in `home.json`; at each start it updates the installed home
app from a newer shipped release (silently, unless the person changed it:
then they choose Update or Keep mine, once per release). The `home`
capability it uses is "The home screen's capability" in
`docs/contract.md`; `assets/kit/fake-home.js` stands in for it in its
tests (`add_kit` part `home`).

The bundle is a build output: every build of the app packs it from the
sources (`//apps/clax_app:home_bundle`, an asset of the APK and the macOS
app; natively `scripts/build-home-bundle.sh`, which `app-apk`, `app-macos`
and `app-test` run first), so `just app-phone`, `just app-apk` and `just
app-macos` ship the current `home/` with nothing to pack or commit. Its
version is chosen by the build (`apps/clax_app/tool/home-version.sh`):
the same while `home/` and `assets/kit/` are unchanged, and higher than
any earlier build's on this machine when they changed, so a phone given a
new build updates its home app. The release is `<release>.<version>`, the
manifest's `release` naming the series; `version` there is the floor the
built versions start above.

The bundle is signed by the home app's own key, and every release must be
signed by the same one: an install treats a bundle under another key as a
different app. Its seed is kept outside the repository, owner-only, at
`~/.config/clax/home-bundle/key` on the owner's Mac (`CLAX_HOME_BUNDLE_KEY`
names another file), beside `releases`, the ledger of the versions given;
keep that directory backed up. The manifest's `key` is the key's public
half, and a build with another key stops. A machine without the key (CI,
or anyone else's) signs with the development key, `tool/home-dev.key`, on
the development lineage: its app installs that home app as a different
one, which a release never updates.

```
just home-bundle                 # builds the bundle alone and prints its path
./scripts/build-home-bundle.sh   # natively, a dry run: says the version it would give
```

`test/home/home_bundle_test.dart` checks that the built bundle verifies,
matches its release record and holds today's `home/` and kit files.

**On an Android phone.** Turn on developer options and USB debugging, plug
the phone in, and from the checkout run:

```
just app-phone   # builds HEAD's APK, installs it keeping the app's data, and checks its core opens and the app stays up
```

or by hand, with an `adb` on `PATH`:

```
adb devices    # lists the phone; accept the prompt on it if it says unauthorized
adb install -r "$(just app-apk | tail -n 1)"
```

`flutter run --release -d <device>` from `apps/clax_app` builds, installs
and starts it in one step, once `scripts/build-app-clax.sh android` has
placed the core. The app needs Android 10 (API 29) or later on an arm64 or x86_64
device. Open it, tap Settings on the launcher (a banner says so while no
key is set), and paste a Claude API key into "Claude API key"; it is kept in
the phone's Keystore-backed secure storage and sent only to the Claude API.
Pick a model there too (Sonnet 5.5 by default; Haiku 5.5 is faster,
Opus 5.5 stronger). Then New: say or type what
you want. The app's core, its documents and every version stay on the
phone; only the model call leaves it. The core listens on no port, so
other apps on the phone have nothing to reach (see "Security model" in
`docs/contract.md`).

**On a Mac, with Claude Code instead of a key.** A sandboxed app cannot
start `claude`, so the Claude Code backend is offered only in a debug
build, which runs without the App Sandbox:

```
just app-macos-dev   # builds the embedded core and the home bundle, then flutter run -d macos
```

`just app-macos-dev` keeps you in `flutter run`, so `r` hot-reloads and `R`
hot-restarts; run it again after changing Swift, the menus or
`apps/clax_app/home`.

In Settings choose "Claude Code on this Mac" (it uses your Claude Code
login; set "Claude Code path" when `claude` is not on `PATH` or in
`~/.local/bin`). Each change runs `claude -p` with tools off, at effort low,
with `--safe-mode`, so your CLAUDE.md, memory, skills, plugins, hooks and
MCP servers stay out of it. The release build (`just app-macos`) is
sandboxed and offers only the Claude API with a key, kept in the login
keychain.

**Signing the Mac app, so the keychain stops asking.** By default the
macOS app and its embedded core are signed ad hoc: each build is a new
signature, the keychain item trusts only the build that saved it, and
macOS asks "clax_app wants to use your confidential information stored in
'flutter_secure_storage_service'" after every rebuild (once a launch: the
app reads the key once a process). With a free Apple Development
certificate (Xcode → Settings → Accounts → add your Apple Account →
Manage Certificates → + Apple Development), sign with it instead:

```
security find-certificate -c "Apple Development" -p | openssl x509 -noout -subject   # OU= is your team ID
sed 's/XXXXXXXXXX/<team ID>/' apps/clax_app/macos/Signing.xcconfig.example > apps/clax_app/macos/Signing.xcconfig
just app-macos
```

`apps/clax_app/macos/Signing.xcconfig` is gitignored; without it builds
stay ad hoc. On the first launch of a signed build the keychain asks once
more, since the saved item still lists only the ad-hoc build: click Always
Allow (or save the key again in Settings). After that, rebuilds keep the
same signature and do not ask.

**Using the app.**

- **Home.** Your apps in a grid, each with the icon its agent designed;
  the inbox button with its unread count; Settings; and New. A long press
  on a tile offers Open, Inspect and, while its plan has features left,
  Resume build. The home screen is itself an app: comment on it (Settings
  › Home screen › Open) to change it, as any app, for example "sort my
  apps alphabetically"; it never lists itself. When it cannot show (it
  fails to load, or has not shown within 10 seconds of a load of its
  page: the first at a start, and the reload after each Undo or Reset),
  Clax's built-in home screen takes its place with "Your home screen
  couldn't load", **Undo last change** and **Reset** (Clax's own home
  screen again, as a new version, your data and threads kept). Undo
  makes the version before the latest current again, as a new version;
  each Undo in a row steps back one more (from versions 1, 2 and 3, Undo
  restores 2, and Undo again 1), even across a restart, until there is
  no earlier version. Any other change (a new version from a comment, a
  Reset or an update) starts it again from the latest. Undo restores the
  files only: the home app's capabilities stay as they are. Settings ›
  Home screen has Open, Undo last change, Reset to Clax's home screen,
  and Use the built-in home screen.
- **Making an app.** New, then say or type what you want. The build view
  shows the plan as a live checklist, what the agent is doing, and a live
  preview of the page as it is written and rendered. Add, edit, drop or
  reorder features while it builds. Open app is offered as soon as the
  first version lands; the agent goes on building the rest of the plan,
  a version at a time, until it is done, the build has spent $6, or you
  Stop it (Resume carries on). Back to apps (Android's back, or Esc and
  ⌘[ on macOS) leaves a make running: up to three run at once, each a
  tile with its progress until it lands, and a tap shows its build view
  again. While a make or a change runs with Clax in the background,
  Android keeps a progress notification up, then posts "<title> is ready"
  (or that it changed, or couldn't); its tap opens the app.
- **When Claude can't be reached.** Comments and new apps are kept on the
  device at once, whether or not the Claude API can be reached. A make, a
  change, Keep going or Retry that cannot reach it waits instead of
  failing: the make's tile reads "Can't reach Claude" and its build view
  "Waiting for Claude", and a change says "Waiting for Claude" on the
  Clax pill over the app (in a pane, its top app bar) and on its thread,
  which offers Try now and Cancel (Cancel stops that comment alone; the
  thread then reads Cancelled, with Send again). While something waits and
  Clax is in front, it probes the API, every few seconds at first and less
  often the longer it stays down, and at once when Clax comes back to the
  front or you tap Try now; the work goes on by itself when it answers,
  also after Clax restarts. A run that loses the connection part way
  pauses and resumes from the turn it was on, without repeating what it
  already did; waiting does not count against its time limit. A turn
  whose connection breaks more than five times waits for Try now or for
  Clax to come back to the front, and each broken attempt that reached the
  API counts toward the run's turn and cost limits.
- **Commenting.** Nothing of Clax's is drawn over an open app. On Android,
  a silent notification named after the app stays up while it is open,
  with **Comment** and **Threads**; on macOS, View › Comment (⇧⌘K) and View ›
  Show Threads (⌘K). A two-finger swipe left opens the threads too, and when
  notifications are off, or you swiped the notice away, a thin handle on
  the right edge does. Comment mode frames the page and puts a bar under it: "Tap anything to comment
  on it", **Comment on the whole app** (a comment on no element), Threads
  (with the open count), Voice (whether a pick starts dictation) and ✕.
  Tap what you want changed to comment there; hold, then drag, to select
  text or draw an area. Clax's composer docks at the bottom (over the
  page, which keeps its size, and above the keyboard) and shows what the
  comment is on: the screenshot of what you picked beside its words, the
  selected text, or a "Whole app" chip. Dictate or type, then Send (or
  Cancel, or Add to plan while the app builds); the threads show each
  comment's screenshot too. The page stays
  usable while the change runs: the Working chip narrates it and folds
  into a small pill at the top right, just under the status bar and any
  camera cutout, and a tap opens the change in progress (its
  comments, steps and latest render, Stop, and Open thread).
- **New versions.** A new version of the open app (a change, a build's
  next feature, an update) never replaces it by itself: a toast "New
  version ready" offers **Reload**, which swaps it in place on the page
  and route you are on, with your data and scroll kept. While you are
  writing a comment the toast waits until you send or cancel it; if it
  fades, it comes back the next time you return to the app. A new app you
  just made opens on its first version directly. A toast says when Clax replied, and
  View opens the thread at its reply; a thread's page takes replies, shows
  Retry on a failed change and opens the version that addressed it.
- **Big changes.** A change has one run of 10 steps, 3 checks and 2
  minutes. A request too big for that (a new screen, several features)
  becomes a short build plan instead: the reply says so and the Working
  chip shows the build, as for a new app. A change that still runs out
  says which limit it hit ("too big for one go — it needed more than 10
  steps"), how often it stopped and what it cost, the files its draft
  changed so far and the agent's latest notes, and offers **Keep going**:
  it goes on from the stopped run's unfinished draft, notes and answers,
  kept until the app changes under that draft. When the draft cannot be
  used, Keep going starts again from the current app and its reply says
  so. After a second stop, Keep going plans the rest as a build instead.
  A change and its Keep goings may cost $6 together. A build
  that spent its $6 waits; Resume on it starts a new $6.
- **Questions.** When the agent asks you something while its app is on
  screen, the question docks in a sheet at the bottom, with a light tap
  (back or Esc closes it before anything else). Over another app a toast
  says so, and View shows it; on the home screen, the build view or the
  inbox it opens in a sheet; with Clax in the background a notification
  "<title> has a question" shows it when tapped. A second question waits
  for the first to close.
- **Inbox.** Replies, new versions, new apps, questions and finished work
  across your apps, newest first and live: Unread or All, filters by kind,
  search, Mark all read; a tap opens the thread, version, app or
  question.
- **Versions.** The threads panel's Versions tab lists every version:
  tap one to preview it, Restore (or Undo) to make an earlier one's files
  current again; the app's capabilities stay as they are. A version's
  page shows the threads it addressed and the files it changed, with a
  line diff against the version before, or against the current one with
  Compare with current.
- **Inspector.** Inspect on a tile's long press shows what an app is made
  of: Overview (title, ID, versions, size, capabilities, history), Files
  (read-only), Data (collections and documents as JSON), Permissions
  (reads of other apps, the agent's remembered read consents, device
  capabilities, network hosts and AI use, each revocable) and Plan.
- **Location.** The first time an app asks where you are, Clax offers
  Precise, Approximate (only your area, about 2 km across) or Don't allow,
  and keeps the answer until you revoke it in the inspector's Permissions.
  An app may follow your location only while it is on screen: a small
  location pill shows at the top left meanwhile, a tap on it stops it, and
  leaving the app stops it for good until the app asks again from a tap.
  On the Mac the window's toolbar also shows Location while an app follows
  you, and a click on it stops it the same way. When Android or macOS will
  not ask again, or the device's location is off, a toast offers Open
  settings.
- **Camera.** An app may take a photo when you let it: on the Mac Clax
  shows a sheet with the camera's live picture, Cancel and Take Photo.
- **Sharing an app.** Share app (Export… on the Mac) on a tile's long
  press or on the inspector's Overview makes the app a `.clax` file and
  hands it to the share sheet (on the Mac, a save panel). The sheet says
  which version goes and how many files; **Include my data** (off by
  default, and unavailable past 2,000 documents or 8 MiB) adds the app's
  documents, so the other person starts with them. Never in the file: your
  API key, your other apps or their data, comment threads, earlier
  versions, or the permissions you granted. The first share of an app
  makes your sharing key and calls it v1; sharing again after a change
  makes the next version, and sharing an unchanged app again sends the
  same version. Settings › Sharing holds **Your name**, shown to the
  people you share with, and, once you have shared, your sharing key's
  fingerprint (`7KQ2-M9XD-4FTA-QW3B`).
- **Installing one.** Open a `.clax` file in Clax: from Files or a
  message on Android (Open with, or Share to Clax), by double-clicking it
  in Finder on the Mac, or with Import app… in the launcher's overflow
  menu on either. The install sheet shows the app, who it is from (their
  name as they gave it, which anyone can claim, beside their key's
  fingerprint: check the fingerprint with them another way when it
  matters), what it can ask for (each capability in words, the internet
  hosts by name) and whether it comes with documents. Install, and the
  app opens. It needs no API key to install or use; changing it, or an
  app that uses Claude, does. A file that was damaged or changed after it
  was signed, or that is not a Clax app, is refused with one sentence.
- **Updates.** An update is a later file of the same app from the same
  sender; there are no automatic checks. Opening it offers Update (with
  what the new version also asks for), and the update is checked before it
  lands: Clax runs it against a copy of your data (its data upgrade, its
  page and its tests), and only when all pass does it become the app's
  next version, with your documents kept and the earlier versions still
  in Versions. When the check fails, "Your app is unchanged", and Details
  says why. While an update runs, the app shows "Updating…"; if its data
  upgrade then fails on your documents, the app goes back to the version
  before (files, title, icon and permissions) with your documents as they
  were, your own private ones included. An update cut short by Clax quitting is put back at the next
  start, or, when the app changed since, Clax asks whether to Restore or
  Discard. If you changed the app yourself, choose **Update (replaces
  your changes)** or **Keep mine (stop updates for this app)** (for the
  home app, **Keep mine (skip this update)**: the next release asks
  again). A file of
  an older or the same version is not installed; the same app signed by
  someone else's key, or the original of an app you made your own,
  installs as a separate app beside yours. Sharing an app you installed
  sends the sender's file as you received it; if you changed it, you may
  instead share **your version as a new app**, which you then update
  yourself.
- **Back.** In an app, back (Android's back gesture or button, Esc or ⌘[
  on macOS) first closes what is over the page (the questions sheet, the
  composer, the threads panel, the comment bar), then goes back through
  the app's own pages and routes, and only from where the app started
  returns to your apps. Elsewhere it goes back a screen. On macOS Esc in a
  text field only leaves the field, and while an app's page has the
  keyboard only ⌘[ goes back.

The app keeps its own records in its application support directory
(inside `~/Library/Containers/dev.empathic.clax` for the sandboxed macOS
build): `settings.json`, `timings.jsonl` (say → usable, long-press →
landed and home-usable times, also listed in Settings),
`agent-timings.jsonl`,
`lineages.json` (which apps were shared or installed as files, and under
which key), `home.json` (the home app's ID, whether the built-in home
screen is chosen, the shipped release the person kept their own home
app over, and where Undo last change has stepped back to),
`bundles/<ID>.clax` (each installed app's file as received, for sharing
it on), `snapshots/` (an app's documents while an update
upgrades them, put back if it fails or Clax ends part way), and
`clax-home/`, the core's home. The sharing key is in the platform's
secure storage beside the API key; uninstalling Clax loses it, and your
later files then install on others' devices as separate apps. It never touches `~/.clax` or port 7480.

A cold start shows the Clax mark while the core opens, then the home
app's page under its loading cover. An app's page, the home app's
included, is covered while it loads: the plain surface for 100 ms, then
the working Clax mark at the centre, which once shown stays for at least
one full breath (`AppCover` and `CoverTiming` in
`apps/clax_app/lib/ui/app_cover.dart`); under reduced motion the still
mark is not held. A new app handed over from its build view shows the
same cover.

**Measuring the home screen's start.** `home-usable` runs from the app's
launch (its `main`) to the home app's first `shown()`. Each launch adds
one line to `timings.jsonl`. On the phone:

```sh
just app-phone        # build and install the release APK
adb shell am force-stop dev.empathic.clax
adb shell am start -W -n dev.empathic.clax/dev.empathic.clax_app.MainActivity
```

Each `am start` after a `force-stop` is a cold start; Settings lists the
last ten timings ("Launch → home usable").

On macOS, `CLAX_APP_DIR=<dir inside the app's container>` in the
environment of the app's executable (`Clax.app/Contents/MacOS/Clax`)
gives it a scratch support and cache directory.

`scripts/perf-app-launch.sh` does those cold starts and reports the
median and p90 of `home-usable`; it gates nothing, and without
`--i-know-what-i-am-doing` it prints its steps and changes nothing:

```sh
scripts/perf-app-launch.sh --macos --i-know-what-i-am-doing    # builds the macOS app
scripts/perf-app-launch.sh --macos --app <Clax.app or its zip> --commit <sha> --runs 5 --i-know-what-i-am-doing
scripts/perf-app-launch.sh --android --commit <sha> --i-know-what-i-am-doing  # the app installed on the phone
```

On macOS it never runs the build as `dev.empathic.clax`: it makes a copy
of it once per build under `target/perf/clax-perf-<sha>`, sets the copy's
`CFBundleIdentifier` to `dev.empathic.clax.perf` and signs it again ad
hoc with its entitlements kept, so the copy runs in a sandbox container of
its own, `~/Library/Containers/dev.empathic.clax.perf`, which the script
removes before the first launch and at the end. Your installed app's
container is never read or written. The login keychain is not kept per
container, though, so each launch runs with `CLAX_APP_NO_KEY=1`: the app
then reads no saved API key and saves none (no keychain access at all, as
if none were saved), and its core opens with `sample()` off. Without
it the copy would ask for the key item your installed app keeps, and
macOS would ask you whether to let it, before the core opened. A build
from before the variable was honoured still asks. On Android the
installed app is measured, on its own home and with its own key (the
variable does not reach an Android app, and a Keystore read asks
nothing), and its timings are read from its log: the app says each
`timings.jsonl` line in logcat after `clax-timing: `, which the script
reads with `adb logcat -d --pid=<the launch's PID>`. A release build is not
debuggable, so `run-as` cannot read its files.

One launch more than `--runs` (default 9) is made and the first dropped.
The run stops before the first launch
while the Mac's screen is locked, and after a dropped launch that never
reaches its home screen. The JSON names the build measured (`--commit`,
the app, its digest and version). `just perf` runs the macOS measure
after the other benchmarks; with `CLAX_PERF_JSON` set the figures and
the missing launches go to the perf log (`scripts/perf-log.sh`), the p90
beside its budget in `web/perf/app-budget.json`
(`<platform>.app_home_usable_ms`), which
`--record` writes for a platform that has none, the p90 rounded up to
50 ms, and refuses to write from a run with any launch missing.

Taking the launch baseline, the daemon's numbers before the app runs on
the embedded core. An agent never makes a git worktree; you may.

1. Name the build. It must include the commit that made the app honour
   `CLAX_APP_NO_KEY` (else its launches wait on the keychain question
   above), and come from before the app was put on the core: the branch
   head while "Run the app on the embedded core" has not landed, else the
   commit before it. The last command checks the first condition.

   ```sh
   C=$(git rev-parse HEAD)
   # once "Run the app on the embedded core" has landed:
   C=$(git rev-parse "$(git log -1 --format=%H --grep='^Run the app on the embedded core')^")
   K=$(git log --reverse --format=%H -S CLAX_APP_NO_KEY -- apps/clax_app/lib/platform/secure_key.dart | head -1)
   git merge-base --is-ancestor "$K" "$C" && echo "$C honours CLAX_APP_NO_KEY"
   ```

2. Build its macOS app in a checkout of `$C` that you make, and keep its
   zip here (`just app-macos` prints the zip's path last):

   ```sh
   git worktree add ../clax-baseline "$C"
   (cd ../clax-baseline && just app-macos)
   mkdir -p target/perf && cp <the zip it printed> "target/perf/clax-macos-$C.zip"
   git worktree remove ../clax-baseline
   ```

3. With the screen unlocked, from this checkout (each launch reads no
   saved key, so nothing asks for the keychain):

   ```sh
   CLAX_PERF_JSON=target/perf/app-launch-before.json scripts/perf-app-launch.sh --macos \
       --app "target/perf/clax-macos-$C.zip" --commit "$C" --runs 9 --record --i-know-what-i-am-doing
   ```

4. Log it under the measured build:

   ```sh
   PERF_COMMIT="$C" scripts/perf-log.sh record --append target/perf-log.jsonl target/perf/app-launch-before.json
   ```

5. On the phone, on the build installed now (not `just app-phone`: a
   build of this branch is not installed on your devices before their
   homes are checked), unlocked and awake, with `P` the commit that build
   was made from. The script reads the launch timings from the app's log
   (`clax-timing:` lines), which only builds that include "Log the core's
   readiness and the launch timings to the platform log" write; a release
   build from before it writes none, and its files cannot be read with
   `run-as`, so on such a build every launch counts as missing:

   ```sh
   adb shell dumpsys package dev.empathic.clax | grep -m1 versionName
   CLAX_PERF_JSON=target/perf/app-launch-before-phone.json scripts/perf-app-launch.sh --android \
       --commit "$P" --record --i-know-what-i-am-doing
   PERF_COMMIT="$P" scripts/perf-log.sh record --append target/perf-log.jsonl target/perf/app-launch-before-phone.json
   ```

6. Commit `web/perf/app-budget.json`, with both runs' medians and p90s in
   the message.

### Clax on iPhone and iPad

On iPhone and iPad, as on Android and the Mac, the core runs inside the
app's own process, and iOS lets an app start no other program, so everything in Clax that
runs one is left out there. The core's code keeps each of those paths off
on iOS when it is compiled (`//scripts:ios_spawns` checks that no new one
appears unlisted).

What runs on iOS: the app's own agent over the Messages API with your
key, every artifact, version, thread and question route, `sample()`,
rooms, and app bundles (`.clax` files).

What does not:

- local agents (Claude Code, Codex, Pi) and their hooks: there is
  nothing to run them;
- Codex tier-5 push;
- "Claude Code on this Mac" as the app's agent;
- the `clax` CLI and plugin installs;
- parent-PID credential files;
- the repository context of sessions (no git runs, so a session's
  capture is `unavailable`);
- MCP over HTTP (`/mcp`): the core in the app listens on no port.

**On the iOS simulator.** With Xcode and its iOS simulator runtime
installed (`xcodebuild -downloadPlatform iOS`, about 8.5 GB), from the
checkout run:

```
just app-ios                      # "Clax iPhone"
APP_IOS_DEVICE=ipad just app-ios  # "Clax iPad"
scripts/app-ios.sh                # a dry run: prints each step's command
```

It builds the app for the simulator with Bazel (`//apps/clax_app:ios_app`
in debug, the only build the simulator runs; uncommitted changes
included), creates the simulator from the newest iOS runtime when it is
missing, boots it, installs the app keeping its data, launches it, and
waits for its core: a `core.json` in the app's data container
(`Library/Application Support`) naming the launched PID with the state
`ready` (one naming it `failed` ends the wait). It ends "Clax is up on
Clax iPhone" and leaves the simulator running; `scripts/app-ios.sh
--udid` prints its UDID (for `xcrun simctl io <udid> screenshot`). It
touches no other simulator.

#### Signing for a device and TestFlight

The simulator app is unsigned and needs none of this. A build for a
device (`//apps/clax_app:ipa_dev`) or for TestFlight
(`//apps/clax_app:ipa_appstore`) is signed with your Apple Developer
team's files, which stay on your Mac in one directory,
`~/.config/clax/apple/` (`CLAX_APPLE_DIR` names another). Bazel reads it
through `@clax_apple_signing` (`bazel/apple/signing.bzl`); the builds that
use it run on this Mac and are never cached, locally or remotely, so no
profile, key or signed app leaves it. Never commit these files.

Without them, the simulator builds as before, and a device or TestFlight
build stops before it starts, naming each file it lacks and this
directory. A device build needs `team-id` and
`clax-development.mobileprovision`; a TestFlight build needs `team-id`,
`clax-appstore.mobileprovision`, and the API key with its two IDs.

Make the directory owner-only, and keep every file in it so:

```
mkdir -m 700 ~/.config/clax/apple
chmod 600 ~/.config/clax/apple/*
```

A directory or file the group or others can read is refused, with its
path and mode. Bazel sees a change to a file at once, but not a change to
the directory's own mode: after `chmod 700` on it, run
`bazelisk fetch --force --repo=@clax_apple_signing`.

| File | What it is | How to get it |
|---|---|---|
| `team-id` | your team's 10-character ID, on one line | developer.apple.com › Account › Membership details, "Team ID" |
| `clax-development.mobileprovision` | the development provisioning profile: the App ID, the Apple Development certificate and your devices | developer.apple.com › Certificates, Identifiers & Profiles › Profiles › +, "iOS App Development" |
| `clax-appstore.mobileprovision` | the App Store provisioning profile: the App ID and the Apple Distribution certificate | the same page, "App Store Connect" under Distribution |
| `AuthKey_<KEYID>.p8` | the App Store Connect API key the upload uses | App Store Connect › Users and Access › Integrations › App Store Connect API › Team Keys, +, with the Developer role. Apple lets you download it once |
| `api-key-id` | that key's Key ID (the `<KEYID>` of its file name), on one line | the key's row on the same page |
| `api-issuer-id` | your team's Issuer ID, on one line | above the keys on the same page |

Before the profiles:

1. **The certificates.** "Apple Development" and "Apple Distribution"
   live in your login keychain, not in this directory: `codesign` reads
   them there, and rules_apple picks the identity the profile names.
   Create each in Xcode › Settings › Accounts › Manage Certificates, or
   on developer.apple.com › Certificates with a request made by Keychain
   Access › Certificate Assistant.
2. **The App ID.** Under Identifiers, an explicit App ID for the bundle
   ID `dev.empathic.clax` (`IOS_BUNDLE_ID` in `bazel/flutter/ios.bzl`),
   with no extra capabilities: the app's entitlements ask for none but the
   keychain group every app has.
3. **The devices.** Under Devices, each iPhone and iPad by its UDID
   (`xcrun devicectl list devices` shows it for a connected or paired
   one). A device added later needs the development profile made again.
4. **The app record.** For TestFlight, an app in App Store Connect › Apps
   with that bundle ID.

Download each profile and save it under the name above. The entitlements
are made from `team-id`: `<team>.dev.empathic.clax` as the
`application-identifier` and the keychain group, the team, and
`get-task-allow` on the development build only.

To check the directory: `bazelisk test //bazel/apple:signing_test` builds
every input from it and fails naming whatever is missing or refused. To
build: `bazelisk build //apps/clax_app:ipa_dev -c opt
--ios_multi_cpus=arm64` (or `:ipa_appstore`).

**On a device, and to TestFlight.** From the checkout:

```
just app-iphone                   # build, install on the iPhone or iPad, launch
just app-iphone-testflight        # build and upload to TestFlight
scripts/app-iphone.sh             # a dry run: prints each step's command
scripts/app-iphone.sh --testflight
```

Both build HEAD exactly: with uncommitted changes they build a clean copy
of HEAD at `target/app-iphone/tree` (and say so). They stop before
building when a file of this directory is missing, naming each one.

`just app-iphone` builds the development-signed app (`:ipa_dev`, `-c opt
--ios_multi_cpus=arm64`, stripped), then installs it with `xcrun devicectl
device install app` on the connected or paired device, keeping the app's
data, launches it and reads `core.json` from its data container until it
names the launched PID ready. The device need not be connected until the build
is done (it then waits up to 15 minutes, `APP_IPHONE_WAIT` seconds); with
more than one, `APP_IPHONE_DEVICE` names the one to use (its name or
UDID). The device must be in the development profile, trust this Mac, and
have Developer Mode on (Settings › Privacy & Security).

`just app-iphone-testflight` (or `just app-iphone --testflight`) builds
the App Store-signed app (`:ipa_appstore`) with the next build number and
uploads it with `xcrun altool --upload-package` and the API key above,
then prints the build number and the App Store Connect link; the build
shows in TestFlight once App Store Connect has processed it. The build
numbers are this Mac's: `builds` in this directory holds one line per
marketing version (`pubspec.yaml`'s version name), `<version> <last
number>`. Each upload takes the next number and writes it there before
uploading, so no number is given twice, even after a failed upload;
nothing is committed. Keep the file with the rest of the directory: a
Mac without it starts each version at 1 again, which App Store Connect
refuses once that number is uploaded. When `altool` refuses an upload,
Transporter (the app) uploads the same `.ipa`, whose path it prints.

## Releasing

A release is what the plugins download, and what `install.sh` installs.
Only a person cuts one. Its downloads work only while the repository is
public.

Releases come from the `prod` branch, and so does clax.dev: production
deploys from GitHub Actions on every push to `prod`, never from a laptop
(`docs/prod.md`). Every change reaches `prod` as a release, through a pull
request that raises the version. One command, run twice, does all of it:

```
just release 0.6.0                              # a dry run: the checks, and every step with whether it is done
just release 0.6.0 --i-know-what-i-am-doing     # prepare: the release branch and its pull request into prod
# review the pull request; once CI passes, merge it on GitHub with a merge commit (this deploys clax.dev)
git switch prod && git pull --ff-only
just release 0.6.0 --i-know-what-i-am-doing     # cut: tag, release, pin, carry to main, Pi and SDK packages
```

`just release` runs `scripts/release.sh` as it is (no build system in
between), and releases Clax and all four plugins: the Claude Code, Codex
and Grok plugins update through this repository's marketplaces once
`main` carries the new version and its pin, and the Pi package is
published to npm from the pin commit on `prod`, as is the SDK package,
`@empathic/clax-sdk`, at the same version.

Which half a run does follows from `origin/prod`. Until `prod`'s version
is the release's, it **prepares** (steps 2-9), from any branch with a clean
checkout: it switches to `release/<version>`, cut from `origin/prod`
(or the branch as it stands, here or on origin); makes a signed merge of
`origin/main` into it, "Merge origin/main into release/<version>" (`main`
carries every earlier release's version and pin, carried there in step
18, so the version files merge cleanly); runs `scripts/bump-version.sh`,
`just bazel-sync` until it changes nothing, and `just ci`; makes the
signed commit "Release <version>"; pushes the branch; and opens its pull
request into `prod` with `gh pr create`, printing its URL. Then it stops
(step 10 is yours): CI runs on the pull request, including the check that
it raises the version. Merge it with a merge commit (Create a merge
commit, not squash or rebase: the cut finds the release commit in
`prod`'s history to carry it to `main`); the push to `prod` deploys
clax.dev. The bump lives on the release branch, never on `main`, until its
release exists: harnesses update a plugin when its version changes, so
`main` must get the version and the pin to its release in one push.

Once `prod` is at the release's version, a run **cuts** (steps 11-22),
from a `prod` checkout at `origin/prod`'s tip: it signs the tag
`v<version>` on `prod`'s tip and pushes the tag alone; waits for the
Release workflow on it and checks that the release lists `SHA256SUMS` and
was built from the tag commit's tree; runs `scripts/pin-release.sh` and
`just ci`; makes the signed commit "Pin the plugins to Clax <version>"
and pushes `prod` (your bypass of `prod`'s pull request rule; the push
deploys clax.dev again, with no server change). Right after, so that
`main` is red as briefly as possible (`scripts/test-plugins.sh` fails on
`main` while its pin is not the newest tag), it **carries** the release to
`main`: the release commit and the pin commit, cherry-picked (`git
cherry-pick -S -x`) onto `origin/main`'s tip in a temporary worktree under
`target/release/`, as two signed commits, pushed to `main`, which refuses
merge commits. Last it waits for CI's gate (ci.yml's `bazel` job, not the
perf-log job, which is cancelled when pushes pile up) on the pin commit on
`prod`, runs Publish Pi package from that commit with `dry_run` off and
waits for it, then Publish SDK package the same way, and prints what is
left by hand: checking that the deploys
of `prod` succeeded (`gh run list --workflow deploy-prod.yml`), and the
crates.io package, which it does not publish. It deploys nothing itself.

The dry run changes nothing (it fetches `main` and `prod` from origin, and
the release branch when origin has it, which moves only the
remote-tracking refs). It checks first, and so does every run, which
refuses to start unless: the version is `X.Y.Z` and newer than every `v*`
tag here and on origin; preparing, `origin/prod` is older than it, the
checkout is clean (no untracked file), the release branch holds
`origin/prod`'s tip and adds only merges of `origin/main` and the release
commit, and its pull request is not open already (an open one is refused
with its URL: merge it first); cutting, the checkout is on `prod`, not
behind `origin/prod` and ahead of it only by the pin commit, and
`origin/prod` has a "Release <version>" commit `main` lacks (a squashed
pull request is refused). It also needs `gh` signed in (`gh auth status`);
commit signing working (a signed commit written to a scratch object
directory); `just`, Bazelisk or Bazel, `python3`, `curl` and `npm` on
`PATH`; npm reachable (and signed in, with `pnpm`, for a first Pi or SDK
publish); and the `npm` environment set up (below). It signs every commit
and the tag with your signing key, never force-pushes, never deletes a
tag, and checks that each commit and the tag it pushes carries a
signature. Before the first step it lists every step, done or to do, with
an estimate of the time in all and still to run (the review not counted),
from the steps' durations in earlier runs' logs or else typical ones. Each
step then prints one line with the share of the estimate done (steps found
done count in full), and a long step a heartbeat about every 30 seconds
with its time so far, the time left, and Bazel's tests done or the
workflow run's state and jobs done. The steps' own output goes only to
`target/release/<version>.log`; a failed step prints the log's last 50
lines and its path. `--verbose` streams the steps' output to the terminal
as well.

A step that fails stops the run, which names the step and changes nothing
after it. Fix the cause and run the same command again: each step's state
is read from git, GitHub and npm (the version files, the commits by
subject, the release branch here and on origin and its pull request's
state, the tag here and on origin, the release's assets, the workflow
runs, the versions npm has), so the run resumes at the first step not
done. Uncommitted changes are accepted only while the step that made them
waits for its commit (the bump, 4-6, on the release branch; the pin,
14-15, on `prod`). A merge of `origin/main` that conflicts stops step 3
with the files: merge by hand with the same subject (`git merge -S
origin/main -m "Merge origin/main into release/<version>"`, resolve, then
`git commit -S`: git does not carry `-S` past a conflict) and run the
command again. A carry that conflicts (`main` changed a line the bump or
the pin changes) stops step 18 with `prod` pushed and `main` untouched,
naming the files and the commits: cherry-pick them by hand in a worktree
of `origin/main`, keeping their subjects and signing each, push `main`,
and run the command again, which finds both subjects on `main` and goes
on. Before pushing `main`, step 18 also runs `scripts/check-version.sh` on
the cherry-picked tree: a version file `main` gained after step 3 merged
it still names the old version, and the run stops the same way, naming
it; carry by hand, then run `scripts/bump-version.sh <version>` and `just
bazel-sync` there and amend the pin copy (`git commit -S -a --amend
--no-edit`) before the push. A refused push of `main` (it moved meanwhile) is carried onto its new
tip by the next run. A commit amended unsigned is refused with how to sign
it (`git commit --amend -S --no-edit`). A failed workflow run is rerun on
GitHub (`gh run rerun <ID> --failed`), then the command again picks it up;
a failed Publish Pi package or Publish SDK package run is dispatched anew.
Each takes the pin commit as its `sha` input and titles its run after it,
which is how the script finds the run again. A wait gives up after
`CLAX_RELEASE_MAX_POLLS` polls (270) `CLAX_RELEASE_POLL_SECONDS` apart
(20), 90 minutes. The cut is known by its tag and its pin commit on
`prod`, so `main` moving on after the carry does not matter, and a
finished release reports every step done.

What it runs, which is also the procedure by hand when a release needs
recovering outside the script:

```
git switch -c release/0.4.0 origin/prod
git merge -S origin/main -m "Merge origin/main into release/0.4.0"
scripts/bump-version.sh 0.4.0     # every version, both Cargo.lock files and the skills' tool blocks
just bazel-sync                   # bazel/workspace.bzl and MODULE.bazel.lock follow the new version; again until it changes nothing
just ci
git commit -S -am "Release 0.4.0"
git push origin release/0.4.0
gh pr create --base prod --head release/0.4.0 --title "Release 0.4.0"
# once CI passes, merge it with a merge commit; then, on prod at origin/prod's tip:
git tag -s v0.4.0 -m "Clax 0.4.0"
git push origin v0.4.0            # the tag alone
# wait for the Release workflow to publish v0.4.0, then:
scripts/pin-release.sh v0.4.0     # PINNED_VERSION and the four SHA256 values, in every wrapper copy
just ci
git commit -S -am "Pin the plugins to Clax 0.4.0"
git push origin prod
# carry both commits to main, in a worktree of origin/main:
git cherry-pick -S -x <the "Release 0.4.0" commit> <the pin commit>
git push origin HEAD:refs/heads/main
# once CI passes on the pin commit on prod: Actions, Publish Pi package, Run workflow
# (on prod, dry_run off, sha the pin commit), or:
gh workflow run publish-pi.yml --ref prod -f dry_run=false -f sha=<pin commit>
gh workflow run publish-sdk.yml --ref prod -f dry_run=false -f sha=<pin commit>
```

`scripts/test-release-script.sh` (gate "release command") runs the script
in scratch repositories with a bare origin holding `main` and `prod` (which
refuses merge commits on `main`, as GitHub does), fakes of `gh` (the pull
request included), `just` and Bazel, and an SSH signing key of its own. It
covers the dry run (also mid-release), both halves of a release in order
and a rerun of it, the refusal while the pull request is open, and
resuming after: a failed `just ci`; a refused push of the release branch
(from another branch); a refused pull request; a merge of `main` that
conflicts; a failed, never-listed or unfinished Release run; a release of
the version from another build; a refused push of `prod` and an unsigned
pin commit; a refused push of `main`; a carry that conflicts, carried by
hand; versions that disagree after the carry, bumped by hand; a failed
CI run on the pin commit, also once `main` has moved on;
and a failed Publish Pi package or Publish SDK package run. It also checks `bazel-sync` that
never settles, `gh` failing during a wait, `main` moving during the
Release wait, a release of the version from an earlier build of the same
tree or of another tree, the Pi package's and the SDK package's first
versions published from here (fakes of `npm` and `pnpm`), packages on npm
published through their workflows, versions npm has already, and each preflight refusal but a missing Bazel,
`python3`, `curl`, `npm` or `pnpm`: among them a cut from another branch
than `prod` or behind `origin/prod`, a squashed pull request, and a
release branch that `origin/prod` has moved past.

The Pi package, `@empathic/clax-pi` on npm, is published from the pin
commit on `prod`, not the tag: its wrapper must pin the release that
shares its version, which `.github/workflows/publish-pi.yml` checks, with
that the commit (its `sha` input, the tip of the ref it runs on by
default) is on `prod` and CI's `bazel` job passed on it, before it
typechecks, packs and publishes with provenance through npm trusted
publishing. npm lets a package name a trusted publisher only once it
exists, so while `npm view @empathic/clax-pi` finds nothing, `just
release` publishes the first version from your machine instead: its
preflight then needs `npm whoami` to succeed (`npm login`) and `pnpm`, and
the dry run says so. Step 20 checks out the pin commit in a temporary
worktree under `target/release/`, makes the checks `publish-pi.yml` makes
(the wrapper pins the package's version, npm lacks that version, `pnpm
install --frozen-lockfile` and `pnpm run typecheck` pass), runs `npm
publish --access public` on your terminal, so npm's one-time-password
prompt reaches you, waits for npm to list the version, removes the
worktree, and prints the one-time setup: on npmjs.com, the package's
Settings, Trusted Publisher, GitHub Actions, organization `empathic`,
repository `clax`, workflow `publish-pi.yml`, environment `npm`. Later
releases publish through the workflow.

The workflow's job runs in the GitHub environment `npm`, which the
repository's settings limit to `prod`, so npm refuses a token from a run
dispatched on any other ref, whatever that ref's copy of the workflow
checks. The environment is a repository setting, made once by the owner:
Settings, Environments, New environment (or `npm`), name `npm`; Deployment
branches and tags: Selected branches and tags; Add deployment branch or
tag rule, branch `prod` (in place of `main`, which it allowed before
releases came from `prod`). `just release` reads it (`gh api
repos/empathic/clax/environments/npm`) and, until npm has the version,
warns in a dry run and refuses to start a release when it is missing or
allows more than `prod`; it never changes it.
Step 20 is done whenever npm has the version. After that first publish,
step 22 lists the trusted-publisher setup and switching `README.md` and
`plugins/pi/README.md` to lead their Pi install with `pi install
npm:@empathic/clax-pi`; until the package exists they keep their current
install instructions. `scripts/test-plugins.sh` keeps the package
publishable: public, tagged `pi-package` for Pi's gallery, Pi's own
modules as optional `"*"` peers (Pi provides them; installed, they would
add a second Pi of about 200 MB), no `.qual` files, and a `LICENSE`.

The SDK package, `@empathic/clax-sdk` ("The SDK package" above), is
published by step 21 the same way, through
`.github/workflows/publish-sdk.yml`: from the pin commit on `prod` with
CI's `bazel` job passed on it, it builds `web/dist-sdk` (`pnpm run
build:sdk`, at `Cargo.toml`'s version), runs its test (`pnpm run
test:sdk`), refuses a version npm has already, and publishes
`web/dist-sdk` with provenance through npm trusted publishing, in the same
`npm` environment. While `npm view @empathic/clax-sdk` finds nothing, step
21 publishes the first version from your machine instead: in a temporary
worktree of the pin commit under `target/release/`, `pnpm install
--frozen-lockfile`, `pnpm run build:sdk` and `pnpm run test:sdk` in
`web/`, then `npm publish --access public` from `web/dist-sdk` on your
terminal; step 22 then lists its trusted-publisher setup (workflow
`publish-sdk.yml`, environment `npm`). The preflight's `npm` environment
check stands until npm has both packages at the version.

A plugin pins the release its wrapper names (`PINNED_VERSION` and one
`SHA256_*` per target in `scripts/ensure-clax.sh`; `scripts/pin-release.sh`
writes them, from the release's `SHA256SUMS`, into that file and into every
plugin's copy, Pi's included). A release cannot pin itself, since its
checksums exist only once it is built, so the tagged commit pins the
previous release and the pin follows in the next commit. `scripts/test-plugins.sh`
(and so `just ci` and CI, which fetches the tags) fails while the pin is
older than the newest `v*` tag or newer than every tag; with no tag at all,
nothing may be pinned. Harnesses update an installed plugin only when its
version changes, so main gets the new version and its pin in one push,
the carry of the cut (above), after the tag and the pin. The binary a release builds
carries plugins that pin the previous release; `clax init` points them at
that binary through the `bin` setting.

The tag runs `.github/workflows/release.yml`. It checks that the tag matches
every version. A release records the tree it was built from in a
`SOURCE_TREE` asset, which `SHA256SUMS` covers. When the tag's release
exists already, the run builds nothing: it finishes if the release was
built from the tree the tag names now (a tag moved to a re-signed copy of
its commit), and fails otherwise, saying to delete the release and rerun.
For a release made before `SOURCE_TREE`, its tree is that of the newest
successful Release run on the tag created before the release: the run that
uploaded it. `scripts/release-exists-check.sh` holds that rule, and
`scripts/release.sh` uses the same script, so both judge a release by
trees, not commits: a moved tag with the same tree keeps its release, and
a release built from another tree is never pinned. Otherwise the
workflow builds macOS arm64 and x86_64 and Linux x86_64 and arm64
(static musl) binaries on native runners, each with the web UI embedded. It
smoke-tests each binary and packs `clax-<version>-<target>.tar.gz`. It
installs one with `install.sh` from a local copy of the release, and
another through the plugins' wrapper pinned to that copy, and publishes the archives, `install.sh` and `SHA256SUMS`. Running the workflow
by hand (Actions, Release, Run workflow), or a pull request that touches the
release path, does everything except publish, and keeps the result as the
`release-dist` artifact.

### The crates.io package

`cargo install clax` is not published yet; everything up to the upload is
in place. crates.io takes a package holding only its own files, and the
binary needs eleven workspace crates and the built web UI, extension and
plugins, so `scripts/crate-package.py` assembles one package, `clax`, in
`target/crate-package/clax`. The workspace crates the binary depends on
become modules of it under `crates/<name>/src/`, their `crate::` and
`clax_<name>::` paths rewritten to match; the files the sources and
`clax-embedded` include are copied at their repository paths; the
dependencies are the union of the crates' own, with the workspace's
`Cargo.lock`; and `scripts/crate-package/build.rs` stands in for the
crates' build scripts. Its license is `GPL-2.0-only AND AGPL-3.0-only AND
LGPL-2.1-only`: GPL-2.0 Rust code with the AGPL-3.0 web UI embedded, whose
bridge and overlay scripts are built from the LGPL-2.1 `web/bridge` and
`web/overlay`.

```
just crate-package                            # build the web UI, assemble, pack, and check the packed crate (uploads nothing)
cd target/crate-package/clax && cargo publish --allow-dirty   # the upload, by a person, from the release tag
```

`just crate-package` checks the packed crate as `cargo install` would
unpack it, with `cargo check --locked` into `target/` (no binary, so
`target/debug/clax` is untouched), and is not part of `just ci`; run it
before a release. The package records the commit it was
assembled from in `BUILD_COMMIT`, so `clax --version` names it.

## Security model

Clax traffic on the LAN is not encrypted: serve on the LAN (`clax serve --bind 0.0.0.0`) only on a network you trust. A local daemon answers another machine only for a connected person (a signed request of their machine, or their browser's cookie); anyone else is refused with 403 `forbidden_host`.

Writes, and reads of the session list (working directories, process IDs, harness session IDs), need the token in `~/.clax/daemon.json` (mode 0600, served only to a browser holding an owner browser session, which only a one-use login link opens), or a connected person's machine key. A connected person's agents publish, reply, resolve and write page data with that person's rights, as the owner set them. They sign every request with their machine's key, which never leaves that machine, so a request seen on the network cannot be replayed. Connecting machines, approvals, rights and raises change only in the owner's or the person's own browser: no agent, tool or CLI command can change them. What another person wrote reaches an agent only once that agent's person approves its author. An agent of the owner's that has a shell runs as the owner's OS user, and is trusted as the owner. Every `/api` route answers only to a `Host` of `localhost`, `127.0.0.1`, `[::1]`, or the exact IP and port the connection arrived on (403 `forbidden_host` otherwise), which defeats DNS rebinding. The comment routes need no token, but they need a credential (an owner browser session, or a viewer cookie the daemon minted) and refuse requests from another origin, including published pages: a request with no credential is refused before any route. The viewer cookie never leaves the daemon; viewers are named by a public ID, comment threads carry no session IDs, and the daemon's `codex` path is served only with the token. Each artifact's content is served on a site of its own, `<id>.localhost`, never on the daemon's own host: a page runs on that origin alone, and only the owner's shell and the artifact's own pages may frame it. The site answers only a site session (made through the shell, `/_clax/session/start` and a one-use code), so a page reached under any other name, a rebound one among them, reads nothing.


While an agent of yours is running, the decisions that matter made with the daemon token (answering an agent's question, pushing, connecting a public Clax, widening who may open a page, pairing the extension) wait for your click in a browser of yours; `docs/contract.md`, "Browser confirmations", lists them. Any of your browser sessions may confirm. An agent running as you can open a browser session too, so you may also require Touch ID or another passkey to confirm (offered on the first confirmation, and in Settings, Confirmations); each passkey shows when it was added, from which browser, and which agents were running then. A passkey you did not add, or one you can no longer use (a lost authenticator, Clax now served at a LAN address, where passkeys do not work), can lock you out of confirming. To recover, stop Clax (`clax stop`) and run `clax passkeys reset`: it refuses while the daemon for the home runs, then removes every passkey, stops requiring one, and prints what it removed. An agent that runs as you could do the same, as it could stop Clax; the passkey keeps a running Clax's confirmations to your presence.
Runtime capabilities arrive in a later phase.
