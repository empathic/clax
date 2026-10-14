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
  origin, live pointers, a spectator, and seats across a simulated wake.

Each takes `--no-build` to skip `build.sh`.

## Deploy to Cloudflare

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

Create the bucket once: `npx wrangler r2 bucket create clax-assets`. The
Durable Object classes are created by the migrations in `wrangler.jsonc` on
the first deploy. Artifacts needs the namespace enabled on the account.

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

Set them in `wrangler.jsonc`'s `vars` or with `--var` on `wrangler deploy`.
A hosted instance whose `PUBLIC_ORIGIN` is not an `https://` origin (or
`http://` on localhost), or whose `CONTENT_ORIGIN` shares its host, does not
start: every request answers 500 naming the var.

### Secrets

```sh
npx wrangler secret put SETUP_SECRET   # what /setup takes, once, to make the owner
npx wrangler secret put CLAX_TOKEN     # the owner's bearer token for the CLI (optional)
```

Without `SETUP_SECRET`, `/setup` stays closed. Without `CLAX_TOKEN` the
instance makes a token and keeps it in its storage, where nothing hands it
out: set one if you mean to use the CLI against the instance. Use a long
random value for both.

### Deploying

```sh
../../scripts/build-web.sh
npx wrangler deploy
```

`wrangler deploy` runs `build.sh` (worker-build, release, wasm-opt). `npm
run size` prints the upload size without deploying.

### DNS for the two origins

Both origins are the same Worker on two hostnames of a zone on Cloudflare
(so `CF-Connecting-IP` names each client, which the sign-in limits count
by):

1. Add the zone (`clax.example.com`'s) to Cloudflare.
2. Route both hostnames to the Worker, as custom domains (Workers > the
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

Every check above runs under `wrangler dev --local`. Not yet run against a
deployed Worker:

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
