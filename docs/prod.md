# Prod: releases and deploys from one branch

What clax.dev runs and what users download is one known commit: the tip of
the branch `prod`. Every change to `prod` is a release that arrives by pull
request; while it is open it is staging, and merging it releases it from
GitHub Actions. The design, and the calls it made, are in
[`docs/superpowers/specs/2026-11-04-release-by-pr-merge-design.md`](superpowers/specs/2026-11-04-release-by-pr-merge-design.md)
(which replaced the flow of
[`2026-10-27-prod-releases-design`](superpowers/specs/2026-10-27-prod-releases-design.md));
the runbook is `docs/usage.md`, "Releasing". The `dev` environment
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
   commits in `prod`'s history). While it is open, each push to its branch
   deploys staging ("Staging", below) from the release candidate, built
   once and attested.
4. The push to `prod` runs Deploy prod (`.github/workflows/deploy-prod.yml`,
   "The deploy workflow" below): it checks the candidate staging tested,
   waits for the owner's approval, deploys clax.dev from it, publishes the
   GitHub release `vX.Y.Z` from its files, carries the release and the
   plugins' pin to `main` as the release App ("The release App", below),
   publishes the Pi package from `main` and tears staging down. Nothing
   else pushes to `prod`, which never holds the pin.
5. The SDK package, `@empathic/clax-client-sdk`, is not part of this:
   `just release-sdk X.Y.Z`, run by hand, tags `sdk-vX.Y.Z` on `main` and
   publishes it from there (docs/usage.md, "Releasing the SDK package"),
   whether or not `prod` exists.

## One-time GitHub setup

Run, from a checkout, with `gh` signed in as a repository admin:

```
scripts/setup-prod-github.sh                            # dry run: every read, and each write it would make
scripts/setup-prod-github.sh --i-know-what-i-am-doing   # make them
```

It is safe to run again; it reads what exists and changes only what
differs (the rulesets it writes whole each time). It

- creates the branch `prod` at the commit clax.dev runs now (the `commit`
  of `https://clax.dev/healthz`, which must be on `main`; `--commit <sha>`
  names another), unless `prod` exists;
- creates or updates the repository ruleset `prod` on `refs/heads/prod`:
  pull request required, merge commits only, required checks `bazel`,
  `worker` and `version` from GitHub Actions, no deletion, no force push,
  signed commits (GitHub signs the merge commits it makes), and updates
  restricted (GitHub's "Restrict updates" rule): only the ruleset's bypass
  actors, the repository admin role, update `prod`. Anyone with write
  access can open a pull request into `prod`, but only an admin merges it.
  GitHub cannot narrow a bypass to one person, so the repository's other
  admin can merge into and push to `prod` too. No approval is required: the owner cannot
  approve the release pull request `just release` opens as them;
- creates the environment `production`, deployable from `prod` only, with
  the variables the deploy reads: `PUBLIC_HOST`, `SITE_DOMAIN`, `STORE`,
  and `RETIRED_CONTENT_ORIGIN` when the instance has one. The deploy reads
  three more settings as the environment's secrets, so that they stay out
  of the repository's settings and its logs: `CLOUDFLARE_ACCOUNT_ID`,
  `OAUTH_CLIENT_ID` (the GitHub OAuth App's client ID; GitHub refuses
  names starting with `GITHUB_`) and `OWNER_GITHUB`. A variable left with
  one of those names by an earlier setup is removed. It takes all their
  values from your deploy configuration, the one
  `scripts/cloudflare-setup.sh` keeps in `~/.config/clax/deploy`:
  `setup.env` (only the settings above; never its client secret) and the
  deploy record's `account` and `retired_content_origin`. A flag replaces
  each (`--help`), and a setting found in neither fails the run, naming
  its flag. None of these values is kept in this repository;
- sets the repository variable `PUBLIC_HOST` to the same host, for the
  drift check, which runs outside the environment;
- makes the environment `npm` deployable from `prod` and `main` alone:
  `publish-pi.yml` and `publish-sdk.yml` run in it from `main`;
- creates or updates the repository ruleset `release` on
  `refs/heads/release/*`: only its bypass actors, the repository admins,
  create, update or delete a release branch. `just release` cuts each
  release onto `release/X.Y.Z`, and CI refuses a pull request into `prod`
  from any other branch (`scripts/check-release-head.sh`);
- makes every `production` deploy wait for the owner's approval: the
  environment's required reviewer is `OWNER_GITHUB`, which the run
  therefore needs (`--owner-github`). The owner merges the release pull
  request, then approves its deploy in the run (Review deployments);
- creates the environment `staging`, deployable from `release/*` and
  `main` alone, with the variables `STAGING_HOST_ZONE`
  (`clax-staging.dev`) and `STAGING_SITE_ZONE` (`clax-staging.run`):
  "Staging", below. GitHub matches an environment's branch rule against a
  run's `GITHUB_REF`, which a `pull_request` run has as
  `refs/pull/<N>/merge`, so no pull request's run reaches its secrets; a
  `staging` environment that deploys from every branch, or from another
  one, is narrowed;
- creates the environment `release`, deployable from `prod` alone, for
  the release GitHub App's secrets;
- prints the commands that set the secrets, which it never runs: those
  with `--body` take values from your deploy configuration, the others
  are yours to paste into:

```
gh secret set CLOUDFLARE_ACCOUNT_ID --env production --repo empathic/clax --body <account ID>
gh secret set OAUTH_CLIENT_ID --env production --repo empathic/clax --body <client ID>
gh secret set OWNER_GITHUB --env production --repo empathic/clax --body <login>
gh secret set CLOUDFLARE_API_TOKEN --env production --repo empathic/clax
gh secret set CLAX_SMOKE_TOKEN --env production --repo empathic/clax
gh secret set CLOUDFLARE_API_TOKEN --env staging --repo empathic/clax
gh secret set CLOUDFLARE_ACCOUNT_ID --env staging --repo empathic/clax --body <account ID>
gh secret set OAUTH_CLIENT_ID --env staging --repo empathic/clax
gh secret set OAUTH_CLIENT_SECRET --env staging --repo empathic/clax
gh secret set OWNER_GITHUB --env staging --repo empathic/clax --body <login>
gh secret set RELEASE_APP_ID --env release --repo empathic/clax
gh secret set RELEASE_APP_PRIVATE_KEY --env release --repo empathic/clax < <the App's private key, .pem>
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

### ClaxBot's web search

The Worker binds Workers AI (`AI`, `crates/clax-worker/wrangler.jsonc`)
for ClaxBot's `clax.web.search` and `clax.web.fetch`. A deploy carries the
binding whatever the settings; nothing is billed until an admin turns
search on (the admin's web settings, `PUT /api/admin/web`). Each search
then bills the account through the AI Gateway the admin names (`default`
unless set): its **AI Gateway credits**, or a **provider key stored on the
gateway** under the alias the admin sets as `search.byok_alias` (the
provider bills that key). Before turning search on in production, load
credits on the account or store the provider's key on the gateway;
without either, every search answers `web_unavailable` ("no credits and
no provider key") and is not charged to anyone. Fetches and their Markdown
conversion need neither. Not confirmed: whether uploading a Worker with an
`ai` binding needs more of the API token than Workers Scripts: Edit.
`smoke-remote.mjs` makes one real search and one real fetch once search is
on (`crates/clax-worker/README.md`, "ClaxBot's runs").

### The smoke's agent token

The deploy's smoke runs as an agent token, never the owner token, which
never goes to GitHub. Signed in to clax.dev as the owner, make an agent
token named `smoke` (People, Agent tokens: Machine `smoke`, Create token;
the token is the part after `Bearer ` in the command it shows) and set it
as `CLAX_SMOKE_TOKEN`. The smoke runs every step an agent can and names
each owner-only step it skips.

### Staging

A release pull request into `prod` is staging while it is open: a fresh
instance, `clax-pr-<N>` at `https://pr-<N>.clax-staging.dev` with its
sites at `<ID>.pr-<N>.clax-staging.run`, on prod's Cloudflare account and
configured as clax.dev is (design:
[`2026-11-04-release-by-pr-merge-design`](superpowers/specs/2026-11-04-release-by-pr-merge-design.md)).
It runs on prod's account so that the plan, the features turned on and
the limits are prod's own. `clax-staging.dev` and `clax-staging.run` are
two registrable domains of their own, as `clax.dev` and `clax.run` are,
so neither is the same site as clax.dev to a browser.

`.github/workflows/staging.yml` deploys it on every push to the release
branch (the run finds `<N>` from the branch's open pull request into
`prod`, waiting up to two minutes for `just release` to open it):

- **the candidate**, built once in jobs with no environment: the release
  binaries with `SHA256SUMS`, `install.sh` and `SOURCE_TREE`
  (`release.yml`, called with `candidate: true`; the workflow artifact
  `release-dist`), and the Worker with the web UI it embeds (the
  artifact `worker`, one `worker.tar`). Each file is attested
  (`actions/attest-build-provenance`) as made by this repository's
  workflow at the pushed commit;
- **the deploy**, in the `staging` environment: that Worker
  (`cloudflare-setup.sh --env pr-<N> --staging --ci --no-build --smoke`),
  with the bundled legal pages, Cloudflare Stream's newest reel and
  GitHub sign-in through the staging OAuth App, `OWNER_GITHUB` signing
  in as the owner. Each deploy gives the Worker a new owner token, kept in
  the runner's deploy record and never printed; the smoke turns it into
  an owner sign-in link, mints an agent token with the session and runs
  as that token alone, as the production smoke runs as
  `CLAX_SMOKE_TOKEN` (`smoke-remote.mjs --mint-agent-token`), revoking it
  at the end. The run then comments the address on the pull request.

A release pull request gets no preview (`preview.yml`); its staging is
torn down by the reaper's `reap-staging` job (`preview-reaper.yml`, daily
from `main`, in the `staging` environment) once the pull request is
closed.

Its credentials, in the `staging` environment:

- **The Cloudflare API token**, limited to prod's account and the zones
  `clax-staging.dev` and `clax-staging.run`, with prod's permissions
  (the table above) except Stream: Read in place of Stream: Edit, and
  with DNS: Edit on `clax-staging.run` for each instance's
  `*.pr-<N>.clax-staging.run` record. Cloudflare scopes Workers Scripts:
  Edit to an account, not a script, so this token can change prod's
  Worker too: only the repository admins, who can merge into `prod`
  already, run code with it (the `release` ruleset and the environment's
  branch rule, above).
- **A GitHub OAuth App of its own** (GitHub, Settings, Developer settings,
  OAuth Apps, New OAuth App): homepage `https://clax-staging.dev`,
  authorization callback `https://clax-staging.dev/auth/github/callback`
  with "Enable wildcard matching" on, so every `pr-<N>.clax-staging.dev`
  host matches. Its client ID is `OAUTH_CLIENT_ID`, its client secret
  `OAUTH_CLIENT_SECRET`.
- `CLOUDFLARE_ACCOUNT_ID` and `OWNER_GITHUB`, prod's.

`clax-staging.run` needs Total TLS (Advanced Certificate Manager; the
zone's SSL/TLS, Edge Certificates): a site host is two levels below the
zone, beyond what the universal certificate covers, as on the previews'
site zone.

### The release App

Deploy prod's `carry` job carries each release to `main` as a GitHub App
of its own: a workflow's own token starts no workflow with what it
pushes or opens, so `main`'s CI would never run on the carry. Create it
once (GitHub, the organization's Settings, Developer settings, GitHub
Apps, New GitHub App): no webhook, no callback; repository permissions
Contents: Read and write and Pull requests: Read and write, nothing else;
installed on `empathic/clax` alone. Its App ID is the `release`
environment's secret `RELEASE_APP_ID`, and a private key it generates
(the `.pem` file) `RELEASE_APP_PRIVATE_KEY`; the job mints a token from
them (`actions/create-github-app-token`) that lasts the job. The App is on
no ruleset's bypass list: its commit on `main`, made through GitHub's Git
Data API, is signed by GitHub, which `main`'s signed-commit rule takes, and
it merges only once CI's `bazel` job passed on its pull request. The
`release` environment admits `prod` alone, so only Deploy prod's runs on
`prod` read the key.

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

Deploy prod runs on every push to `prod`, which is a release pull
request's merge, and by hand (Actions, Deploy prod, Run workflow, branch
`prod`), which runs it again for `prod`'s tip (with `redeploy`, uploading
the Worker even when clax.dev already runs it: to apply a changed
variable, say). It builds nothing that
ships; its jobs, in order:

- **candidate**, with no environment and no secret:
  `scripts/find-candidate.sh <merge commit>` names the merge's second
  parent (the release branch's head, which staging tested), refusing a
  commit that is no merge, a merge whose tree is not the head's (`prod`
  moved while the pull request was open) and a head with no successful
  `staging.yml` run; it downloads that run's `release-dist` and `worker`
  artifacts, and `scripts/verify-candidate.sh` checks each file with `gh
  attestation verify --repo empathic/clax --signer-workflow
  empathic/clax/.github/workflows/<release.yml or staging.yml>
  --source-digest <head>`, refusing the candidate if one file fails, or
  any file is not one a candidate has. The verified files become this
  run's artifacts.
- **reel**, beside it: the landing page's reel, built from the commit
  with no secret; the deploy puts it on Cloudflare Stream, and a reel
  that fails to build or upload never fails the deploy.
- **deploy**, in the `production` environment, so once the owner
  approves: it refuses unless the commit is still `origin/prod`'s tip
  (GitHub keeps only the newest waiting run, and one deploy runs at a
  time, never cancelled), installs the deploy's packages before any
  secret is in the environment, unpacks the candidate Worker and runs

  ```
  scripts/cloudflare-setup.sh --legal bundled --public "$PUBLIC_HOST" --site-domain "$SITE_DOMAIN" --store "$STORE" \
    [--github-client-id "$OAUTH_CLIENT_ID" --owner-github "$OWNER_GITHUB"] [--retired-content-origin "$RETIRED_CONTENT_ORIGIN"] \
    [--reel-url … --reel-poster-url …] --account "$CLOUDFLARE_ACCOUNT_ID" \
    --ci --production --no-build --smoke --i-know-what-i-am-doing
  ```

  with `CLAX_DEPLOY_HOME` in the runner's temporary directory, the one
  step given `CLOUDFLARE_API_TOKEN` and `CLAX_SMOKE_TOKEN`. `--no-build`
  installs and builds nothing and refuses a Worker whose `built-from.json`
  names a commit with another tree than the checkout's (the candidate was
  built from the release branch's head; the checkout is the merge commit,
  with the same tree); it deploys, waits for `/healthz` to report the
  build, checks a site host and runs the smoke. With `--unless-live`,
  which the step passes unless the run was started by hand with
  `redeploy`, a Worker whose `/healthz` already reports the merge commit
  is not uploaded again: the record is written for the build that runs,
  and the checks and the smoke are made of it, so a run again restarts no
  instance. The smoke's push step warns, and does not fail, when
  Cloudflare delivers no `pushed` event and the publish after the push
  records it instead. The deploy record stays on
  the runner. The job summary gives the commit, the head it was built
  from, the record's build, the Cloudflare Worker version the deploy made
  (the record's `version_id`, from `wrangler deploy`'s "Current Version
  ID") and what `/healthz` answers.
- **publish:** the tag `vX.Y.Z` on the merge commit and the GitHub
  release as a draft (a published release is immutable, the repository's
  setting, so every asset is on it before it is published),
  from `release-dist`, once its `SOURCE_TREE` is the merge commit's tree;
  a release of the version from that tree already is kept, one from
  another tree fails the job (`scripts/release-exists-check.sh --tree`).
  The tag, made with the workflow's token, runs no workflow.
- **fleet:** so `fleet-image.yml` is run on the tag here, and waited for,
  unless a run of the tag passed or is running. It adds `clax-fleet.yaml`
  to the draft.
- **release:** publishes the draft, once it holds `clax-fleet.yaml`; a
  release published already is kept.
- **carry**, in the `release` environment: `scripts/carry-release.sh`, as
  the release App ("The release App", above).
- **pi:** once CI's `bazel` job passed on the carry's commit of `main`
  (`scripts/wait-ci.sh`), Publish Pi package from it, unless npm has the
  version.
- **teardown:** the reaper, dispatched on `main` (where the `staging`
  environment admits it), removes the staging instance of the merged pull
  request.

A failure fails the run and GitHub notifies the owner; nothing is rolled
back. Each job finds what an earlier run did, so a failed run is rerun
once its cause is fixed, and finishes the release without deploying
clax.dev again.

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

Every hour the same workflow's `drift` job compares the tree of the
`commit` that `https://$PUBLIC_HOST/healthz` reports (the repository
variable the setup script sets) with `prod`'s tip's, and fails when they
differ, unless a Deploy prod run of the tip is still running or waiting.
Trees, not commits: clax.dev runs the Worker built from the release
branch's head, which reports that commit, and `prod`'s tip is its merge,
with the same tree. It also fails
when `/healthz` reports `"kek": false`: the instance has lost its
`CLAX_KEK` ("CLAX_KEK" above). It holds no
deploy credential and deploys nothing. When it fails: open the latest
Deploy prod run; if it failed, fix the cause and release again, or rerun
it; if none ran, run Deploy prod by hand. Scheduled runs use `main`'s copy
of the workflow.

## Rollback

A rollback is a release: revert the change on `main`, then
`just release X.Y.Z+1`. The patch release's pull request is staging like
any other, and deploys when merged and approved. Until it does, the drift
check keeps failing whenever clax.dev runs something other than `prod`'s
tip. In an emergency, `wrangler rollback <version ID>` (the Cloudflare
version a Deploy prod run's summary names) returns the Worker to an
earlier deploy at once; the drift check then fails until the patch
release is out.

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
