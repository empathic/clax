# Prod: releases and deploys from one branch

What clax.dev runs and what users download is one known commit: the tip of
the branch `prod`. Every change to `prod` is a release that arrives by pull
request, and every push to `prod` deploys clax.dev from GitHub Actions.
The design, and the calls it made, are in
[`docs/superpowers/specs/2026-10-27-prod-releases-design.md`](superpowers/specs/2026-10-27-prod-releases-design.md);
cutting a release is `docs/usage.md`, "Releasing". The `dev` environment
(from `main`) and the pull requests' previews are
[docs/environments.md](environments.md).

## How it fits together

1. `just release X.Y.Z` cuts `release/X.Y.Z` from `origin/prod`, merges
   `origin/main` into it, bumps the version and opens a pull request into
   `prod`.
2. CI runs on the pull request (`.github/workflows/ci.yml`). The ruleset
   on `prod` requires its `bazel`, `worker` and `version` jobs to pass. The
   `version` job runs only on pull requests into `prod`:
   `scripts/check-version-bump.sh origin/prod` passes when the head's
   version is `X.Y.Z`, every version file agrees
   (`scripts/check-version.sh`) and the version is greater than `prod`'s.
3. The owner merges it (only a repository admin can merge into `prod`)
   with a merge commit (the only method the ruleset allows: the next release branch's merge of `main` needs `main`'s
   commits in `prod`'s history). The push to `prod` runs Deploy prod
   (`.github/workflows/deploy-prod.yml`).
4. `just release X.Y.Z` again, on `prod`, tags, pins the plugins, pushes
   the pin commit to `prod` (the owner bypasses the ruleset for it, which
   deploys again with no server change), carries both commits to `main`
   and publishes the Pi package from `prod`.
5. When the release brought a new SDK package version (the `version` in
   `web/sdk/package.json`) to `prod`, `just release-sdk X.Y.Z`
   tags `sdk-vX.Y.Z` on `prod` and publishes `@empathic/clax-client-sdk` from it
   (docs/usage.md, "Releasing the SDK package"); the release's last step,
   "Left by hand", names the command while npm lacks that version. Until
   origin has a `prod` branch (the one-time setup below creates it),
   `just release-sdk` tags and publishes from `main` instead, and the
   `npm` environment allows `main`; once `prod` exists, it publishes from
   `prod` with no change to the command.

## One-time GitHub setup

Run, from a checkout, with `gh` signed in as a repository admin:

```
scripts/setup-prod-github.sh                            # dry run: every read, and each write it would make
scripts/setup-prod-github.sh --i-know-what-i-am-doing   # make them
```

It is safe to run again; it reads what exists and changes only what
differs (the ruleset it writes whole each time). It

- creates the branch `prod` at the commit clax.dev runs now (the `commit`
  of `https://clax.dev/healthz`, which must be on `main`; `--commit <sha>`
  names another), unless `prod` exists;
- creates or updates the repository ruleset `prod` on `refs/heads/prod`:
  pull request required, merge commits only, required checks `bazel`,
  `worker` and `version` from GitHub Actions, no deletion, no force push,
  signed commits (GitHub signs the merge commits it makes), and updates
  restricted (GitHub's "Restrict updates" rule): only the ruleset's bypass
  actors, the repository admin role, update `prod`. Anyone with write
  access can open a pull request into `prod`, but only an admin merges it,
  and an admin pushes the release's pin commit directly. GitHub cannot
  narrow a bypass to one person, so the repository's other admin can merge
  into and push to `prod` too. No approval is required: the owner cannot
  approve the release pull request `just release` opens as them;
- creates the environment `production`, deployable from `prod` only, with
  the variables the deploy reads: `PUBLIC_HOST`, `SITE_DOMAIN`, `STORE`,
  `OAUTH_CLIENT_ID` (the GitHub OAuth App's client ID; GitHub refuses
  variable names starting with `GITHUB_`), `OWNER_GITHUB`,
  `CLOUDFLARE_ACCOUNT_ID`, and `RETIRED_CONTENT_ORIGIN` when the instance
  has one. It takes their values from your deploy configuration, the one
  `scripts/cloudflare-setup.sh` keeps in `~/.config/clax/deploy`:
  `setup.env` (only the settings above; never its client secret) and the
  deploy record's `account` and `retired_content_origin`. A flag replaces
  each (`--help`), and a setting found in neither fails the run, naming
  its flag. None of these values is kept in this repository;
- sets the repository variable `PUBLIC_HOST` to the same host, for the
  drift check, which runs outside the environment;
- makes the environment `npm` deployable from `prod` in place of `main`
  (from then on `just release-sdk` publishes from `prod`, as it does once
  origin has a `prod` branch);
- prints the two commands that set the secrets, which it never runs:

```
gh secret set CLOUDFLARE_API_TOKEN --env production --repo empathic/clax
gh secret set CLAX_SMOKE_TOKEN --env production --repo empathic/clax
```

### The Cloudflare API token

Make it in the Cloudflare dashboard (My Profile, API Tokens, Create Token,
Custom token), limited to your Cloudflare account and to the zones of
the public host and the site domain (and the retired content host's zone,
if there is one). A full deploy re-applies every binding, the custom domain and the
route, and makes what is missing, so it needs:

| Scope | Permission | For |
| --- | --- | --- |
| Account | Workers Scripts: Edit | uploading the Worker, its Durable Objects, Worker Loader and secrets (`wrangler secret list`); attaching `clax.dev` as a custom domain (`PUT /accounts/{account}/workers/domains` accepts Workers Scripts Write) |
| Account | Workers R2 Storage: Edit | `wrangler r2 bucket create clax-assets` and the `ASSETS` binding |
| Account | Queues: Edit | `wrangler queues create clax-artifacts-events`, the Worker's consumer, and the smoke's `wrangler queues subscription create/delete` |
| Account | Artifacts (Edit) | the `clax` namespace: deploy.mjs lists namespaces and creates and deletes a probe repository |
| Account | Account Settings: Read | wrangler reading the account |
| Account | Stream: Edit | the landing page's reel: `scripts/reel-stream.mjs` lists, uploads and deletes the account's `clax-reel-*` videos and turns on their MP4 download (the account needs a Stream subscription; without either, the deploy goes on and the page shows no player) |
| Zone (`clax.run`) | Workers Routes: Edit | the route `*.clax.run/*` (`POST /zones/{zone}/workers/routes` accepts Workers Routes Write) |
| Zone (both) | Zone: Read | wrangler finding each zone by name |
| User | User Details: Read, Memberships: Read | `wrangler whoami`, which deploy.mjs runs to see it is signed in |

Confirmed from Cloudflare's docs: the permission names above (its API
token permissions reference), and that the custom-domain and route
endpoints accept Workers Scripts Write and Workers Routes Write. Not
confirmed:

- **Artifacts**: the permissions reference lists no Artifacts permission;
  take the one the dashboard offers for Artifacts with edit rights.
- **Queue event subscriptions**: whether `wrangler queues subscription`
  needs more than Queues: Edit.
- **Zone: Read, Account Settings: Read, User Details: Read and
  Memberships: Read**: wrangler's account and zone lookups and `whoami`
  are not documented per permission; Cloudflare's "Edit Cloudflare
  Workers" template carries them, and the Workers CI guide
  (developers.cloudflare.com/workers/ci-cd/external-cicd/github-actions/)
  recommends that template without listing its parts.
- **DNS: Edit** is not in the table: the custom-domain endpoint accepts
  Workers Scripts Write, and Cloudflare makes the domain's DNS record and
  certificate itself. If the first Actions deploy fails on the custom
  domain with an authentication error, add Zone, DNS: Edit on `clax.dev`.
- **Observability**: the Worker's `observability` setting is assumed to be
  part of the script's settings (Workers Scripts: Edit).

The token goes only to the `production` environment, which only runs on
`prod`.

### The smoke's agent token

The deploy's smoke runs as an agent token, never the owner token, which
never goes to GitHub. Signed in to clax.dev as the owner, make an agent
token named `smoke` (settings, Agent tokens) and set it as
`CLAX_SMOKE_TOKEN`. The smoke runs every step an agent can and names each
owner-only step it skips.

## Feedback: clax.dev's own report queue

clax.dev's people report problems with Clax itself through a report
queue, as any product's users do (spec
[`2026-10-29-feedback-loop-design`](superpowers/specs/2026-10-29-feedback-loop-design.md)
§10): the shell asks `POST /_app/feedback/token` for a reporter token, and
the instance mints it with the queue's service grant through the public
`POST /api/queues/<qid>/reporter-tokens` route
(`crates/clax-server/src/integrations/feedback.rs`). Every signed-in
person may report, the owner and each connected person who is not
revoked; no flag gates it.

It needs no setup. With neither secret below set, the instance makes its
own queue the first time a signed-in person's shell asks
(`GET /_app/feedback/config`), once the owner exists: `clax-feedback`,
the owner its admin, `https://clax.dev` (the public origin) its one
origin, reporters seeing their own reports; and a service grant of it
named `instance`. It keeps both in its store (`instance_feedback`; the
grant's token and private key in the clear, as they mint reporter tokens
on that queue alone) and mints with them through the public route, as an
outside backend would. The owner finds the queue on the Queues page and
adds the people who triage there (Members, or `PUT
/api/queues/<qid>/members/<public ID> {"role": "triage"}`). Later starts
reuse the kept queue and grant; a deleted queue is made again (a new
one, with a new grant), a revoked grant is replaced on the same queue,
and a public origin that moved is added to the queue's origins.

To file into a queue of your choosing instead, once, signed in to
clax.dev as the owner, with `CLAX_TOKEN` the owner's bearer token:

1. Make the queue for `https://clax.dev` (the Queues page, or):

   ```
   curl -sf https://clax.dev/api/queues -H "authorization: Bearer $CLAX_TOKEN" \
     -H 'content-type: application/json' \
     -d '{"name": "clax-feedback", "origins": ["https://clax.dev"], "reporters_see": "own"}'
   ```

   Its answer's `queue.id` is the queue ID.
2. Add the people who triage reports, as above.
3. Make its service grant: `POST /api/queues/<qid>/service-grants
   {"name": "clax.dev"}` answers `{grant, token, private_key}` once.
4. Store both as Worker secrets, from `crates/clax-worker` (`clax` is the
   Worker's name):

   ```
   printf %s "<queue ID>" | npx wrangler secret put CLAX_FEEDBACK_QUEUE --name clax
   printf '{"token":"%s","private_key":"%s"}' "<token>" "<private_key>" \
     | npx wrangler secret put CLAX_FEEDBACK_SERVICE_GRANT --name clax
   ```

   They are secrets, not vars: a deploy rewrites the Worker's vars from
   `wrangler.jsonc` and keeps its secrets. The configured queue wins over
   the instance's own. One secret without the other, or a grant that does
   not read, is logged and leaves the integration off.

In the shell (`web/shell/src/feedback/`), a signed-in person gets a
feedback menu in the gallery's header and the artifact view's top bar:
**Report a problem**, with its key F (a single letter like the shell's C
and ?; the keys sheet lists it), and **My reports**, with the count of reports waiting on them,
which the menu's button shows too. The shell embeds the overlay as any
product does: it asks `GET /_app/feedback/config` first; only when that
answers `enabled: true` does it load `<instance>/_clax/overlay.js`, start
`Clax.diagnostics.install()` and then `Clax.overlay({launcher: false, auth,
context})`, with `auth` the token route above. For anyone else that one
request is all. F does nothing with a modifier but Shift, in a text field
or in a dialog; pressed in the artifact's frame, the bridge forwards it as
it does C (`clax:shortcut`) unless the page handles it.

To check it: `GET https://clax.dev/_app/feedback/config` in a signed-in
person's browser answers `enabled: true` with the queue's ID. To go back
from a configured queue to the instance's own, delete both secrets (`npx
wrangler secret delete … --name clax`). To rotate a configured grant,
make a new one, put the secret again, and revoke the old one (`DELETE
/api/queues/<qid>/service-grants/<gid>`), which ends every reporter token
it minted; revoking the instance's own grant is enough, as it makes a new
one at the next request.

A daemon takes the same from `[feedback]` in its home's `config.toml`:
`queue`, `service_grant_file` (a JSON file `{token, private_key}`,
relative to the home; keep it 0600, as the daemon warns at start when
its group or others may read it) and, optionally, `origin` (by default a hosted
daemon's public origin, else its browser address). A hosted daemon with
no `[feedback]` makes its own queue as clax.dev does (so does `just
clax-dev-local`, which runs the Worker as clax.dev runs); a local daemon
that is not hosted reports only with `[feedback]`. `just dev-feedback`
(`scripts/dev-feedback.sh`) sets it up on the dev home (`~/.clax-dev`)
with a queue `clax-feedback-dev` for `http://localhost:<port>`, so the
loop runs locally end to end.

## The deploy workflow

Deploy prod runs on every push to `prod` and by hand (Actions, Deploy
prod, Run workflow, branch `prod`), which redeploys `prod`'s tip. One
deploy runs at a time and a running one is never cancelled; GitHub keeps
only the newest waiting run. It checks out the pushed commit and refuses
unless it is still `origin/prod`'s tip, sets up Node 26, pnpm, the Rust
toolchain of `rust-toolchain.toml` with `wasm32-unknown-unknown`, and
clang, lld and llvm, then runs `scripts/cloudflare-setup.sh` twice, with
`CLAX_DEPLOY_HOME` in the runner's temporary directory:

```
scripts/cloudflare-setup.sh --legal bundled --public "$PUBLIC_HOST" --site-domain "$SITE_DOMAIN" --store "$STORE" \
  [--github-client-id "$OAUTH_CLIENT_ID" --owner-github "$OWNER_GITHUB"] [--retired-content-origin "$RETIRED_CONTENT_ORIGIN"] \
  --account "$CLOUDFLARE_ACCOUNT_ID" --ci --production                                         # build
... --ci --production --no-build --smoke --i-know-what-i-am-doing                            # deploy
```

The first, a dry run in a step that has neither secret, installs the
packages and builds the web UI and the Worker from the commit, so no
package's install script and no crate's build script runs with the
Cloudflare token. The second, the one step given `CLOUDFLARE_API_TOKEN`
and `CLAX_SMOKE_TOKEN`, installs and builds nothing (`--no-build`; it
refuses a Worker not built from the checked-out commit), deploys, waits
for `/healthz` to report the build, checks a site host and runs the
smoke. Both run in one job, so the build step could still change files
the deploy step runs. The deploy record stays on the
runner. The job summary gives the commit, the record's build, the Cloudflare
Worker version the deploy made (the record's `version_id`, from `wrangler
deploy`'s "Current Version ID") and what `/healthz` answers. A failure fails the run and GitHub notifies the owner;
nothing is rolled back.

## CLAX_KEK

`CLAX_KEK` is ClaxBot's key-encryption key: 64 hex digits that seal every
model key the instance stores. Two copies exist: the Worker's secret, and
the backup its operator keeps, outside any machine and any CI run. A
deploy keeps the Worker's key and never reads it back (Cloudflare cannot),
so deploys need neither the key nor its backup. The backup is needed when
the Worker has no key: an instance's first deploy, a Worker deleted or
remade, or a secret removed or overwritten. Without both copies every
stored model key is lost.

A new instance's key (dev's included; previews make their own and keep
none) is 64 random hex digits, `openssl rand -hex 32`, made where the
backup is kept. A deploy to a Worker without a key uploads the one given in
the environment's `CLAX_KEK`, and refuses when none is. Give it without
leaving it in the shell's history or on disk:

```
read -rs CLAX_KEK && export CLAX_KEK   # paste the key from its backup
just env-deploy …                      # its first deploy
unset CLAX_KEK
```

Once deployed, the owner checks the backup is the Worker's key: Admin,
Provider shows the key's fingerprint (never public), and this prints the
backup's, with the key in `CLAX_KEK` as above:

```
printf %s "$CLAX_KEK" | xxd -r -p | shasum -a 256 | cut -c1-8
```

Deploys made before this kept the backup in a file,
`~/.config/clax/deploy/<public host>.kek` (or the record's `kek`) on the
machine that ran the first deploy; a deploy that finds one says so. Keep
that key with the backups, check the fingerprints match, then delete the
file.

When `/healthz` reports `"kek": false` (every deploy, and the drift check,
fail on it), put the key back from its backup:

```
read -rs CLAX_KEK && printf %s "$CLAX_KEK" | just wrangler secret put CLAX_KEK --name clax; unset CLAX_KEK
```

## Drift

Every hour the same workflow's `drift` job compares the `commit` that
`https://$PUBLIC_HOST/healthz` reports (the repository variable the setup
script sets; `clax.dev` when it is unset) with `prod`'s tip, and fails when they differ, unless
a Deploy prod run of the tip is still running or waiting. It also fails
when `/healthz` reports `"kek": false`: the instance has lost its
`CLAX_KEK` ("CLAX_KEK" above). It holds no
deploy credential and deploys nothing. When it fails: open the latest
Deploy prod run; if it failed, fix the cause and release again, or rerun
it; if none ran, run Deploy prod by hand. Scheduled runs use `main`'s copy
of the workflow.

## Rollback

A rollback is a release: revert the change on `main`, then
`just release X.Y.Z+1`. The patch release's pull request into `prod`
deploys when merged. Until it does, the drift check keeps failing whenever
clax.dev runs something other than `prod`'s tip.

## Deploying from a laptop

`just clax-dev-deploy` deploys clax.dev alone, so it always passes
`--production` (which also writes `PRODUCTION=1` to `setup.env`): it
refuses to deploy from anywhere but GitHub Actions at `origin/prod`'s tip,
whatever `setup.env` says. `scripts/cloudflare-setup.sh` run directly
refuses the same for a `setup.env` with `PRODUCTION=1`. In an emergency, `--from-laptop`
lifts the Actions requirement only: the checkout must still be clean and at
`origin/prod`'s tip, so the laptop deploys exactly what Actions would. The
next drift check then passes, since clax.dev runs `prod`'s tip. First-time
setup of a new instance stays a laptop run.
