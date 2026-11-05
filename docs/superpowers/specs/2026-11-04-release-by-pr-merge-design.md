# Releases by pull request merge, with staging on the pull request

## Intent

A release is a pull request into `prod`. While it is open, it is staging:
the pull request's own instance, `pr-<N>`, deployed as clax.dev is
configured. Merging it does everything else in GitHub Actions: deploys
clax.dev, tags and publishes the release, carries it to `main`, publishes
the packages, and tears staging down. The only local step is cutting the
pull request, `just release X.Y.Z`.

This is the flow pathbase and changelist already use (pathbase's
`docs/release.md` and `.github/workflows/{preview,deploy-prod}.yml`;
changelist's `.github/workflows/{preview,deploy-prod}.yml`), adapted to
what Clax adds: a semver version, and plugins that download a pinned
release. It replaces the flow of
[`2026-10-27-prod-releases-design`](2026-10-27-prod-releases-design.md)
(its steps 1–5 and `scripts/release.sh`'s local prepare and cut). Its
goals that remain: `prod`'s tip is what clax.dev runs, a pull request into
`prod` must raise the version, deploys come only from Actions, the smoke
runs as an agent token, and the drift check.

## Goals

- **Cutting is one command:** `just release X.Y.Z` opens the release pull
  request, and runs nothing else locally.
- **Staging is the pull request:** every push to a pull request into `prod`
  deploys `pr-<N>` with prod's configuration, on prod's Cloudflare
  account, starting empty.
- **Releasing is one action:** merging. The deploy waits for the owner's
  approval on the `production` environment, then everything else runs.
- **The marketplaces' constraint holds:** the version and its pin reach
  `main` in one push, after the release they pin exists (2026-10-27,
  "Constraint that shapes the flow").

## Non-goals

- Copying prod's data into staging: staging starts empty for each release
  pull request (owner's call, 2026-11-04). pathbase and changelist fork
  prod's database; Clax has no database branch to fork.
- Automatic rollback (as 2026-10-27).
- Changing how `main` takes changes: signed, linear, by pull request.

## Anti-goals

- A plugin version reaching users before the release it pins exists.
- A deploy to clax.dev of anything but `prod`'s tip.
- Shipping anything but what staging tested: no build on merge (owner's
  call, 2026-11-04: a second build is untested).
- Shipping an artifact the pull request's credentials could have replaced
  (changelist's reason for building on merge, answered here by
  attestation instead).

## The flow

1. **Cut:** `just release X.Y.Z`, as pathbase's `just cut-release`: a dry
   run by default, acting with `--i-know-what-i-am-doing`. It keeps
   `scripts/release.sh`'s prepare as it is, less `just ci`: refuses a
   version that is not `X.Y.Z` newer than every `v*` tag; makes
   `release/X.Y.Z` from `origin/prod` with a signed merge of `origin/main`;
   runs `scripts/bump-version.sh` and `just bazel-sync` (a bump rewrites
   files Bazel derives: `bazel/workspace.bzl`, `bazel/wasm/Cargo.lock`,
   `MODULE.bazel.lock`); commits "Release X.Y.Z", signed; pushes; and
   opens the pull request into `prod` with a checklist body (the staging
   URL, the commits shipping). It runs no build and no tests: CI does, on
   the pull request. `prod`'s history gets one merge commit per release,
   whose second parent is the release branch's tip, the tested head.
2. **Staging, on each push to the release branch:**
   - CI, as now, including the version check.
   - `staging.yml`, on the push to `release/*` ("Who runs code with the
     staging token", below), deploys `pr-<N>` from the candidate Worker
     (below, built once) with prod's configuration: prod's account,
     production mode
     (`--production`: the reel, the legal pages, the smoke), at the
     staging zones (below).
   - **build**, jobs with no environment and no secrets: the Worker and
     the release binaries with `SHA256SUMS` (`release.yml`'s build and
     assemble jobs), each file attested (`actions/attest-build-provenance`:
     its digest signed, through the job's OIDC token, as made by this
     repository's `release.yml` at the pull request's head commit). These
     are the release candidate: staging deploys this Worker, the plugins
     are tried against staging with these binaries (`CLAX_BIN`), and the
     merge ships these same files.
3. **Merge** (the owner, with a merge commit). `deploy-prod.yml` on the push
   to `prod`:
   - **find the candidate:** the merge commit's second parent is the
     tested head. It refuses unless the merge commit's tree is the head's
     (nothing else entered with the merge); finds the latest successful
     `staging.yml` run for that head (a push to its `release/*` branch);
     downloads
     its artifacts; and refuses any file `gh attestation verify` does not
     tie to this repository's `release.yml` at that head commit. It builds
     nothing.
   - **approve:** the `production` environment's required reviewer, the
     owner, approves the deploy job.
   - **deploy** clax.dev from the candidate Worker (`--no-build`, which
     already refuses a Worker built from another commit's tree), wait for
     `/healthz`, smoke.
   - **release:** tag `vX.Y.Z` on the merge commit and create the GitHub
     release from the candidate binaries and `SHA256SUMS`, the files
     staging's testers downloaded.
   - **carry to main**, as the release App: a pull request into `main`
     with the "Release X.Y.Z" commit and a new "Pin the plugins to Clax
     X.Y.Z" commit (`scripts/pin-release.sh`), with auto-merge on, gated by
     `main`'s CI. The marketplaces see the version and its pin in one push,
     after the release exists.
   - **packages:** once the carry merges, Publish Pi package from that
     commit on `main` (its wrapper pins the release; the `npm` environment
     admits `main`).
   - **tear down** `pr-<N>`.

`prod` never holds the pin: the plugins install from `main`, and the Pi
package now publishes from `main`'s carry commit. So nothing pushes to
`prod` except the release pull request's merge.

## The pieces

### Staging

- **Names:** the Worker `clax-pr-<N>`, at `pr-<N>.<staging host zone>`,
  with its sites at `<ID>.pr-<N>.<staging site zone>`, the pull request's
  number as a preview's (owner's call, 2026-11-04). A pull request into
  `prod` gets staging in place of a preview, so one number names one
  instance.
- **Account:** prod's (owner's call, 2026-11-04): the same plan, features
  (Durable Objects, Artifacts, Worker Loader, R2, Queues, Stream) and
  limits as clax.dev, with nothing to keep in step. A Worker's custom
  domain and routes need their zone on the Worker's account, so the
  staging zones are on prod's account too.
- **Zones:** `clax-staging.dev` (hosts: `pr-<N>.clax-staging.dev`) and
  `clax-staging.run` (sites: `<ID>.pr-<N>.clax-staging.run`), registered
  for staging (owner, 2026-11-04). They mirror prod's `clax.dev` and
  `clax.run`: two registrable domains, so a site is never the same site as
  Clax, and neither is the same site as clax.dev, so staging's code makes
  no same-site request to prod. `clax-staging.run` needs Total TLS
  (Advanced Certificate Manager), as the previews' site zone does: a
  site host is two levels below it.
- **Credentials:** an environment `staging` with its own Cloudflare API
  token (scoped to prod's account and the two staging zones) and a staging
  GitHub OAuth App (callback `https://clax-staging.dev/auth/github/callback`,
  wildcard matching on, so every `pr-<N>` host matches).
- **Who runs code with the staging token.** Cloudflare scopes Workers
  Scripts: Edit to an account, not a script, so the staging token can
  change prod's Worker. A pull request's run uses the workflow file from
  the pull request, so a check in the YAML is no guard. The guards are
  GitHub's (owner's call, 2026-11-04):
  - a ruleset on `refs/heads/release/*`: only the repository admins
    create, update or delete those branches;
  - the `staging` environment deploys only from `release/*`;
  - CI refuses a pull request into `prod` whose head is not a
    `release/*` branch of this repository.

  So only admins, who can already merge into `prod`, ever run code with
  the token. GitHub matches an environment's deployment branch rule
  against the run's `GITHUB_REF`, which a `pull_request` run has as
  `refs/pull/<N>/merge`
  ([docs](https://docs.github.com/en/actions/reference/workflows-and-actions/deployments-and-environments)),
  so a `release/*` rule never admits a pull request's run. Staging
  therefore deploys on `push` to `release/*` (`GITHUB_REF` is
  `refs/heads/release/X.Y.Z`), in a workflow of its own (`staging.yml`),
  not in `preview.yml`. The run finds its number from the branch's open
  pull request into `prod`, waiting up to two minutes for it on the first
  push (`just release` opens it right after pushing); without one, it
  fails saying so. The `staging` environment admits `main` as well, for
  the reaper's teardown (below); `main` takes only signed, reviewed pull
  requests, so that admits no one new.
- **The smoke:** the deploy mints an agent token with the owner token it
  has just made, on the runner; the owner token never leaves the runner.
- **Teardown:** `crates/clax-worker/teardown.mjs` (it refuses `dev` and
  `prod`): after a merge, by `deploy-prod.yml`'s last step (its token is
  on the same account); after a close without merge, by
  `preview-reaper.yml`, which runs on `main` on its schedule and, in the
  `staging` environment, removes every `clax-pr-<N>` on prod's account
  whose pull request is closed. A `pull_request` run cannot do it: the
  `staging` environment does not admit `refs/pull/<N>/merge`. A pull
  request's preview, on the development account, is untouched by this.

### The release App

A GitHub App installed on empathic/clax only (Contents: write, Pull
requests: write), its ID and key secrets of an environment `release` that
only `deploy-prod.yml`'s carry job uses. A workflow's own token cannot do
the carry: pull requests and pushes it makes start no workflows, so
`main`'s CI would never run on the carry. The App is on no ruleset's bypass
list; its commits, made through GitHub's API, are signed by GitHub, which
`main`'s signed-commit rule accepts.

### Checks and scripts that change

- `scripts/release.sh`, `just release`: rewritten as step 1.
- `.github/workflows/staging.yml` (new): the candidate build and the
  staging deploy, on push to `release/*`.
- `.github/workflows/preview.yml`: no preview for a pull request whose
  base is `prod` (staging is its instance).
- `.github/workflows/preview-reaper.yml`: also reaps staging instances on
  prod's account.
- `.github/workflows/deploy-prod.yml`: the build job, the approval, the
  release, carry, packages and teardown steps.
- `.github/workflows/release.yml`: a reusable workflow `staging.yml` calls
  for the candidate build, with attestations (their signer is
  `release.yml` at the head commit); no publishing of its own
  (`deploy-prod.yml` publishes); its `SOURCE_TREE` names the tested head's
  tree, which is the merge commit's.
- `.github/workflows/publish-pi.yml`: publishes from `main`, not `prod`.
- `scripts/setup-prod-github.sh`: the `production` environment's required
  reviewer, the `staging` and `release` environments, and the repository's
  merge methods (merge commits on `prod`, as now).
- `docs/usage.md`, "Releasing", and `docs/prod.md`: the runbook, as
  pathbase's `docs/release.md`.

### One build for every instance

Reusing the Worker needs its build to be the same for staging and prod,
and it is: `scripts/build-web.sh` reads no instance setting, and
`crates/clax-worker/build.sh` takes only `CLAX_BUILD_COMMIT`, which
`/healthz` reports (`src/instance.rs`, `BUILD_COMMIT`). Hosts, the OAuth
client ID, production mode and the build ID are the wrangler config's
vars, written at deploy (`deploy.mjs`, `deployConfig`).

The one consequence: the Worker clax.dev runs reports the pull request's
head commit, not the merge commit on `prod`, though both have the same
tree. So the checks that compare commits compare trees:
`builtFromRefusal` (`--no-build`), `productionGate` (prod's tip), and the
hourly drift check, which accepts a reported commit whose tree is `prod`'s
tip's.

## The owner's setup

- `clax-staging.run` on prod's Cloudflare account, with Total TLS
  (`clax-staging.dev` is on Cloudflare already).
- The staging GitHub OAuth App, and the release GitHub App.
- The staging Cloudflare API token.

`scripts/setup-prod-github.sh` makes the rest: the `release/*` ruleset,
the `staging` and `release` environments, and the `production`
environment's required reviewer, and prints the `gh secret set` commands.

## Paths not taken

- **A staging account of its own.** It would keep a staging bug from ever
  touching prod's Worker, but every account-level difference from prod
  would go untested, and the `release/*` guards already limit the token to
  people who can ship to prod.
- **Staging under prod's zones** (`pr-<N>.clax.dev`): the same site as
  clax.dev to a browser.
- **Approval on each staging deploy, or staging only pull requests the
  owner opened:** a click per push, or a check the pull request's own
  workflow file could drop.
- **Building again on merge** (changelist's choice): what ships would not
  be what was tested.
