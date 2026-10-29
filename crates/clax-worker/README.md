# clax-worker

The Clax server in a Cloudflare Worker: one Durable Object per Clax instance
(`InstanceDo`, `src/instance.rs`), the web UI from Workers Static Assets,
artifact versions in git (`RepoDO`'s stand-in or Cloudflare Artifacts) and
assets in R2. `src/lib.rs` documents how the Worker routes requests and
`src/instance.rs` what the instance does (hosted mode, sockets, seats across
a wake, the alarm).

## Developing

The Worker and its VFS are crates of the root workspace. `just worker-test`
runs all of their checks under Bazel: the Rust tests, the wasm build
(`//crates/clax-worker:bundle`, `../../bazel/wasm/README.md`) and the checks
below on it, `wrangler dev --local` and the browsers included.

By hand, from this directory: `pnpm install`, build the web UI first
(`just web`, or `../../scripts/build-web.sh`: the Worker embeds `web/dist`,
its wasm build refuses to run without it, `build.rs`, and recompiles when it
changes), then

- `pnpm run dev`: `wrangler dev --local` on `.wrangler/state` (runs
  `build.sh`, which builds the Worker into `../../target/clax-worker/build`:
  with Bazel when it is installed, else worker-build);
- `cargo test -p clax-worker -p clax-do-vfs`: the VFS's tests and the
  crate's unit tests, natively;
- `node test-wasm.mjs`: the SQLite-on-Durable-Object spike across a restart;
- `node test-sandbox.mjs`: ClaxBot's sandbox conformance suite against the
  Worker Loader sandbox (below), on the instance's object;
- `node test-bot.mjs`: a ClaxBot run driven by the instance's alarm
  against a scripted Messages API on 127.0.0.1 (below), across a restart;
- `node test-repos.mjs`: the repository hosts (`CLAX_TEST_JJ` names a `jj`
  for the jj clone check);
- `node e2e.mjs`: the instance in two browsers with the owner token;
- `node e2e-hosted.mjs`: the instance as a hosted Clax: setup, an invite, a
  sign-in link, an agent token on `/mcp`, a guest, each artifact's site
  on its own host (`SITE_DOMAIN=localhost`), app data in each artifact's
  object, live pointers, a spectator, and seats across a simulated wake;
- `node smoke-remote.mjs --local`: the deployed instance's smoke (below)
  against a fresh hosted `wrangler dev --local` with `REPO_STORE=memory`.

Each takes `--no-build` to skip `build.sh`.

To try clax.dev itself on this machine (its signed-out pages, setup,
GitHub sign-up and sign-in against real GitHub, invites, the Terms and the
quickstart a new person gets), `just clax-dev-local` (from the repository's
root, `demo/clax-dev-local.ts`) runs this Worker on the first free port of
8788–8797 with a production deploy's vars, nothing seeded, you its owner
and its data kept between runs. It needs no configuration: the first run
on a machine registers a GitHub App from a manifest (a page with one
button, then GitHub's confirmation), whose callbacks name those ports and
whose client ID, secret and owner are kept in
`~/.config/clax/local/github.env` for every worktree. `--fresh` starts
over and `--setup` leaves setup open to try it. `just hosted-demo` is the
other local instance: seeded people and artifacts, and demo agents.

### ClaxBot's sandbox

ClaxBot's scripts run in the Worker Loader sandbox (`src/sandbox/`,
`LoaderSandbox`): each script in a fresh dynamic Worker from the `LOADER`
binding, with no network (`globalOutbound: null`), no bindings, and a
function of the instance's as its only way out, behind the same contract,
prelude and conformance suite as the daemon's QuickJS sandbox
(`crates/clax-bot-sandbox`). The module docs of `src/sandbox/mod.rs` say
how a run goes and what bounds it.

`test-sandbox.mjs` runs the conformance suite under `wrangler dev
--local`. Two cases end otherwise here, by how V8 differs from QuickJS
(`PLATFORM_DIFFERENCES`), and are checked to end as documented; eight need
the CPU or memory limit only Cloudflare puts on a dynamic Worker
(`PLATFORM_ONLY`): workerd enforces neither, so there a script that never
yields holds the whole runtime, and they are listed, not run.

### ClaxBot's runs

The instance drives ClaxBot's runs from its alarm (`src/bot.rs`, whose
module docs have the details): a run started or cancelled sets the alarm
for now; each alarm ticks as many steps of as many runs as fit in
`alarm_budget` (10 minutes of wall time: the platform's 15-minute alarm
limit less a 5-minute margin, never less than the 60 s a model turn is
started with), keeps the alarm a minute ahead while it does (so an
invocation the platform ends is followed by another), and then sets it
for the driver's next wake: at once while runs are queued, after a wait
(a model retry, a stalled sandbox's backoff) when one waits, never when
none is left. A wait is recorded on its run, so a later incarnation does
not tick it early. Each object incarnation has a driver of its own ID,
which starts at its first event and takes back the runs an earlier one
left; the step that was cut is executed again, its model call billed
once. The provider's URL must be https to a public DNS name (no IP
address, localhost or private network name), and changing it clears the
stored instance key; when the sandbox cannot be made, ClaxBot is off and
a send is refused `not_configured`.
Settings come from the database alone (the admin pages; the Worker reads
no `[bot]` table), and each step also charges the Durable Object request
and Toolpath storage lines. Model keys are sealed under the `CLAX_KEK`
secret: without it (or with one that is not 64 hex digits) no key is
stored (503 `no_kek`) and a send to ClaxBot is refused `not_configured`;
the instance serves on. A run's events stream as SSE from the object for
as long as the run lasts, replayed from the committed steps after a
reconnect with `Last-Event-ID`.

`test-bot.mjs` runs it under `wrangler dev --local` (its header lists
every check): refusals without `CLAX_KEK` and of private provider URLs;
a thread sent to ClaxBot whose script reads, edits, publishes and
replies; the alarm kept ahead while it drives; wrangler killed by its
process group while the second turn streams and started again on the
same state, the alarm alone resuming the run, which finishes with nothing
published or replied twice and its tokens equal to what the scripted API
served; the event stream replayed across the restart; a cancel aborting
the model request; and a send refused `credits_exhausted`. Its scripted
provider listens on 127.0.0.1, which the test vars
`CLAX_TEST_ROUTES` and `CLAX_TEST_LOOPBACK_PROVIDER` together allow.

## Deploy to Cloudflare

clax.dev, Empathic's instance, is deployed from GitHub Actions on every
push to `prod`, never from a laptop: [docs/prod.md](../../docs/prod.md) has
the workflow and its GitHub setup. What follows sets up an instance (its
first deploy, which generates its secrets, always runs from a laptop) and
deploys any other.

### In one command

On an account with Workers Paid (and the Artifacts beta, for
`REPO_STORE=artifacts`), with a zone on it for the public hostname and a
zone of its own for the site domain (each artifact is served at
`<artifact ID>.<site domain>`, e.g. `<ID>.clax.run`):

`scripts/cloudflare-setup.sh` does all of it, from any directory, and is
safe to run again: it checks the tools (Node 26, rustup's wasm32 target, an
LLVM that targets wasm32), installs the pnpm packages when their lockfiles
changed, starts `wrangler login` for you to finish in the browser when you
aren't signed in, then runs `deploy.mjs`, which creates only what is missing.
It is a dry run unless you pass `--i-know-what-i-am-doing`:

```sh
scripts/cloudflare-setup.sh --zone example.com --site-domain example.run         # dry run: changes nothing on the account
scripts/cloudflare-setup.sh --smoke --i-know-what-i-am-doing                     # sign in if needed, deploy, smoke it, redeploy without diagnostics
scripts/cloudflare-setup.sh --login                                              # only sign in
```

`--production` marks the instance as the production one: `setup.env` keeps
`PRODUCTION=1`, and from then on every run passes `--production` to
`deploy.mjs`, which deploys only a clean checkout whose `HEAD` is origin's
`prod` tip (`git ls-remote origin refs/heads/prod`, checked again just
before the upload), and only in GitHub Actions. `just clax-dev-deploy`,
which deploys clax.dev alone, always passes `--production`. To deploy
production from a laptop anyway (say, while Actions is down), check out
`origin/prod`'s tip and add `--from-laptop`, which lifts only the Actions
condition:

```sh
git fetch origin prod && git switch --detach origin/prod && git status --short   # clean, at prod's tip
just clax-dev-deploy --from-laptop --smoke --i-know-what-i-am-doing
```

A dry run (the default) never refuses; it says what a deploy would refuse.

The deploy workflow runs the same script headless, twice: a dry run, in a
step without the Cloudflare token, installs the packages and builds; then
the step given the token deploys that build with `--no-build`, which
installs and builds nothing and refuses a Worker not built from the
checked-out commit:

```sh
scripts/cloudflare-setup.sh --legal bundled --public clax.dev --site-domain clax.run --store artifacts \
  --github-client-id … --owner-github … --account … --ci --production                                  # build
scripts/cloudflare-setup.sh … --ci --production --no-build --smoke --i-know-what-i-am-doing             # deploy
```

`--ci` neither reads nor writes `setup.env`, asks nothing (`deploy.mjs`
fails naming each setting it needs that no flag gives: the public host, the
site domain, `--store`, the legal documents, the account, and
`--owner-github` with `--github-client-id`; `--retired-content-origin
<https origin>` keeps an instance's retired content host), signs in with
`CLOUDFLARE_API_TOKEN` and never starts `wrangler login`, and keeps the
deploy record in `$CLAX_DEPLOY_HOME`. It changes no secret but the GitHub
client secret it is given (`GITHUB_CLIENT_SECRET`): it refuses a Worker
without `SETUP_SECRET`, `CLAX_TOKEN` or `CLAX_KEK` (a first deploy), one
whose GitHub sign-in it would turn off or leave without its secret,
`--rotate-secrets` and `--diag`; that it keeps no `CLAX_KEK` backup is a
notice. It prints no first-time setup text, and exits 0 only when the
deploy, the health wait, the site host check and the smoke all pass. Its
smoke runs as the agent token in `CLAX_SMOKE_TOKEN`, with no diagnostics
deploy.

Or step by step, in this directory:

```sh
pnpm exec wrangler login                # once, in this directory (pnpm install first)
just hosted-deploy --zone example.com --site-domain example.run   # or: node deploy.mjs …
node smoke-remote.mjs --deploy ~/.config/clax/deploy/example.com.json
```

`deploy.mjs` (its header lists every option) checks the login, takes the
public hostname (`--public`; with `--zone example.com` it offers
`example.com`) and the site domain (`--site-domain example.run`, else the
record's, else asked on a terminal with no default: a registrable domain
of its own, never the public host, a domain under it, one it is under, one
sharing a parent domain with it, or an IP address; the shared parent is
judged by the last two labels, so under a multi-label public suffix such as
`co.uk` it misses `sites.example.co.uk` beside `app.example.co.uk` and
refuses two unrelated `*.co.uk` domains), creates what is missing (the R2 bucket; with Artifacts the queue and the
namespace, which also proves the account has Artifacts), generates
`SETUP_SECRET` and `CLAX_TOKEN` when the Worker has neither and uploads
them with the first version (printed once, and kept with the origins in
`~/.config/clax/deploy/<public host>.json`, 0600, outside the
repository; the secrets, the setup URL and the next steps are printed only
by the deploy that generated them), writes `target/deploy/wrangler.deploy.jsonc` (or `--out
<path>`: this file's config with `HOSTED=1`, the public origin,
`SITE_DOMAIN`, the store, the public host as a custom domain and the route
`*.<site domain>/*` on the site domain's zone, `workers_dev` and preview
URLs off, Workers Logs on, the diagnostics off unless `--diag`), builds the
web UI and the Worker, deploys, waits for `/healthz` on the public host to
report the build it uploaded (saying whether it waits on DNS, the
certificate, the Worker or the build from before the deploy), checks
`https://<random ID>.<site domain>/_clax/healthz` and, when that fails,
prints the steps [below](#dns-for-the-public-host-and-the-site-domain) with
what it found for each, and prints the setup URL and the next steps. Run again, it changes only what differs; secrets are kept
unless `--rotate-secrets`, which also re-identifies every visitor of a site
open to the world (visitor IDs are keyed by the daemon token) and orphans
their `data/users/<id>` subtrees.

- `--dry-run` does everything but change the account, ending at `wrangler
  deploy --dry-run` with the upload size; without a login it skips each
  account call and names it.
- `--config-only` only writes the config: no wrangler, no login, no build
  (`test-deploy-config.mjs` checks what it writes).
- `--store memory` deploys with the `RepoDO` stand-in instead of Artifacts
  (no `ARTIFACTS` binding, no queue; git over `/git/` on the public host),
  so the demo runs on real Cloudflare when Artifacts misbehaves.
- `--diag` turns the diagnostics routes on (they are off by default), for
  the smoke's probe, git token and reconcile steps; the deploy and the
  smoke both end by saying to deploy again without it, which turns them
  off.

`smoke-remote.mjs` (its header lists the steps and options) runs a short
real-platform smoke with a timing per step: the bindings as the Worker sees
them; with Artifacts, the binding's raw calls on a scratch repository
(`src/diag.rs`: what `get` of a missing repository throws, what reads of a
missing object answer, a token's format, the remote's advertisements under
three ways of presenting the token, what a fork answers while it is
copied), each checked against what `src/repos/js.rs` expects; setup with
the secret while setup is open, `--session` for an existing owner's
browser, an agent token (`CLAX_SMOKE_TOKEN` or `--agent-token`, as the
deploy workflow runs it), else the owner token from the deploy record
(every deploy after the first: the agent-token step, which only a
signed-in browser can take, skips and says so, and the MCP and owner steps
use the owner token; with no credential at all the smoke fails); an agent
token; two
versions published over `/mcp` and read back; a fork; a git token, `git
clone`, the clone's trees against the binding's; `git push` and the push
becoming a version (through the queue's `pushed` event, for which it
subscribes the artifact's repository while it runs; when no event comes, a
direct reconcile tells a missing event from a failing reconcile); and a
comment arriving live on a WebSocket stream. On the artifact's site host
(`--site-domain`, else the record's; `localhost` with `--local`) it checks
a private site refused without a session, `/site-signin`'s handshake
(a `SameSite=Lax` cookie), the shell's one-use code (a `Partitioned`
cookie), the site opened to the world and closed again, and two db
artifacts' documents and cookies kept apart; a Worker that does not serve
site hosts yet (its health there answers 403 `forbidden_host` or 404)
skips them all. When it made the owner it
prints a sign-in link for the owner's browser. The diagnostics routes
(`/_diag/`, `CLAX_DIAG_ROUTES=1` and the owner token) give it the probe,
the git token and the reconcile.

As an agent token the smoke takes every step an agent can: `/mcp`, the
publishes and reads, a fork and a git token through the MCP tools (`fork`,
`git_remote`), the clone and the push becoming a version, a site host's
health and a private site refused. It skips, naming each with why, the
steps only the token's person can take: the diagnostics (the owner
token's), the shell's pages, minting an agent token, a site's sign-ins and
the two db artifacts behind them, opening a site to the world (which waits
on the person's approval) and the live WebSocket stream
(`deploy-lib.mjs`, `SMOKE_CANNOT`). `node smoke-remote.mjs --local
--as-agent` runs it so against a local Worker whose owner minted the
token beforehand.

### What the Worker binds

`wrangler.jsonc` declares all of them; a deploy needs each to exist on the
account.

| Binding | Kind | What it holds |
|---|---|---|
| `CLAX_INSTANCE_DO` | Durable Object (`InstanceDo`, SQLite-backed) | One Clax instance: its database (through the VFS), its sockets, its alarm |
| `REPO` | Durable Object (`RepoDO`, SQLite-backed) | The git stand-in's repositories (`REPO_STORE=memory`) and the repository token ledger in both modes |
| `APP_DATA` | Durable Object (`AppDataDo`, SQLite-backed) | One artifact's app data (the `db` capability's documents, leases and their audit records), one object per artifact named by its ID (`src/app_data.rs`); only the instance's object calls it |
| `CLAX_DB` | Durable Object (`ClaxDb`) | The VFS spike; reached only with `CLAX_TEST_ROUTES=1` |
| `ASSETS` | R2 bucket `clax-assets` | Artifact assets, in both `REPO_STORE` modes |
| `ARTIFACTS` | Cloudflare Artifacts, namespace `clax` | Artifact versions as git repositories, when `REPO_STORE=artifacts` |
| `WEB` | Workers Static Assets, `../../web/dist` | The web UI (`run_worker_first`) |
| `LOADER` | Worker Loader (`worker_loaders`) | ClaxBot's code-mode sandbox: a dynamic Worker per script (`src/sandbox/`) |

`deploy.mjs` creates the bucket (`npx wrangler r2 bucket create
clax-assets`), the queue (`npx wrangler queues create
clax-artifacts-events`) and the Artifacts namespace (made by its first
repository). The Durable Object classes are created by the migrations in
`wrangler.jsonc` on the first deploy.

Artifacts delivers `cf.artifacts.repo.pushed` only to a subscription on one
repository (source `artifacts.repo`, with its namespace and name; the
account-wide `artifacts` source carries only created, deleted, forked and
imported). Nothing subscribes each artifact's repository yet, so a push to
Artifacts becomes a version at the artifact's next publish, which records
any unrecorded commits first. One repository is subscribed with

```sh
npx wrangler queues subscription create clax-artifacts-events --source artifacts.repo \
  --events pushed --source-namespace clax --source-repo-name a-<artifact ID>
```

### Vars

| Var | Deploy value | Meaning |
|---|---|---|
| `HOSTED` | `1` | A hosted Clax: browser sign-in, people's rights, guests, agent tokens |
| `PUBLIC_ORIGIN` | `https://clax.example.com` | Where people reach Clax; every link the server writes names it |
| `SITE_DOMAIN` | `clax.run` (`deploy.mjs --site-domain`) | Each artifact's site is served at `<artifact ID>.<SITE_DOMAIN>`; empty serves sites at `<artifact ID>.localhost` |
| `RETIRED_CONTENT_ORIGIN` | the deploy record's old `content_origin`, if any (`https://content.clax.example.com`) | The content origin of before sites: kept as a custom domain, where an asset's old address (`/_blob/<id>?k=…`) answers 301 to its artifact's site and anything else 404; empty when there was none |
| `REPO_STORE` | `artifacts` (or `memory`) | Where versions are kept |
| `GIT_SMART_HTTP` | `0` with `artifacts` | `1` serves the stand-in over git smart HTTP at `/git/*` |
| `CLAX_INSTANCE` | empty | The instance object's name (`default`) |
| `CLAX_LIVE_PEOPLE_SEATS`, `CLAX_LIVE_GUEST_SEATS`, `CLAX_LIVE_AGENT_SEATS`, `CLAX_LIVE_SEAT_IDLE_S` | empty | Live seats per artifact and the idle release; empty takes 100, 20, 20 and 180 s |
| `CLAX_TEST_ROUTES`, `REPO_DEV_ROUTES` | `0` | Test-only routes; never `1` on a deploy |
| `CLAX_DIAG_ROUTES` | `0`; `1` (`deploy.mjs --diag`) only until the smoke is green | `/_diag/` for the owner token (`src/diag.rs`): what the platform's bindings answer |
| `GITHUB_CLIENT_ID` | the OAuth App's client ID (`deploy.mjs --github-client-id`) | GitHub sign-in; off without it or without `GITHUB_CLIENT_SECRET` |
| `OWNER_GITHUB` | the owner's GitHub login (`deploy.mjs --owner-github`) | Whoever signs in as that account (case-insensitive) is the owner |
| `MEMBER_MAX_ARTIFACTS` | empty | The most live artifacts a GitHub member keeps; empty takes 200 |
| `GITHUB_OAUTH_BASE`, `GITHUB_API_BASE` | empty | Tests only: a fake GitHub's bases |
| `LEGAL` | `bundled` on Empathic's service (`deploy.mjs --legal bundled`), else empty | Serve `docs/legal`'s Terms of Service and Privacy Policy and ask people to agree ([below](#terms-of-service-and-privacy-policy)) |
| `TERMS_URL`, `PRIVACY_URL`, `LEGAL_VERSION` | empty, or a deployment's own (`--terms-url`, `--privacy-url`, `--legal-version`) | Link your own documents instead; all three or none |
| `APPLE_TEAM_ID` | the team that signs the Clax app, on clax.dev | Serve `/.well-known/apple-app-site-association` for `<team>.dev.empathic.clax`, so `/a/<id>/get` opens the iOS and macOS app; empty serves none |
| `ANDROID_PACKAGE`, `ANDROID_SHA256` | `dev.empathic.clax` and the signing certificate's SHA-256 fingerprint, on clax.dev | Serve `/.well-known/assetlinks.json`, so `/a/<id>/get` opens the Android app; both or neither |
| `IOS_STORE_URL`, `ANDROID_STORE_URL` | the app's store pages, once listed | Offered on `/a/<id>/get`; empty offers none |

Set them in `wrangler.jsonc`'s `vars` or with `--var` on `wrangler deploy`.
A hosted instance whose `PUBLIC_ORIGIN` is not an `https://` origin (or
`http://` on localhost), or whose `SITE_DOMAIN` is not a bare domain name
or is `PUBLIC_ORIGIN`'s host, a domain above it or one under it (the rule
of the daemon's `[hosted] site_domain`; `localhost` under a `localhost`
public origin is allowed), does not start: every request answers 500
naming the var. On a site host the Worker serves that artifact's site and
nothing else: its own routes (`/git/`, `/_diag/`, `/_dev/repos/`, `/do/`)
and the instance's API are the site's paths there (`src/lib.rs`).

### Secrets

```sh
npx wrangler secret put SETUP_SECRET   # what /setup takes, once, to make the owner
npx wrangler secret put CLAX_TOKEN     # the owner's bearer token for the CLI (optional)
npx wrangler secret put CLAX_KEK       # ClaxBot's key-encryption key: 64 hex digits (openssl rand -hex 32)
```

`deploy.mjs` generates and uploads both on the first deploy, and
`CLAX_KEK`, ClaxBot's key-encryption key (64 hex digits), when the Worker
has none: its backup when this machine keeps one, else a new one, written
to the backup first, `~/.config/clax/deploy/<public host>.kek` (0600).
Keep that file: the record may be deleted once set up, but without the
backup and the Worker's secret every stored model key is lost. A deploy
warns when the Worker has `CLAX_KEK` and this machine keeps no backup.
`CLAX_KEK` is read from the Worker's secrets alone (a plain var of the
name is ignored). It is never rotated, `--rotate-secrets`
included: a new one opens none of the model keys the old one sealed (to
replace it, clear the stored keys first and set them again after). Without
`SETUP_SECRET`, `/setup` stays closed. Without `CLAX_TOKEN` the
instance makes a token and keeps it in its storage, where nothing hands it
out: set one if you mean to use the CLI against the instance. Use a long
random value for both; a `CLAX_TOKEN` under 32 characters is refused and
the instance does not start.

### Sign in with GitHub

Anyone with a GitHub account can sign up: their first sign-in makes a
person of their own, who may create artifacts and do everything at the
ones they made (publish, comment, their db, agent tokens and MCP), and
sees anyone else's only where a link or the owner shares it. The account
named as the owner's signs in as the owner (on an instance whose owner was
made by setup, the first such sign-in links that GitHub account to it and
renames the owner after it; from then on no other account, whatever its
login, is the owner). Sign-ups are rate limited per client address, a
member keeps at most `MEMBER_MAX_ARTIFACTS` artifacts, and revoking a
person on People blocks their GitHub account from signing in or up again.

1. On GitHub: Settings → Developer settings → OAuth Apps → New OAuth App.
   Homepage URL `https://clax.example.com`; Authorization callback URL
   `https://clax.example.com/auth/github/callback`. Register it, note the
   Client ID, and generate a client secret.
2. Deploy with both (the client ID and owner are kept in the deploy record,
   so later deploys keep them; the secret is uploaded as the Worker secret
   `GITHUB_CLIENT_SECRET`, from the environment or asked on the terminal):

   ```sh
   GITHUB_CLIENT_SECRET=… scripts/cloudflare-setup.sh --zone clax.example.com \
     --github-client-id Ov23li… --owner-github <your login> --i-know-what-i-am-doing
   ```
3. Open `https://clax.example.com/signin` and choose **Sign in with
   GitHub**.

### Terms of Service and Privacy Policy

The Terms of Service and Privacy Policy in `docs/legal` are Empathic's, for
the hosted service it operates; a deployment shows them only when told to:

- `--legal bundled` serves them at `/terms` and `/privacy`.
- `--terms-url <URL> --privacy-url <URL> --legal-version <v>` links your
  own documents instead.
- With neither, the instance shows no documents and asks no one to agree.

With either, the sign-in, join and setup pages say that signing in agrees
to them, and each sign-in there records the person's agreement to the
current version. A signed-in person who has not agreed to it, including
everyone who signed up before the documents were turned on, is asked once.
Change the version (`BUNDLED_LEGAL_VERSION` in
`crates/clax-core/src/config.rs` for the bundled documents) only for a
material change: everyone is asked again. The deploy record keeps the
setting for later deploys.

### Deploying by hand

```sh
../../scripts/build-web.sh
pnpm exec wrangler deploy
```

`wrangler deploy` runs `build.sh` (the release build, wasm-opt and the shim;
with Bazel or worker-build). `pnpm run size` prints the upload size without
deploying. `deploy.mjs` does this
with its generated `target/deploy/wrangler.deploy.jsonc` instead.

### DNS for the public host and the site domain

The public host and every site host are the same Worker, on zones on
Cloudflare (so `CF-Connecting-IP` names each client, which the sign-in
limits count by): the public host (`clax.example.com`) is a custom domain,
and the site domain (`example.run`) a zone of its own whose first-level
names, `<artifact ID>.example.run`, each serve one artifact. A custom
domain cannot be a wildcard, so the site domain is a route:

1. Add both zones to Cloudflare, on the Worker's account.
2. Route the public host as a custom domain and the site domain's hosts as
   a route on its zone (`deploy.mjs` does this), with `routes` in
   `wrangler.jsonc`:

   ```jsonc
   "routes": [
     { "pattern": "clax.example.com", "custom_domain": true },
     { "pattern": "*.example.run/*", "zone_name": "example.run" }
   ]
   ```
3. In the site domain's zone, add a proxied DNS record for `*` (type
   `AAAA`, content `100::`, Proxied): a route answers only for names that
   resolve to Cloudflare.
4. A certificate whose hosts include `*.example.run`: Universal SSL's covers
   a zone's first-level wildcard when the site domain is a zone of its own;
   otherwise Advanced Certificate Manager or Total TLS. `deploy.mjs`
   connects to a random site host and reads the certificate's names, which
   decides.
5. Set `PUBLIC_ORIGIN` to the public host's `https://` origin and
   `SITE_DOMAIN` to the site domain, and deploy again.
6. Turn the Worker's `workers.dev` address and preview URLs off
   (`"workers_dev": false` and `"preview_urls": false` in
   `wrangler.jsonc`): the instance refuses their host (403
   `forbidden_host`), but the Worker would still answer there.

`deploy.mjs` cannot make steps 1, 3 and 4 through wrangler; after a deploy
it prints each one that a site host's health shows undone, with what it
found (the zone's name servers, where a site host resolves, the
certificate's names). The site domain must be a domain of its own, neither
the public host nor under it nor above it: each artifact's scripts run on
its host, so it must share no cookies or storage with the app or with the
other artifacts. Until the site domain is on the Public Suffix List
(`docs/hosted/public-suffix.md`), sibling site hosts are same-site to
browsers; the platform's cookies are host-only (`__Host-`) either way.

### After the deploy

1. Open `https://clax.example.com/setup`, enter `SETUP_SECRET` and the
   owner's name: this signs that browser in as the owner and closes setup.
   (`/setup?secret=<SETUP_SECRET>` fills the field in, but sends the secret
   in the request's URL, which Cloudflare's request logs keep.)
2. On People, invite people (each invite is a one-use link), share pages by
   link for guests, and make sign-in links for other devices.
3. On People, each person mints agent tokens for their agents; the page
   shows the `claude mcp add --transport http clax https://clax.example.com/mcp
   --header "Authorization: Bearer cxa_…"` command and Codex's config. An
   agent acts with its person's rights.

### What has not run against the real platform

Every check above runs under `wrangler dev --local`; `smoke-remote.mjs`
is the first run against a deployed Worker, and each item below is one of
its steps. Not yet run against a deployed Worker:

- Cloudflare Artifacts (`REPO_STORE=artifacts`): it has no local simulator,
  so `src/repos/artifacts.rs` has run only against its tests' doubles. The
  error codes it retries on, the tokens of a repository deleted and made
  again under one name, and the cost of each version read (a ref
  advertisement and a binding call per tree level) are unverified there.
- Hibernation itself: the e2e simulates a wake by rebuilding the server on
  the same storage (`POST /_worker/hibernate`); a real eviction also drops
  in-flight requests and the isolate's memory, which the instance rebuilds
  from storage in the same way.
- The custom domains, certificates and `CF-Connecting-IP` from Cloudflare's
  edge (under `wrangler dev` every client is one address).
- The Bazel build on Linux (CI): the bundle has been built on macOS only
  (the NDK's Linux LLVM compiles its C there).
- ClaxBot's sandbox on real dynamic Workers: the `cpuMs` limit and the
  isolate's memory limit (and the error messages `src/sandbox/mod.rs`
  `platform_error` reads them by), `abortIsolate` after a script's last
  message, and the eight conformance cases `test-sandbox.mjs` cannot run
  locally.
- ClaxBot's runs on a real Durable Object: an alarm driving for minutes
  (the 15-minute wall limit, and the 30 s of CPU an invocation has by
  default), an invocation the platform ends mid-step and the alarm kept a
  minute ahead that follows it, and a run's SSE stream held open from the
  object through Cloudflare's edge.
