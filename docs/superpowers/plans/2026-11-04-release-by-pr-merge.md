# Releases by pull request merge: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `just release X.Y.Z` cuts the release pull request into `prod`; while it is open, it is staging (`pr-<N>` on prod's account, the attested candidate build); merging it, once the owner approves, deploys clax.dev from those same files, publishes the release, carries it to `main` and publishes the Pi package.

**Architecture:** Three pull requests to `main`, each shippable alone. PR 1 (cut and guards) changes only scripts and repository setup; the post-merge cut of `scripts/release.sh` keeps working until PR 3 replaces it. PR 2 adds `staging.yml` and the deploy tooling for staging instances. PR 3 rewrites `deploy-prod.yml` to promote the candidate and do the rest, and removes the local cut.

**Tech Stack:** bash scripts tested against fake `gh`/`git` executables (`scripts/test-*.sh`, `sh_test` in `scripts/BUILD.bazel`), GitHub Actions YAML, Node (`crates/clax-worker/deploy*.mjs`, tested with `node --test`), Bazel.

**Spec:** [`docs/superpowers/specs/2026-11-04-release-by-pr-merge-design.md`](../specs/2026-11-04-release-by-pr-merge-design.md)

## Global Constraints

- Staging: Worker `clax-pr-<N>`, host `pr-<N>.clax-staging.dev`, sites `<ID>.pr-<N>.clax-staging.run`, on prod's account `c450cf57b1b5a1e0a14462aeadb4e2bc`.
- Environments: `production` (deploys from `prod`; required reviewer the owner), `staging` (deploys from `release/*` and `main` only), `release` (deploys from `prod` only; the release App's secrets), `npm` (`prod` and `main`).
- Ruleset `release` on `refs/heads/release/*`: creation, update and deletion restricted; bypass the repository admin role (`RepositoryRole` 5, `always`), as `prod`'s.
- `production` secrets: `CLOUDFLARE_API_TOKEN`, `CLAX_SMOKE_TOKEN`, `CLOUDFLARE_ACCOUNT_ID`, `OAUTH_CLIENT_ID`, `OWNER_GITHUB`; variables `PUBLIC_HOST`, `SITE_DOMAIN`, `STORE`, `RETIRED_CONTENT_ORIGIN`.
- `staging` secrets: `CLOUDFLARE_API_TOKEN`, `CLOUDFLARE_ACCOUNT_ID`, `OAUTH_CLIENT_ID`, `OAUTH_CLIENT_SECRET`, `OWNER_GITHUB`; variables `STAGING_HOST_ZONE=clax-staging.dev`, `STAGING_SITE_ZONE=clax-staging.run`.
- `release` secrets: `RELEASE_APP_ID`, `RELEASE_APP_PRIVATE_KEY`.
- No secret value is printed, logged, or put on a command line; scripts print `gh secret set` commands and never run them.
- Agents commit with `git -c commit.gpgsign=false commit`, conventional subjects (`type(scope): summary`), no attribution lines; the owner re-signs.
- Bazel only in worktrees (`just test-affected`, `bazelisk test //scripts:<name>`); never `--nocache_test_results`.

## Review Focus

- **A second `just release` with the same version while its pull request is open:** prints the pull request's URL and changes nothing (today: refuses naming the URL). Test in Task 2.
- **A pull request into `prod` from a branch not named `release/*`, or from a fork:** CI's `version` job fails naming the rule. Test in Task 4 (the script it calls).
- **`setup-prod-github.sh` re-run over environments that exist with other settings** (a `staging` environment admitting every branch, as the owner made by hand on 2026-11-04): it narrows them to the spec's, never widens. Test in Task 3.
- **The merge commit's tree differs from the tested head's** (`prod` moved while the pull request was open): Deploy prod refuses before downloading anything. Test in Task 9.
- **An artifact in the candidate run without a matching attestation, or attested by another workflow or commit:** refused, nothing deployed. Test in Task 9.

---

## PR 1: cut and guards

### Task 1: `production`'s IDs and owner login as secrets

Already made on this branch (`prod-ids-as-secrets`): `.github/workflows/deploy-prod.yml` reads `OAUTH_CLIENT_ID`, `OWNER_GITHUB`, `CLOUDFLARE_ACCOUNT_ID` from `secrets.*`; `scripts/setup-prod-github.sh` stops writing them as variables, removes variables of those names, prints their `gh secret set … --body` commands; `docs/prod.md`; `scripts/test-setup-prod-github.sh`.

- [ ] **Step 1: Run** `bazelisk test //scripts:setup_prod_github`; expected PASS.
- [ ] **Step 2: Commit** `feat(deploy): production's account ID, client ID and owner login are secrets`.

### Task 2: the prepare runs no local CI

**Files:**
- Modify: `scripts/release.sh` (header steps 6 and 15, `STEP_NAME`/weights at ~611-620, `step_6`, `step_15`, `check_prepare`'s open-PR case, `pr_body`)
- Test: `scripts/test-release-script.sh`

**Interfaces:**
- Produces: step 6 named "Check the version files", running `scripts/check-version.sh` (exit 0 when every version file agrees); step 15 the same, until PR 3 removes it.

- [ ] **Step 1: Change the tests first.** In `case_2` the prepare's call list is `bump 0.6.0`, three `just bazel-sync`, `check-version`, `gh pr create`: no `just ci`. `has "[2%] step 6/21 Check the version files"`. The "resume after a failure in just ci" cases (~607-628) become a failing `check-version.sh` (a version file edited to disagree): it stops at step 6 before the release commit, names the step and the resume command. A run whose pull request is open prints `the pull request: <url>` and exits 0 having changed nothing (replacing the refusal). The heartbeat case at ~1284-1299 uses step 13's wait instead of `just ci`.
- [ ] **Step 2: Run** `bazelisk test //scripts:release_script`; expected FAIL on those cases.
- [ ] **Step 3: Implement** `step_6() { (cd "$ROOT" && logged bash scripts/check-version.sh); }`, `step_15() { step_6; }`; rename the steps; set their weights to seconds, not minutes; the open pull request case in `check_prepare` becomes a `tell` of its URL and a clean exit before step 2; `pr_body` says CI runs every gate on the pull request.
- [ ] **Step 4: Run** `bazelisk test //scripts:release_script`; expected PASS.
- [ ] **Step 5: Commit** `feat(release): just release cuts the pull request without running CI locally`.

### Task 3: repository setup for release branches, staging and approval

**Files:**
- Modify: `scripts/setup-prod-github.sh`
- Test: `scripts/test-setup-prod-github.sh` (its fake `gh`: rulesets by name, environments with `reviewers`, deployment branch policies with `type: branch` patterns)

**Interfaces:**
- Consumes: the existing `get`, `write`, `set_var`, `branch_policy ENV BRANCH...` helpers.
- Produces: new flags `--staging-host-zone` (default `clax-staging.dev`), `--staging-site-zone` (default `clax-staging.run`); steps, in order after today's: the ruleset `release` on `refs/heads/release/*`; `production`'s required reviewer (the `OWNER_GITHUB` login's user ID, `gh api users/<login> --jq .id`); environment `staging` with branch policies exactly `release/*` and `main` and variables `STAGING_HOST_ZONE`, `STAGING_SITE_ZONE`; environment `release` with branch policy exactly `prod`; then the printed `gh secret set` lines for `staging` and `release` (Global Constraints).

- [ ] **Step 1: Write the failing tests**:
  - acting on a fresh repository makes the `release` ruleset with rules `creation`, `update`, `deletion` on `refs/heads/release/*` and bypass `[{actor_id: 5, actor_type: RepositoryRole, bypass_mode: always}]`;
  - `production`'s `reviewers` is `[{type: User, id: <octo-owner's id from the fake>}]`;
  - `staging` deploys from `['main', 'release/*']` only and has the two zone variables;
  - `release` deploys from `['prod']` only;
  - a `staging` that admitted every branch (`deployment_branch_policy: null`) is narrowed, and one with an extra branch policy loses it;
  - the dry run prints `gh secret set OAUTH_CLIENT_SECRET --env staging --repo empathic/clax` and `gh secret set RELEASE_APP_PRIVATE_KEY --env release --repo empathic/clax`;
  - a second run writes nothing but the two rulesets.
- [ ] **Step 2: Run** `bazelisk test //scripts:setup_prod_github`; expected FAIL.
- [ ] **Step 3: Implement** with the existing helpers; the ruleset body is written whole each run, as `prod`'s. The `production` reviewer is written with `PUT repos/$repo/environments/production` and `{"reviewers":[{"type":"User","id":N}],"deployment_branch_policy":{"protected_branches":false,"custom_branch_policies":true}}`.
- [ ] **Step 4: Run** `bazelisk test //scripts:setup_prod_github`; expected PASS.
- [ ] **Step 5: Commit** `feat(release): setup-prod-github makes the release-branch ruleset, staging and release environments, and the prod approval`.

### Task 4: a pull request into `prod` comes from a release branch

**Files:**
- Create: `scripts/check-release-head.sh` (`check-release-head.sh <head ref> <head repository> <repository>`: exit 0 when the head ref matches `release/X.Y.Z` and the head repository is the repository; else 1 with one line on stderr)
- Create: `scripts/test-check-release-head.sh`; `sh_test` `check_release_head` in `scripts/BUILD.bazel`
- Modify: `.github/workflows/ci.yml` (`version` job: a step before the bump check, `scripts/check-release-head.sh "$HEAD_REF" "$HEAD_REPO" "$GITHUB_REPOSITORY"` with `HEAD_REF: ${{ github.head_ref }}`, `HEAD_REPO: ${{ github.event.pull_request.head.repo.full_name }}`)

- [ ] **Step 1: Write the failing test**: accepts `release/0.7.0 empathic/clax empathic/clax`; refuses `main …` ("is not a release branch, release/X.Y.Z"), `release/next …`, `release/0.7.0 someone/clax empathic/clax` ("comes from a fork").
- [ ] **Step 2: Run** `bazelisk test //scripts:check_release_head`; expected FAIL (no script).
- [ ] **Step 3: Implement** the script.
- [ ] **Step 4: Run** it again; expected PASS.
- [ ] **Step 5: Commit** `feat(ci): a pull request into prod must come from a release branch of this repository`.

### Task 5: docs for PR 1

**Files:** `docs/usage.md` ("Releasing": step 6 is the version check, CI runs the gates on the pull request; an open pull request's URL is printed), `docs/prod.md` ("One-time GitHub setup": the ruleset, the environments, the approval, the new secrets; the staging Cloudflare token's permissions: prod's table with Stream: Read, plus DNS: Edit on the site zone, zones `clax-staging.*`).

- [ ] **Step 1: Write them**; check every claim against Tasks 2-4's scripts.
- [ ] **Step 2: Run** `bazelisk test //scripts/... //docs/...`; expected PASS (`check_copy` reads the docs).
- [ ] **Step 3: Commit** `docs(release): cutting a release and the staging and release setup`.

Before the PR: `just check`; `qualifier diff main`.

## PR 2: staging

### Task 6: the deploy knows staging instances

**Files:** `crates/clax-worker/deploy-lib.mjs` (`envOf`), `crates/clax-worker/deploy.mjs` (flags), `crates/clax-worker/deploy-lib.test.mjs`, `crates/clax-worker/test-deploy-config.mjs`.

**Interfaces:**
- Produces: `--env pr-<N> --staging` (with `--production` settings otherwise: legal bundled, the reel, the smoke) deploys `clax-pr-<N>` as an ephemeral instance at `--public pr-<N>.<STAGING_HOST_ZONE> --site-domain pr-<N>.<STAGING_SITE_ZONE> --site-zone <STAGING_SITE_ZONE>`; `envOf("pr-12")` stays `{ name: "pr-12", ephemeral: true }`; `--staging` refuses without `--env pr-<N>` and refuses together with `--production`.
- Produces: `smokeTokenOf(record)`: on a fresh ephemeral instance with `--smoke` and no `CLAX_SMOKE_TOKEN`, the deploy mints an agent token named `smoke` with the record's owner token (`POST /api/tokens` on the instance) and uses it for the smoke; the owner token is never printed.

- [ ] Tests first (`node --test`), then implementation, then `bazelisk test //crates/clax-worker:deploy_lib_test //crates/clax-worker:deploy_config_test`; commit `feat(deploy): staging instances on prod's account, smoked as a minted agent token`.

### Task 7: `staging.yml`, the attested candidate and the staging deploy

**Files:** create `.github/workflows/staging.yml`; modify `.github/workflows/release.yml` (`on: workflow_call` with input `sha`; the build and assemble jobs attest each file with `actions/attest-build-provenance`, `permissions: { id-token: write, attestations: write, contents: read }`; no publish when called); the Worker's build as a job of `staging.yml` with no environment, uploading `target/clax-worker/build` and `built-from.json` as artifact `worker`, attested.

**Interfaces:**
- Produces: a `staging.yml` run on `push` to `release/*` with artifacts `release-dist` (binaries, `SHA256SUMS`, `SOURCE_TREE`) and `worker`, each file attested by `.github/workflows/release.yml` or `staging.yml` at `github.sha`; a `deploy` job in environment `staging` that resolves `N` (`gh pr list --head <branch> --base prod --state open --json number`, retried for 120 s), deploys with Task 6's flags from the `worker` artifact (`--no-build`), and comments the staging URL on the pull request.

- [ ] Write `staging.yml`; check it with `actionlint` if installed, and by a run on a throwaway `release/0.0.0-test` branch pushed by the owner; commit `feat(release): staging deploys each release branch's attested candidate`.

### Task 8: no preview for release pull requests; the reaper reaps staging

**Files:** `.github/workflows/preview.yml` (`if:` adds `github.event.pull_request.base.ref != 'prod'`), `.github/workflows/preview-reaper.yml` (a second job in environment `staging`, on `main`: lists `clax-pr-*` Workers on the staging account whose pull request is closed and runs `teardown.mjs` with `--site-zone "$STAGING_SITE_ZONE"`).

- [ ] Write; run the reaper by hand on `main` after merge; commit `feat(release): release pull requests get staging instead of a preview, reaped when closed`.

## PR 3: release on merge

### Task 9: find and verify the candidate

**Files:** create `scripts/find-candidate.sh` (`find-candidate.sh <merge commit>`: prints the head commit and the `staging.yml` run ID; exit 1 naming the reason when the commit has no second parent, its tree differs from the second parent's, or no successful `staging.yml` run exists for that head) and `scripts/verify-candidate.sh <dir> <head commit>` (`gh attestation verify` each file with `--repo empathic/clax --source-digest <head commit>` and `--signer-workflow` of `release.yml` or `staging.yml`; exit 1 naming the first file that fails); tests `scripts/test-find-candidate.sh`, `scripts/test-verify-candidate.sh` against fake `git`/`gh`; `sh_test`s.

- [ ] Tests first (the two Review Focus cases among them), implement, `bazelisk test //scripts:find_candidate //scripts:verify_candidate`; commit `feat(release): deploy prod finds and verifies the tested candidate`.

### Task 10: commits compared by tree

**Files:** `crates/clax-worker/deploy-lib.mjs` (`builtFromRefusal`, `productionGate` take trees), `crates/clax-worker/deploy.mjs` (passes `git rev-parse <commit>^{tree}`), `.github/workflows/deploy-prod.yml` drift job (compares `git rev-parse <healthz commit>^{tree}` with `prod`'s tip's tree, fetching the commit), tests in `deploy-lib.test.mjs`.

- [ ] Tests first: a Worker built from the head commit deploys from a checkout of the merge commit with the same tree; one built from another tree is refused as today. Implement; `bazelisk test //crates/clax-worker:deploy_lib_test`; commit `fix(deploy): a build is the merged commit's when their trees are the same`.

### Task 11: `deploy-prod.yml` promotes and releases

**Files:** `.github/workflows/deploy-prod.yml`, `.github/workflows/publish-pi.yml` (runs from `main`).

**Interfaces:**
- Consumes: Task 9's scripts; Task 10's checks; `scripts/pin-release.sh vX.Y.Z` with `CLAX_RELEASE_BASE_URL` pointed at the downloaded `release-dist`.
- Produces jobs, in order: `candidate` (no environment: find, download, verify; outputs the head commit and run ID); `deploy` (environment `production`, so the owner's approval: deploy `--no-build` from the `worker` artifact, wait, smoke; the reel step as today); `publish` (`gh release create vX.Y.Z --target <merge commit>` with `release-dist`'s files); `carry` (environment `release`: an App token from `actions/create-github-app-token`; branch `carry/X.Y.Z` from `main` with the release commit and the pin commit made through the Git Data API, so GitHub signs them; a pull request into `main`; `gh pr merge --auto --rebase`); `pi` (after the carry merges: `publish-pi.yml` on that commit of `main`); `teardown` (`clax-pr-<N>`).

- [ ] Write; check with `actionlint`; the first `just release` is its integration test; commit `feat(release): merging the release pull request deploys, publishes and carries it`.

### Task 12: remove the local cut; the runbook

**Files:** `scripts/release.sh` (steps 11-21 removed: once `prod` is at the version it says the release runs on merge and points at the Deploy prod run), `scripts/test-release-script.sh`, `docs/usage.md` ("Releasing" becomes a runbook like pathbase's `docs/release.md`: cut, review staging, merge and approve, rollback), `docs/prod.md`.

- [ ] Tests first, implement, `bazelisk test //scripts:release_script`; `just check`; commit `feat(release): the release runs on merge; the local cut is gone`.
