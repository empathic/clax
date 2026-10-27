# Releases and deploys from a prod branch

## Intent

What runs on clax.dev and what users download should always be one known
commit: the tip of a `prod` branch. Today clax.dev is whatever the owner's
laptop last deployed with `just clax-dev-deploy`, and a versioned release is
cut from `main` with `just release`. After this change:

- `prod` is the only branch either kind of release comes from.
- Every change to `prod` is a release: it arrives by pull request, and CI
  refuses a pull request into `prod` that does not raise the version.
- A push to `prod` deploys clax.dev from GitHub Actions, never from a laptop.

## Goals

- `prod`'s tip is what clax.dev runs; a scheduled check says when it is not.
- A pull request into `prod` must pass CI and raise the version (semver,
  greater than `prod`'s), with every version file agreeing
  (`scripts/check-version.sh`).
- `just release X.Y.Z` cuts the versioned release (tag, GitHub release,
  plugin pins, Pi package) only from a `prod` checkout at `origin/prod`'s tip.
- `just clax-dev-deploy` refuses to deploy the production instance from a
  laptop unless given an explicit override; first-time setup of an
  instance stays local.
- The deploy workflow's smoke runs with an agent token, never the owner
  token; the owner token never goes to GitHub.

## Non-goals

- Changing what CI tests on `main`, or how `main` takes changes (signed,
  linear, no merge commits).
- Automatic rollback. A failed deploy or smoke fails the run and GitHub
  notifies the owner; the new version stays live until the owner acts.
- Moving the plugins' marketplaces off `main`.

## Anti-goals

- A plugin version reaching users before the release it pins exists.
- A deploy to clax.dev from any commit other than `prod`'s tip at the time
  the deploy runs.
- Credentials beyond what each job needs: the Cloudflare token scoped to the
  account's Workers, their routes and custom domains on the two zones, and
  the storage the Worker binds; the smoke an agent token.

## Constraint that shapes the flow

The Claude Code and Codex marketplaces install the plugins from `main`'s
files (`/plugin marketplace add empathic/clax`). A harness updates an
installed plugin only when its version changes. So a version bump must reach
`main` in the same push as the pin to its release (`scripts/pin-release.sh`
says so), or users update to the new version still running the old binary
and never update again until the next version. A bump can therefore not
land on `main` before its release: it lives on a release branch until the
release is published, then reaches `main` together with its pin.

## The flow

1. **Prepare** (`just release X.Y.Z`, from any branch, clean): cuts
   `release/X.Y.Z` from `origin/prod`, makes a signed merge of `origin/main`
   into it (`main` already carries every earlier release's version and pin,
   carried in step 4, so their version lines agree and merge cleanly), runs
   `scripts/bump-version.sh`, `just bazel-sync` and `just ci`, makes the
   signed commit "Release X.Y.Z", pushes the branch and opens a pull request
   into `prod`. Its merge base with `prod` is `prod`'s tip, so the pull
   request never conflicts on the version files. It stops there.
2. **Review and merge** (the owner, on GitHub): CI runs on the pull request,
   including the new version check. The owner merges it; merge commits are
   allowed on `prod`. The push to `prod` deploys clax.dev (below).
3. **Cut** (`just release X.Y.Z` again, now on a `prod` checkout at
   `origin/prod`'s tip, whose version is X.Y.Z): the remaining steps of
   today's `release.sh`, with `prod` in place of `main`. It signs the tag
   vX.Y.Z on `prod`'s tip and pushes the tag; waits for the Release workflow
   and checks the GitHub release; runs `scripts/pin-release.sh`, `just ci`
   and makes the signed commit "Pin the plugins to Clax X.Y.Z"; pushes
   `prod` (the owner may push `prod` directly; the pin push deploys again,
   with no server change).
4. **Carry to main** (same run, right after the pin push, so `main` is red
   as briefly as possible: `scripts/test-plugins.sh` fails on `main` while
   its pin is not the newest tag): cherry-picks the release commit and the
   pin commit onto `origin/main`'s tip as two signed commits and pushes
   `main` (which refuses merge commits: GitHub rejects them with GH013). A
   conflict stops the run, which says what to resolve. The marketplaces then
   see the new version and its pin in one push.
5. **Finish** (same run): waits for CI on the pin commit on `prod`, then
   publishes the Pi package from it.

Each step's state is read from git and GitHub, as today, so a run resumes at
the first step not done. The dry run (the default) changes nothing.

Every change reaches `prod` through a release branch made in step 1, so a
pull request straight from `main` (which cannot raise the version) is not a
path. A rollback is a release too: revert on `main`, cut a patch release.
Until it deploys, the drift check keeps failing, which is the alert wanted.

## GitHub setup (the owner's, with exact steps in the docs)

- **Branch `prod`**, created at the commit clax.dev runs now.
- **Ruleset on `prod`:** pull request required; required checks: CI's
  `bazel` and `worker` jobs (the `worker` job is the one that builds and
  tests what `prod` deploys) and the new version check; no deletion; no force push;
  signed commits required (GitHub's merge commits are signed by GitHub).
  The owner bypasses it, so that `release.sh` can push the pin commit
  directly.
- **Environment `production`:** deployment branches `prod` only; variables
  for the settings `deploy.mjs` takes (for clax.dev: public host `clax.dev`,
  site domain `clax.run`, store `artifacts`, legal `bundled`, the GitHub
  OAuth client ID and owner login, account `00000000…`, worker `clax`);
  secrets `CLOUDFLARE_API_TOKEN` (the permissions a full `wrangler deploy`
  needs, triggers included, since each deploy re-applies the custom domain
  and the `*.<site domain>/*` route; the docs list them exactly), `CLOUDFLARE_ACCOUNT_ID`,
  `CLAX_SMOKE_TOKEN` (an agent token the owner makes in clax.dev's
  settings, named `smoke`).
- **Environment `npm`:** deployment branches `prod` in place of `main`, so
  the Pi package publishes from `prod`.

## Workflows

- **`ci.yml`:** also runs on pushes to `prod`, and on pull requests into
  `prod` adds a `version` job: the head's version is semver, greater than
  the base's, and `scripts/check-version.sh` passes.
- **`deploy-prod.yml` (new):** on push to `prod` and by hand (redeploys
  `prod`'s tip). One deploy at a time; a running deploy is never cancelled
  (GitHub may drop a queued one when a newer push queues, which the
  tip-only rule wants anyway). Refuses unless the
  checked-out commit is `origin/prod`'s tip. Builds the Worker, runs
  `deploy.mjs` in CI mode, waits for `/healthz` to report the build, checks
  a site host, runs the smoke with `CLAX_SMOKE_TOKEN`, and writes the
  commit, build and Cloudflare version to the job summary. A failure fails
  the run.
- **Scheduled drift check (in `deploy-prod.yml`, hourly):** compares
  `/healthz`'s `commit` with `prod`'s tip and fails, which notifies the
  owner, when they differ. It deploys nothing.
- **`publish-pi.yml`:** publishes from a commit on `prod` in place of
  `main`.

## Code changes

- **`deploy.mjs` CI mode:** no prompts; settings from flags and environment
  variables (public host, site domain, store, GitHub client ID, owner
  login, legal, and the retired content origin when an instance has one,
  which today's deploy reads from the record and must not silently drop);
  keeps its deploy record in the runner's temporary directory
  (`CLAX_DEPLOY_HOME`), never uploaded; prints no first-time setup text
  on a redeploy. Secrets already on the Worker stay as they are (it keeps
  them today); with no `CLAX_KEK` backup it says so without failing.
- **Pin to `prod`:** `deploy.mjs` refuses a deploy to the production
  instance unless running in GitHub Actions at `origin/prod`'s tip, or given
  an explicit override flag. Which instance is production is a setting
  (`setup.env`, the workflow's environment), not a hostname baked into the
  code.
- **`smoke-remote.mjs`:** runs as an agent token given by
  `CLAX_SMOKE_TOKEN` (or a flag): every agent step, skipping the
  owner-only ones and naming them. With no credential it fails as now.
- **`release.sh`:** the prepare and cut phases above, with `prod` in place
  of `main` in its preflight and pushes, and the carry to `main`. Its tests
  (`//scripts:release_script`) follow.
- **Docs:** `docs/usage.md` ("Releasing"), `crates/clax-worker/README.md`
  (deploying), and the GitHub setup steps.

## Calls made without the owner (check these)

- The bump lives on a release branch, not `main`, because of the
  marketplace constraint above.
- The owner still signs the tag and the pin commit with their own key
  (`release.sh` from their machine); only the deploy runs in Actions.
- The pin commit is pushed straight to `prod` by the owner's bypass, not
  through a second pull request.
- Copies reach `main` by cherry-pick, since `main` refuses merge commits.
- The drift check only reports; it never redeploys.
- The smoke's agent token is made by the owner once and stored as a
  GitHub secret.
