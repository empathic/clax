# Prod: releases and deploys from one branch

What clax.dev runs and what users download is one known commit: the tip of
the branch `prod`. Every change to `prod` is a release that arrives by pull
request, and every push to `prod` deploys clax.dev from GitHub Actions.
The design, and the calls it made, are in
[`docs/superpowers/specs/2026-10-27-prod-releases-design.md`](superpowers/specs/2026-10-27-prod-releases-design.md);
cutting a release is `docs/usage.md`, "Releasing".

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
  the variables the deploy reads: `PUBLIC_HOST` (`clax.dev`), `SITE_DOMAIN`
  (`clax.run`), `STORE` (`artifacts`), `OAUTH_CLIENT_ID` (the GitHub OAuth
  App's client ID; GitHub refuses variable names starting with `GITHUB_`),
  `OWNER_GITHUB` (`akesling`), `CLOUDFLARE_ACCOUNT_ID`, and
  `RETIRED_CONTENT_ORIGIN` when given `--retired-content-origin`. Each has
  a flag (`--help`). Give `--retired-content-origin` when your local
  deploy record (`~/.config/clax/deploy/clax.dev.json`) names a
  `retired_content_origin`: the workflow starts from an empty record and
  would otherwise stop serving the old content host;
- sets the repository variable `PUBLIC_HOST` to the same host, for the
  drift check, which runs outside the environment;
- makes the environment `npm` deployable from `prod` in place of `main`;
- prints the two commands that set the secrets, which it never runs:

```
gh secret set CLOUDFLARE_API_TOKEN --env production --repo empathic/clax
gh secret set CLAX_SMOKE_TOKEN --env production --repo empathic/clax
```

### The Cloudflare API token

Make it in the Cloudflare dashboard (My Profile, API Tokens, Create Token,
Custom token), limited to the account `00000000…` and to the zones
`clax.dev` and `clax.run` (and the retired content host's zone, if there
is one). A full deploy re-applies every binding, the custom domain and the
route, and makes what is missing, so it needs:

| Scope | Permission | For |
| --- | --- | --- |
| Account | Workers Scripts: Edit | uploading the Worker, its Durable Objects, Worker Loader and secrets (`wrangler secret list`); attaching `clax.dev` as a custom domain (`PUT /accounts/{account}/workers/domains` accepts Workers Scripts Write) |
| Account | Workers R2 Storage: Edit | `wrangler r2 bucket create clax-assets` and the `ASSETS` binding |
| Account | Queues: Edit | `wrangler queues create clax-artifacts-events`, the Worker's consumer, and the smoke's `wrangler queues subscription create/delete` |
| Account | Artifacts (Edit) | the `clax` namespace: deploy.mjs lists namespaces and creates and deletes a probe repository |
| Account | Account Settings: Read | wrangler reading the account |
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

## Drift

Every hour the same workflow's `drift` job compares the `commit` that
`https://$PUBLIC_HOST/healthz` reports (the repository variable the setup
script sets; `clax.dev` when it is unset) with `prod`'s tip, and fails when they differ, unless
a Deploy prod run of the tip is still running or waiting. It holds no
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
