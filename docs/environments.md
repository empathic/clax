# Environments: prod, dev and previews

Hosted Clax runs in three kinds of environment, each a Cloudflare Worker
with resources of its own, deployed by the same script
(`scripts/cloudflare-setup.sh`, which runs `crates/clax-worker/deploy.mjs`)
from GitHub Actions:

| env | Worker | Hosts | Versions | Deployed by | GitHub environment |
|---|---|---|---|---|---|
| `prod` | `clax` | `clax.dev`, `<ID>.clax.run` | Artifacts (bucket `clax-assets`, queue `clax-artifacts-events`, namespace `clax`) | every push to `prod` (`deploy-prod.yml`, [docs/prod.md](prod.md)) | `production` |
| `dev` | `clax-dev` | `dev.PREVIEW_HOST_ZONE`, `<ID>.dev.PREVIEW_SITE_ZONE` | Artifacts (bucket `clax-assets-dev`, queue `clax-artifacts-events-dev`, namespace `clax-dev`) | every push to `main` whose `ci` run passes (`deploy-dev.yml`) | `preview` |
| `pr-<N>` | `clax-pr-<N>` | `pr-<N>.PREVIEW_HOST_ZONE`, `<ID>.pr-<N>.PREVIEW_SITE_ZONE` | Artifacts (queue `clax-artifacts-events-pr-<N>`, namespace `clax-pr-<N>`), assets in the shared bucket `clax-assets-preview` | every push to a pull request from a branch of this repository, except a release pull request into `prod`, and of ClaxBot's only a commit a person names (`preview.yml`) | `preview` |
| staging, `pr-<N>` | `clax-pr-<N>`, on prod's account | `pr-<N>.clax-staging.dev`, `<ID>.pr-<N>.clax-staging.run` | as a preview's, on prod's account | every push to a release branch whose pull request into `prod` is open (`staging.yml`, [docs/prod.md](prod.md), "Staging") | `staging` |

The `clax-dev` Worker is the dev environment, not clax.dev; clax.dev is
`prod`'s Worker `clax` (and `just clax-dev-deploy` deploys it).

Every environment has GitHub sign-in, and `OWNER_GITHUB` signs in as its
owner: prod through its own GitHub OAuth App, dev and the previews through
the preview one.

dev runs in the `preview` GitHub environment: dev and the previews need
the same Cloudflare account, token and OAuth App, and dev's hosts are
named from the preview zones. `deploy-dev.yml` deploys from `main` alone
(its job's condition); the environment itself admits any branch, for the
previews.

## How it fits together

`deploy.mjs --env <name>` (and `cloudflare-setup.sh --env <name>`, which
passes it on) picks the environment's names from one mapping,
`envOf` in `crates/clax-worker/deploy-lib.mjs`, which `teardown.mjs` reads
too, so a deploy and a teardown cannot disagree about which resources a
name refers to:

- **prod** keeps clax.dev's names, so its deploys are as they were.
- **dev** has a bucket, queue and Artifacts namespace of its own. Sharing
  prod's would mix the two: a queue takes one consumer Worker, and neither
  an asset's key (`assets/<artifact ID>/…`) nor a repository's name says
  which instance it belongs to.
- **A preview** (`pr-<N>`, or any lowercase name of at most 40 characters)
  keeps its versions on Artifacts, as dev and prod do, in a queue
  (`clax-artifacts-events-pr-<N>`) and namespace (`clax-pr-<N>`) of its
  own, made by its first deploy and deleted by its teardown. Its assets go
  to the one bucket every preview shares, `clax-assets-preview`, whose
  objects expire 30 days after they are written (an R2 lifecycle rule
  `deploy.mjs` sets), so a torn-down preview's assets do not outlive it by
  more than that. (`--store memory` still deploys one on the RepoDO
  stand-in, by hand.)

Each environment keeps its laptop settings in a file of its own beside
prod's `setup.env`: `~/.config/clax/deploy/setup.<name>.env`. The deploy
record is `<public host>.json` there, as for prod.

### A preview's hosts

A preview's site domain is `pr-<N>.PREVIEW_SITE_ZONE`, one level below the
zone, so its artifacts' hosts (`<ID>.pr-<N>.PREVIEW_SITE_ZONE`) are two
levels below it. The deploy, given `--site-zone PREVIEW_SITE_ZONE`:

- puts the Worker's route `*.pr-<N>.PREVIEW_SITE_ZONE/*` on that zone;
- makes the proxied DNS record `*.pr-<N>.PREVIEW_SITE_ZONE` (AAAA `100::`)
  through the Cloudflare API (`crates/clax-worker/cloudflare-api.mjs`);
- relies on **Total TLS** on the zone for the certificate: Universal SSL
  covers one level below the zone only. Total TLS issues a certificate for
  each proxied host, the wildcard record's included, and needs Advanced
  Certificate Manager.

The public host `pr-<N>.PREVIEW_HOST_ZONE` is a Workers custom domain, for
which Cloudflare makes the DNS record and the certificate itself.

`PREVIEW_HOST_ZONE` and `PREVIEW_SITE_ZONE` are two registrable domains of
their own (the same rule as prod's `clax.dev` and `clax.run`: a site must
never be the same site, to a browser, as Clax).

### A preview's lifecycle

- **First push:** a fresh instance. CI gives it no `SETUP_SECRET` (setup
  stays closed) and no `CLAX_TOKEN` (the instance makes its own); it
  generates its `CLAX_KEK` and keeps no copy, since the preview's data is
  thrown away with it (dev's and prod's are backed up by their operator,
  [docs/prod.md](prod.md), "CLAX_KEK"). The pull request gets a comment with its address.
  Sign in with GitHub as `OWNER_GITHUB` to be its owner; anyone else who
  signs in with GitHub is a new person, as on clax.dev.
- **Later pushes:** the same Worker, redeployed: its artifacts and people
  stay (they live in its Durable Objects).
- **Close or merge:** `preview.yml`'s teardown job runs
  `teardown.mjs pr-<N>`, which deletes the route, the DNS record, the
  custom domain, the Worker with its Durable Objects, then its queue and
  every repository in its namespace. The namespace itself stays, empty:
  Cloudflare's API deletes repositories, not namespaces.
- **Missed closes:** GitHub sends no `closed` event when it cannot build a
  pull request's merge ref. `preview-reaper.yml` runs daily (and by hand,
  with a dry run) `teardown.mjs --reap`, which tears down every preview
  whose pull request is closed or gone: each `clax-pr-<N>` Worker, and
  each `clax-artifacts-events-pr-<N>` queue a teardown left behind.

A preview runs no smoke: the deploy's health wait and site host check
fail its run. It runs on the Artifacts store, but nothing checks its queue
path or the diagnostics; dev's smoke does, on every merge to `main`.

Only writers' branches get previews: `preview.yml` deploys a pull request
whose branch is in this repository, which only someone with write access
can push. Pull requests from forks get no preview: their runs are not
given the `preview` environment's secrets. ClaxBot's branches are in this
repository too, and deploy only commit by commit, on a person's command
("Credentials", below).

## GitHub sign-in on every environment

dev and the previews share one GitHub OAuth App (GitHub, Settings,
Developer settings, OAuth Apps), created by hand, with the scope Clax asks
for (`read:user`) and no other setting beyond these:

| App | Homepage | Callback URL | Wildcard matching |
|---|---|---|---|
| preview | `https://PREVIEW_HOST_ZONE` | `https://PREVIEW_HOST_ZONE/auth/github/callback` | **on** |

Clax sends `redirect_uri=<public origin>/auth/github/callback`
(`crates/clax-server/src/github.rs`). With wildcard matching on, GitHub
takes any subdomain of the callback URL's host with the same port and a
path at or under its path, so the one App serves every
`https://pr-<N>.PREVIEW_HOST_ZONE` and `https://dev.PREVIEW_HOST_ZONE`
([GitHub's docs](https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/authorizing-oauth-apps),
"Redirect URLs"). The App's client ID is the environment's secret
`OAUTH_CLIENT_ID` (GitHub refuses secret names starting with `GITHUB_`)
and its client secret the environment's secret `OAUTH_CLIENT_SECRET`, which
the workflows pass to the deploy as `GITHUB_CLIENT_SECRET`.

The App's scope is what Clax asks for, not a limit on what a preview can ask
for. `read:user` is written in `crates/clax-http/src/github.rs`, which a pull
request may change, and a preview runs that pull request's code holding the
App's client ID and secret. So a preview could ask GitHub for `repo`, and
GitHub would show whoever signs in that request. Do not sign in to a preview
with an account that administers anything, and read the consent page:
`OWNER_GITHUB` in the `preview` environment should be an account with no rights
on anything that matters. Anything the App's secret reaches, a preview's code
reaches.

## What runs where

Each deploy of dev or a preview is two jobs on two runners. `build` runs the
commit's code (package install scripts, `build.rs`) with no environment and no
secret, and hands over one file, `worker.tar`. `deploy` holds the environment
and runs no code of that build, with one exception below:
`scripts/unpack-worker.py` takes only a Worker's files out of the archive, and
wrangler uploads them. If Cloudflare refuses a script at startup (error 10021),
wrangler profiles the bundle locally in workerd, beside the deploy's token.

A preview's `deploy` checks out the default branch, so its scripts, its
`wrangler.jsonc` and the lockfile wrangler comes from are all main's, and
`--built-from` names the commit deployed. A pull request's change to any of
those does not reach its own preview. The preview workflow itself still comes
from the branch: that binds ClaxBot, which cannot change `.github/`, and not a
writer. Neither job restores an Actions cache.

## Credentials

A Cloudflare API token's Workers permissions are account-wide: one that
can deploy `clax-pr-12` can also replace `clax`. A preview's `deploy` job holds
the `preview` environment's token and runs the default branch's scripts, not
the pull request's, but the job's own steps come from the branch's copy of
`preview.yml`, so anyone who can push a branch to this repository and edit that
file can use that token.

Who can use the preview token is who has write access to this repository:
only a writer can push the branch a preview runs, and pull requests from
forks get no preview (their runs are not given the environment's
secrets). A check on the pull request's author in `preview.yml` would
bound nothing more against a writer, since a run takes `preview.yml` from
the pull request itself. The split into `build` and `deploy` keeps a branch's
build scripts from the token; a writer who edits the workflow gets past it.

ClaxBot is the one pusher that is not a person: a model steered by a
reporter's text writes its branches, all under `claxbot/`, and nobody has
read them when they are pushed. So a pull request from a `claxbot/` branch,
or by the login in the repository variable `CLAXBOT_LOGIN`, deploys on none of
its `pull_request` events. A person approves one commit with a comment, the
command alone in it:

```
/preview <the head's full 40-character SHA>
```

- **The comment names the commit.** Read the pull request, then name the
  head you read. The job `approve` refuses any SHA but the current head, so a
  later push, ClaxBot's own second commit among them, deploys nothing until a
  new comment names it.
- **Only a person with write access, other than the author.** `approve`,
  which has no environment, no secret and no checkout, fails unless the
  comment's sender is a `User`, is not the pull request's author, and has
  `write` or `admin` on this repository, so neither triage nor ClaxBot's own
  token, which can comment, is enough. It also refuses a closed pull request,
  a fork's, a release's, Dependabot's and one that deploys by itself, and any
  API error. It asks GitHub's compare API what the head changes from the
  default branch and refuses, with a comment, any path
  `deploy/claxbot-workspace/clax.toml` refuses.
- **The build runs on the branch, not in the comment's run.** A run's ref
  sets its Actions cache scope, and a comment's run has the default branch's:
  a build there would run the branch's code where it could poison what that
  scope's runs restore (`native-check.yml`'s pnpm store; nothing that ships
  restores a cache). So `approve` dispatches `preview.yml` on the head branch, with the
  pull request and the SHA as inputs. GitHub names `workflow_dispatch` and
  `repository_dispatch` as the events made with `GITHUB_TOKEN` that start a run.
- **Only `approve` can start a deploy.** The jobs `dispatched`, `build` and
  `deploy` each require `github.triggering_actor` to be `github-actions[bot]`, which no
  person is: the only other jobs with `actions: write`, in `deploy-prod.yml`,
  run from `prod` and dispatch other workflows,
  and ClaxBot's App has no Actions permission. `dispatched` matches the inputs
  against strict patterns, and requires the SHA to be the run's own commit and
  the current head of an open pull request of this branch, so a push landing
  in between refuses it. `deploy` uses only what that job outputs.
- **To deploy again, comment again.** A dispatched run's rerun, whole or of
  its failed jobs, deploys nothing: its triggering actor is then a person.
  Rerunning the comment's run checks everything anew and dispatches again.
- **`approve` is the default branch's; the rest is the branch's.** For
  `issue_comment` GitHub runs `preview.yml` from the default branch, so no
  branch can change `approve`, and the command works once it is on `main`.
  The `pull_request` path and the dispatched run take the file from the
  branch, as the paragraph before says, so a branch that could edit it could
  deploy itself. ClaxBot's cannot: `.github/` is a protected path of every
  target, and its App has no `workflows` permission, so GitHub refuses such a
  push as well ([deploy/claxbot-workspace/README.md](../deploy/claxbot-workspace/README.md)).
  A branch older than this gate has no `workflow_dispatch` trigger, and
  `approve` fails on it until it is rebased.
- **A refused command disturbs nothing.** Only an accepted dispatch's deploy
  joins the pull request's concurrency group, so neither a refusal, any other
  comment, nor a push to the branch cancels a deploy or a teardown. A comment's
  run and a dispatched one show under Actions and not on the pull request: a
  refusal is a failure there, and a success the preview's comment, which names
  the commit.
- **`CLAXBOT_LOGIN` is ClaxBot's App's login** (`<app-slug>[bot]`). The branch
  prefix alone would miss a renamed branch, and every App's pull requests
  cannot be gated, since the release carry's is an App's too
  (`scripts/carry-release.sh`) and keeps its preview. **While the variable is
  unset, a renamed ClaxBot branch drops the gate:** it takes the ordinary
  path, and deploys on every push.
- **Everything else deploys as before.** On a `pull_request` event no job is
  added. Someone who re-branches a ClaxBot branch under another name and opens
  the pull request themselves takes responsibility for it: it is theirs, and
  deploys on every push.
- **Teardown does not wait for an approval.** Closing a ClaxBot pull request
  that never deployed still runs `teardown`, which holds the `preview`
  secrets, but it runs only code from the default branch: it checks that out,
  never the closed branch or its base, which a stacked pull request may have
  on a `claxbot/` branch. A deploy that joins the group after a close can
  cancel a teardown; a dispatched deploy checks for that before it builds,
  and tears down in its place.
- **An accepted risk.** The command is as good as the login that posts it: an
  agent holding a writer's `gh` login can post `/preview` as that person,
  and a person reviewing with an agent should keep that in mind.

ClaxBot waits for a preview for its target's `preview_timeout_secs`, a day on
Clax's, which sits in no run. The queue's notifier tells its people at the
proposal what to post. The reporter hears nothing until the preview answers; if
the wait ends first, the queue's members get a note saying so, and an approval
that comes later deploys a preview nobody is told of.

**So dev and the previews run on a Cloudflare account of their own**, with
their zones on it, apart from clax.dev's: neither token can reach prod's
Worker `clax`, its Durable Objects or its bucket `clax-assets`, whatever a
branch's run does with it. The `preview` environment names that account
in its secret `CLOUDFLARE_ACCOUNT_ID`. The account needs Workers Paid
(Durable Objects and the Worker Loader) and the Artifacts beta.

dev and the previews need the same permissions, so one token, limited to
that account and to its zones, serves both, in the `preview` environment.
So does dev's smoke token, `CLAX_DEV_SMOKE_TOKEN`: a writer's branch can
act as an agent on dev, and redeploy over `clax-dev`, as it could with
the Cloudflare token alone.

| Scope | Permission | For |
|---|---|---|
| Account | Workers Scripts: Edit | deploys, and teardown's deletes |
| Account | Workers R2 Storage: Edit | the buckets, and the preview bucket's lifecycle rule |
| Account | Queues: Edit | each environment's queue, its consumer, and teardown's deletes |
| Account | Artifacts: Edit | each environment's namespace, and teardown's repository deletes |
| Account | Account Settings: Read | the login check |
| Zone (public host's) | Zone: Read | the custom domain |
| Zone (site domain's) | Workers Routes: Edit, Zone: Read | the site route |
| Zone (site domain's) | DNS: Edit | the `*.pr-<N>` and `*.dev` records |
| User | User Details: Read, Memberships: Read | the login check |

The same caveats as prod's token apply ([docs/prod.md](prod.md), "The
Cloudflare API token"): some permission names are not confirmed by
Cloudflare's docs.

## One-time setup

### Cloudflare

1. Make the account dev and the previews use (not clax.dev's), on Workers
   Paid, with R2 turned on (Storage & databases, R2; until it is, every
   deploy fails making its bucket with Cloudflare's code 10042) and the
   Artifacts beta turned on (Storage & databases, Artifacts).
   `just wrangler login`, then `just wrangler whoami` lists the accounts the login sees, with their IDs: the new one's is
   the `preview` environment's `CLOUDFLARE_ACCOUNT_ID`.
2. On it, add the zones `PREVIEW_HOST_ZONE` and `PREVIEW_SITE_ZONE`.
3. Turn on Advanced Certificate Manager and Total TLS on
   `PREVIEW_SITE_ZONE`: the sites `<ID>.pr-<N>.PREVIEW_SITE_ZONE` and
   `<ID>.dev.PREVIEW_SITE_ZONE` are two levels below it, past what
   Universal SSL covers.
4. Make the token above.

### Previews

1. Create the preview OAuth App (above), wildcard matching on.
2. Set up the `preview` GitHub environment (deployable from any branch)
   and its variables, then its secrets as the script prints them (dev's
   smoke token waits for dev, below):

   ```
   scripts/setup-env-github.sh --env preview --preview-host-zone <zone> --preview-site-zone <zone> \
     --github-client-id <ID> --owner-github <login> --cloudflare-account-id <ID>             # dry run
   … --i-know-what-i-am-doing                                                               # act
   ```

3. Name ClaxBot's App, for the gate on its pull requests ("Credentials"):

   ```
   gh variable set CLAXBOT_LOGIN --body '<app-slug>[bot]'
   ```

The next push to a pull request deploys its preview.

### The ClaxBot gate's first run

`preview.yml`'s gate has been checked against a model of GitHub, not GitHub.
Its first run on a real ClaxBot pull request should confirm, and whatever fails
fails closed unless marked:

1. A dispatch made with `GITHUB_TOKEN` has `github.triggering_actor`
   `github-actions[bot]`. GitHub's docs say such a dispatch starts a run but
   name no actor for it; if it is another name, `dispatched` refuses every
   deploy, and both tests in `preview.yml` need that name.
2. The dispatched run's ref is the head branch and its caches are that
   branch's, not the default branch's (open, not closed, if wrong).
3. `approve`'s `pull-requests: write` and `contents: read` are enough for `GET
   collaborators/<login>/permission`, `GET pulls/<N>`, `GET compare/…`, `GET
   contents/…` and a comment, and `actions: write` for the dispatch, sent with
   `gh api -f 'inputs[pr]=…'`.
4. A comment by a person has `sender.type` `User`, and one from the App's
   token `Bot`.
5. `deploy` starts when the job it needs was skipped, at once, on an ordinary
   pull request (a preview would be missing, not unsafe).
6. Job-level `concurrency` reads `needs` and `vars`, and a close's teardown
   cancels a running deploy.
7. The `preview` environment gives the dispatched run its secrets.
8. A person's rerun of a dispatched run has that person as
   `github.triggering_actor`, so it deploys nothing (open, not closed, if
   wrong: it would redeploy the approved commit).
9. A close during a dispatched deploy tears the preview down, and the
   preview's comment lands on the pull request from a dispatched run.
10. A person's Run workflow button on `preview.yml` deploys nothing: `dispatched`
    refuses it (open, not closed, if wrong).
11. `CLAXBOT_LOGIN` is set: while it is unset, a renamed ClaxBot branch drops
    the gate (open, not closed).

### dev

1. Make dev's `CLAX_KEK` (`openssl rand -hex 32`) where its backup is
   kept, its only copy outside the Worker ([docs/prod.md](prod.md),
   "CLAX_KEK").
2. Deploy dev once from a laptop, which makes its secrets and uploads that
   key, as prod's first deploy did, with the preview OAuth App's client
   secret:

   ```
   read -rs CLAX_KEK && export CLAX_KEK   # paste dev's key
   GITHUB_CLIENT_SECRET=… just env-deploy dev --public dev.<PREVIEW_HOST_ZONE> \
     --site-domain dev.<PREVIEW_SITE_ZONE> --site-zone <PREVIEW_SITE_ZONE> \
     --github-client-id <ID> --owner-github <login> --account <ID> --i-know-what-i-am-doing
   ```

   The settings are kept in `~/.config/clax/deploy/setup.dev.env`, and
   later runs need no flags.
3. Sign in to dev as the owner, make an agent token named `smoke`
   (settings, Agent tokens), and set it as the `preview` environment's
   `CLAX_DEV_SMOKE_TOKEN`.

Every green push to `main` deploys dev from then on. Until
`CLAX_DEV_SMOKE_TOKEN` is set, `deploy-dev.yml` deploys without the smoke
and warns.

## By hand

```
just env-deploy pr-7 --public pr-7.<zone> --site-domain pr-7.<site zone> --site-zone <site zone> …   # a dry run
just env-teardown pr-7 --site-zone <site zone> --dry-run                                            # CLOUDFLARE_API_TOKEN, CLOUDFLARE_ACCOUNT_ID
```

`just env-deploy` refuses `prod` (that is `just clax-dev-deploy`), and
`teardown.mjs` refuses `dev` and `prod`.
