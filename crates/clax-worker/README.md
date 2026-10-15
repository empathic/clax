# clax-worker

The Clax server in a Cloudflare Worker: one Durable Object per Clax instance
(`InstanceDo`, `src/instance.rs`), the web UI from Workers Static Assets,
artifact versions in git (`RepoDO`'s stand-in or Cloudflare Artifacts) and
assets in R2. `src/lib.rs` documents how the Worker routes requests and
`src/instance.rs` what the instance does (hosted mode, sockets, seats across
a wake, the alarm).

## Developing

From this directory: `npm install`, build the web UI first
(`../../scripts/build-web.sh`), then

- `npm run dev`: `wrangler dev --local` on `.wrangler/state` (runs
  `build.sh`);
- `cargo test`: the VFS's tests and the crate's unit tests, natively;
- `node test-wasm.mjs`: the SQLite-on-Durable-Object spike across a restart;
- `node test-repos.mjs`: the repository hosts (`CLAX_TEST_JJ` names a `jj`
  for the jj clone check);
- `node e2e.mjs`: the instance in two browsers with the owner token;
- `node e2e-hosted.mjs`: the instance as a hosted Clax: setup, an invite, a
  sign-in link, an agent token on `/mcp`, a guest, content on its own
  origin, live pointers, a spectator, and seats across a simulated wake;
- `node smoke-remote.mjs --local`: the deployed instance's smoke (below)
  against a fresh hosted `wrangler dev --local` with `REPO_STORE=memory`.

Each takes `--no-build` to skip `build.sh`.

## Deploy to Cloudflare

### In one command

On an account with Workers Paid (and the Artifacts beta, for
`REPO_STORE=artifacts`), with a zone on it for the two hostnames:

```sh
npx wrangler login                      # once, in this directory (npm install first)
just hosted-deploy --zone example.com   # or: node deploy.mjs --zone example.com
node smoke-remote.mjs --deploy .deploy/clax.example.com.json
```

`deploy.mjs` (its header lists every option) checks the login, takes the
two hostnames (`--public`, `--content`; with `--zone example.com` it offers
`clax.example.com` and `clax-content.example.com`, asking on a terminal),
creates what is missing (the R2 bucket; with Artifacts the queue and the
namespace, which also proves the account has Artifacts), generates
`SETUP_SECRET` and `CLAX_TOKEN` when the Worker has neither and uploads
them with the first version (printed once, and kept with the origins in
`.deploy/<public host>.json`, 0600 and ignored by git), writes
`wrangler.deploy.jsonc` (this file's config with `HOSTED=1`, both origins,
the store, both hosts as custom domains, `workers_dev` and preview URLs
off, Workers Logs on, `CLAX_DIAG_ROUTES=1`), builds the web UI and the
Worker, deploys, waits for `/healthz` on the public host (saying whether it
waits on DNS, the certificate or the Worker), and prints the setup URL and
the next steps. Run again, it changes only what differs; secrets are kept
unless `--rotate-secrets`.

- `--dry-run` does everything but change the account, ending at `wrangler
  deploy --dry-run` with the upload size; without a login it skips each
  account call and names it.
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
the secret (or `--session` for an existing owner); an agent token; two
versions published over `/mcp` and read back; a fork; a git token, `git
clone`, the clone's trees against the binding's; `git push` and the push
becoming a version (through the queue's `pushed` event, for which it
subscribes the artifact's repository while it runs; when no event comes, a
direct reconcile tells a missing event from a failing reconcile); and a
comment arriving live on a WebSocket stream. When it made the owner it
prints a sign-in link for the owner's browser. The diagnostics routes
(`/_diag/`, `CLAX_DIAG_ROUTES=1` and the owner token) give it the probe,
the git token and the reconcile.

### What the Worker binds

`wrangler.jsonc` declares all of them; a deploy needs each to exist on the
account.

| Binding | Kind | What it holds |
|---|---|---|
| `CLAX_INSTANCE_DO` | Durable Object (`InstanceDo`, SQLite-backed) | One Clax instance: its database (through the VFS), its sockets, its alarm |
| `REPO` | Durable Object (`RepoDO`, SQLite-backed) | The git stand-in's repositories (`REPO_STORE=memory`) and the repository token ledger in both modes |
| `CLAX_DB` | Durable Object (`ClaxDb`) | The VFS spike; reached only with `CLAX_TEST_ROUTES=1` |
| `ASSETS` | R2 bucket `clax-assets` | Artifact assets, in both `REPO_STORE` modes |
| `ARTIFACTS` | Cloudflare Artifacts, namespace `clax` | Artifact versions as git repositories, when `REPO_STORE=artifacts` |
| `WEB` | Workers Static Assets, `../../web/dist` | The web UI (`run_worker_first`) |

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
| `CONTENT_ORIGIN` | `https://content.clax.example.com` | Where artifact content is served, and nothing else |
| `REPO_STORE` | `artifacts` (or `memory`) | Where versions are kept |
| `GIT_SMART_HTTP` | `0` with `artifacts` | `1` serves the stand-in over git smart HTTP at `/git/*` |
| `CLAX_INSTANCE` | empty | The instance object's name (`default`) |
| `CLAX_LIVE_PEOPLE_SEATS`, `CLAX_LIVE_GUEST_SEATS`, `CLAX_LIVE_AGENT_SEATS`, `CLAX_LIVE_SEAT_IDLE_S` | empty | Live seats per artifact and the idle release; empty takes 100, 20, 20 and 180 s |
| `CLAX_TEST_ROUTES`, `REPO_DEV_ROUTES` | `0` | Test-only routes; never `1` on a deploy |
| `CLAX_DIAG_ROUTES` | `0`; `1` (`deploy.mjs --diag`) only until the smoke is green | `/_diag/` for the owner token (`src/diag.rs`): what the platform's bindings answer |

Set them in `wrangler.jsonc`'s `vars` or with `--var` on `wrangler deploy`.
A hosted instance whose `PUBLIC_ORIGIN` is not an `https://` origin (or
`http://` on localhost), or whose `CONTENT_ORIGIN` shares its host, does not
start: every request answers 500 naming the var.

### Secrets

```sh
npx wrangler secret put SETUP_SECRET   # what /setup takes, once, to make the owner
npx wrangler secret put CLAX_TOKEN     # the owner's bearer token for the CLI (optional)
```

`deploy.mjs` generates and uploads both on the first deploy. Without
`SETUP_SECRET`, `/setup` stays closed. Without `CLAX_TOKEN` the
instance makes a token and keeps it in its storage, where nothing hands it
out: set one if you mean to use the CLI against the instance. Use a long
random value for both; a `CLAX_TOKEN` under 32 characters is refused and
the instance does not start.

### Deploying by hand

```sh
../../scripts/build-web.sh
npx wrangler deploy
```

`wrangler deploy` runs `build.sh` (worker-build, release, wasm-opt). `npm
run size` prints the upload size without deploying. `deploy.mjs` does this
with its generated `wrangler.deploy.jsonc` instead.

### DNS for the two origins

Both origins are the same Worker on two hostnames of a zone on Cloudflare
(so `CF-Connecting-IP` names each client, which the sign-in limits count
by):

1. Add the zone (`clax.example.com`'s) to Cloudflare.
2. Route both hostnames to the Worker (`deploy.mjs` does this and the next
   two steps), as custom domains (Workers > the
   Worker > Settings > Domains & Routes > Add custom domain, once for
   `clax.example.com` and once for `content.clax.example.com`), which makes
   the DNS records and certificates; or with `routes` in `wrangler.jsonc`:

   ```jsonc
   "routes": [
     { "pattern": "clax.example.com", "custom_domain": true },
     { "pattern": "content.clax.example.com", "custom_domain": true }
   ]
   ```
3. Set `PUBLIC_ORIGIN` and `CONTENT_ORIGIN` to those two `https://`
   origins and deploy again.
4. Turn the Worker's `workers.dev` address and preview URLs off
   (`"workers_dev": false` and `"preview_urls": false` in
   `wrangler.jsonc`): the instance refuses their host (403
   `forbidden_host`), but the Worker would still answer there.

The content host must be one of its own, not a path of the public one: a
page's scripts run there, so it must share no cookies or storage with the
app. Its frames are sandboxed (an opaque origin per frame), and the page's
bridge talks only to the public origin, which its tag names.

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
- The build on Linux (CI): `build.sh` has run on macOS only.
