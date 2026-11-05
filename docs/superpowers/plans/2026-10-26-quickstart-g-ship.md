# The Quickstart in Every New Gallery (G) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Clax ships the quickstart page (`web/quickstart/`) in its binary, publishes it into each new person's gallery (pinned), and moves an untouched copy to each new release until the person first comments on it or changes it.

**Architecture:** The web build copies the shipped files to `web/dist/quickstart/`, which `clax-embedded` (and the hosted Worker's `web/dist` embed) already carry; `clax_core::quickstart::Release` reads them out of the web `FileTree` and names the release by their hash. A new `quickstarts` table (one row per person: the artifact, the release and the version Clax last wrote, and when updates stopped) is both the "seeded" flag and the evidence for the untouched rule; `clax-store` seeds and updates through the same `validate` and `create_artifact_then_async` / `publish_version_then_async` path a publish takes. The local daemon seeds and updates in `Core::start`; a hosted Clax seeds at a new person's GitHub sign-in or invite and updates at their returning sign-in.

**Tech Stack:** Rust (axum, rusqlite), Node build scripts (vitest), Playwright, Bazel.

**Spec:** `docs/superpowers/specs/2026-10-26-quickstart-onboarding-design.md` §1, §2 (D9), §9; Q5 decided (record `efe20b4d`).

## Decisions

- **What a seeded copy carries:** `index.html`, `style.css`, `icon.svg`, `clax.json`, `logos/` (with `NOTICE.md`). Not `README.md` (repository notes), and not `ideas.html` / `ideas-data.js`: the ideas, votes and roadmap are Clax's own development backlog, and a copy would show a frozen seed list and votes nobody else sees. `index.html` hides its "Make it better" tab and the per-step "This step could be better" box when it has no ideas (no `ideas-data.js` and an empty `extensions` collection), so the canonical page published from the repository looks the same as today. (Owner question 1.)
- **Release identifier:** the first 12 hex digits of SHA-256 over the shipped files, each as `path`, a NUL byte, then the file's own SHA-256, in path order, computed at run time from the web tree. Nothing has to be bumped by hand, so a page edit can never ship without its update. Updates apply when the bundled release *differs* from the one Clax last wrote, which is "newer" in practice; a downgraded binary moves an untouched copy back to the page that binary's shell supports.
- **Web build:** a new `web/scripts/build-quickstart.mjs`, run last in `pnpm run build`, copies the `SHIPPED` list from `web/quickstart/` to `web/dist/quickstart/`; `clean-dist.mjs` removes `quickstart` first. For Bazel, `web/BUILD.bazel` `APP_DIRS` gains `"quickstart/**"` (`.qual` already excluded), so `//web:dist_all` and so `//web:dist` and `clax-embedded` rebuild when the page changes. `build-e2e.mjs` copies `dist/` whole, so the e2e build carries it too.
- **Not served on the shell origin:** the shell serves only `_clax/*` and named entries (`crates/clax-server/src/shell/mod.rs:395-430`), so `web/dist/quickstart/` is never reachable as a page with the shell's cookies. A test pins this.
- **Local trigger:** `Core::start`, once the store has migrated, before it returns its router. That runs before any listing, so the gallery is never seen empty. A GET with a side effect would make concurrent first listings race. This choice also keeps the update rule in the same place.
- **Hosted trigger:** `GitHubOutcome::New` (`crates/clax-server/src/github.rs:238`) and browser invite redemption (`crates/clax-server/src/hosted.rs:368`) seed. A returning GitHub sign-in (`GitHubOutcome::Returning`) applies that person's update. A hosted Clax neither seeds nor updates at start, because a Worker has no start and a daemon would walk every person. The hosted owner and guests get none.
- **Ownership:** local: `artifacts.person` NULL, as for every owner artifact; hosted: the person's public ID (`ValidatedPublish.person`). There is no `owner_session_id` and no `versions.session_id`, because no session made the version. Comments are on and the page is private (the defaults). It is pinned in version 1's transaction. Title `Clax Quickstart`. The audit actor is `AuditCtx::DAEMON`. There is no fork lineage: nothing local to fork from.
- **Flag storage:** new table `quickstarts (person TEXT PRIMARY KEY, artifact_id TEXT, release TEXT, version_n INTEGER, seeded_at TEXT NOT NULL, stopped_at TEXT)`, keyed by public ID or `clax_core::visibility::OWNER`. `home_flags` does not exist, and `person_flags` (`crates/clax-store/src/flags.rs`) holds admin-set booleans with no room for the artifact and the version Clax wrote. A row is written in the same transaction as version 1, so the primary key makes a second seed for one person roll back.
- **Release record:** the `quickstarts.release` column plus the version label `Quickstart <release>`; no tag. Tags are one global, owner-curated list (`tags` has no person column). A tag per release would add a tag to everyone's list on each release, and on hosted would put the owner's vocabulary on members' artifacts.
- **Untouched (Q5):** a row whose `stopped_at` is NULL, whose artifact is live, whose `current_version` and `MAX(versions.n)` both equal `version_n`, and whose artifact has no `threads` row of any status. Any other state sets `stopped_at` once, and updates never resume. Renames, unpinning, tags and page db writes are not versions and do not stop updates; an update keeps them (it sends no title). A restore makes a version, so it stops updates.
- **Delete:** the row stays and `stopped_at` is set the next time the rule runs. A deleted quickstart is never seeded again or updated.
- **Existing homes:** a local home that has ever held an artifact row (drafts and deleted ones included) when G first runs gets a row with `artifact_id` NULL and no seed (owner question 2).
- **Tests stay unseeded:** `[gallery] quickstart` in `config.toml`, default `true`, can be set by env as `CLAX_GALLERY__QUICKSTART=false` (the existing `CLAX_<TABLE>__<KEY>` layering). The in-process paths default off by construction: `CoreConfig.quickstart: bool` (every literal must name it), `AppState.quickstart: None` in the testkit, and `clax-embed` passes `false` because the Clax app has its own home app. Spawned binaries get the env from `bazel/cargo.bzl` `test_kwargs`, `web/BUILD.bazel` `E2E_ENV`, `web/e2e/fixtures.ts` `startDaemon`, `plugins/pi/test/daemon-fixture.ts`, `apps/clax_app/test/support/hosted_daemon.dart`, the native test recipe, and each `env_clear()` harness that spawns `clax`.
- **Migration number:** the next free number at merge time (57 if D has not merged, 58 after D). Task 3 ends with the renumbering step.

## Global Constraints

- Build and test only through Bazel recipes (`just test-affected` while working, `just check` once before merge); never `cargo build|test` or `pnpm install` in the worktree (AGENTS.md).
- No `unsafe`, no hard links, no sleeps or wall-clock budgets; tests wait on conditions or use `ManualClock`.
- Each crate depends only on the crates above it (`scripts/check-crate-layers.py` `ALLOWED`): the release type lives in `clax-core`, seeding in `clax-store`, the glue in `clax-server`; `clax-http` only holds the state field.
- The seed goes through `clax_core::publish::validate`, so the shipped page meets the publish rules (paths, `icon.svg` rules, capabilities) or fails its test, never a person's start.
- A seed or update failure is logged (`tracing::warn!`) and never fails a start or a sign-in.
- Doc comments and commit messages state the contract; write "ID" in prose. Before editing a file, `qualifier threads <path>`.
- Independent of D, E and F: nothing here reads approvals, `account` or `shell`; Task 7 is the only task that waits on them.

## Review Focus

- A home started twice, or two daemons racing on one home: exactly one quickstart (Task 3 `a_second_seed_for_one_person_makes_nothing`, Task 4 `a_second_start_seeds_nothing_more`).
- The person deletes the quickstart, then upgrades: nothing comes back and nothing is updated (Task 3 `a_deleted_quickstart_stops_and_is_never_seeded_again`).
- The person comments, then resolves or deletes the thread, then a release lands: still no update (Task 3 `one_thread_stops_updates_for_good`).
- A binary or `--web-dist` with no `quickstart/index.html` (web UI not built, an old dist): start succeeds, nothing is seeded, no row is written, so a later build still seeds (Task 4 `a_web_ui_without_the_quickstart_seeds_nothing`).
- The person renamed or unpinned it, then a release lands: the files update, the title and pin stay (Task 3 `an_update_keeps_the_persons_title_and_pin`).

---

### Task 1 (G1): The web build ships the quickstart

**Files:**
- Create: `web/scripts/build-quickstart.mjs`, `web/scripts/build-quickstart.test.ts`
- Modify: `web/package.json:8` (`build` ends `&& node scripts/build-quickstart.mjs`), `web/scripts/clean-dist.mjs` (add `"quickstart"` to the removed names), `web/BUILD.bazel` (`APP_DIRS` += `"quickstart/**"`)
- Modify: `web/quickstart/index.html` (hide the ideas tab and box when there are no ideas), `web/quickstart/README.md` (what ships)
- Modify: `crates/clax-embedded/src/web.rs` (embedded test)
- Test: `crates/clax-server/tests/api_quickstart.rs` (new module, gated in `tests/integration.rs` as the others are)

**Interfaces:**
- Produces: `export const SHIPPED = ["index.html", "style.css", "icon.svg", "clax.json", "logos/"]` and `export function copyQuickstart(src: string, dest: string): string[]` (returns the copied relative paths, sorted). A trailing `/` copies the directory recursively; a missing listed file throws.
- Produces: `web/dist/quickstart/<path>` for every shipped file; Task 2 reads `quickstart/` from the web tree.

- [ ] **Step 1: Write the failing vitest** `copies the shipped files and nothing else`: on a temp `src` holding the real `web/quickstart` file names plus `README.md`, `ideas.html`, `ideas-data.js` and `.qual`, `copyQuickstart(src, dest)` returns `["clax.json","icon.svg","index.html","logos/NOTICE.md","logos/claude-code.svg","logos/codex.svg","logos/grok-build.svg","logos/pi.svg","style.css"]` and `dest` holds no `README.md`, `ideas.html`, `ideas-data.js` or `.qual`. `throws when a shipped file is missing`: removing `clax.json` throws naming it.
- [ ] **Step 2: Run** `bazel test //web:unit` and confirm it fails (module not found).
- [ ] **Step 3: Implement** `build-quickstart.mjs` (node `fs` `cpSync`, never links; refuses a symlinked `dist/quickstart` as `clean-dist.mjs` does), wire it into `build`, and add `quickstart` to `clean-dist.mjs` and `APP_DIRS`.
- [ ] **Step 4: Change `index.html`.** `#tab-ideas` starts `hidden`. `countIdeas()` shows it when `ideas.length > 0`, and the step box renders `.ideas-here` only then (today `SEED = window.CLAX_IDEAS_SEED || []` already tolerates the missing script). The `<script src="ideas-data.js">` stays, since a 404 there is harmless.
- [ ] **Step 5: Write the failing Rust tests.** In `crates/clax-embedded/src/web.rs` `tests`: `an_embedded_build_carries_the_quickstart`, where `web_ui("quickstart/index.html")` and `web_ui("quickstart/clax.json")` are `Some` and `web_ui("quickstart/README.md")` and `web_ui("quickstart/ideas.html")` are `None`. In `api_quickstart.rs`: `the_shell_origin_serves_no_quickstart_file`, where `TestServer::spawn()` answers `GET /_clax/../quickstart/index.html` and `GET /quickstart/index.html` with anything but a 200 carrying the page's bytes (assert the body does not contain `Your agents build pages`).
- [ ] **Step 6: Run** `just test-affected`. Expected: the vitest, `//crates/clax-embedded:clax_embedded_lib_test` (an embedding build) and `//crates/clax-server:integration__N` holding `api_quickstart` pass; `the_build_script_watches_every_web_ui_directory` passes with `web/dist/quickstart/logos` listed.
- [ ] **Step 7: Commit** `Ship the quickstart page in the web build, under web/dist/quickstart`.

### Task 2 (G2): The bundled release and its switch

**Files:**
- Create: `crates/clax-core/src/quickstart.rs` (+ `pub mod quickstart;` in `lib.rs`)
- Modify: `crates/clax-core/src/config.rs` (`[gallery] quickstart`, default `true`, in `defaults()`; accessor)
- Modify: `crates/clax-testkit/src/lib.rs` (or `server.rs`): `pub fn quickstart_tree(mark: &str) -> clax_core::embedded::MemTree`
- Test: unit tests in both `clax-core` files

**Interfaces:**
- Produces, in `clax_core::quickstart`:
  - `pub const PREFIX: &str = "quickstart/";`, `pub const TITLE: &str = "Clax Quickstart";`
  - `pub struct Release { pub id: String, pub files: BTreeMap<String, Vec<u8>> }` (paths relative to `PREFIX`)
  - `impl Release { pub fn from_tree(web: &dyn FileTree) -> Option<Release>; pub fn label(&self) -> String; pub fn capabilities(&self) -> Result<Option<serde_json::Value>>; pub fn request(&self, previous: &[String], create: bool) -> crate::Result<PublishRequest> }`
  - `from_tree` is `None` when the tree has no `quickstart/index.html`. `label()` is `format!("Quickstart {id}")`. `capabilities()` is `clax.json`'s `capabilities` (`invalid_manifest` when malformed). `request` builds files as base64 `FileInput`s, adds a `None` for each `previous` path the release lacks, sets `label`, `capabilities`, and `title: Some(TITLE)` only when `create`.
- Produces: `HomeConfig::gallery_quickstart(&self) -> bool` (default `true`; a mistyped value logs and is `true`).
- Produces: `clax_testkit::quickstart_tree(mark)`: a `MemTree` with `quickstart/index.html` (`<!doctype html><title>Clax Quickstart</title><p>{mark}</p>`), `quickstart/clax.json` (`{"capabilities":{"db":{}}}`) and `quickstart/style.css` holding `mark`, so two marks give two releases.

- [ ] **Step 1: Write failing unit tests:**
  - `a_release_is_named_by_its_files`: the same tree twice gives equal IDs of 12 lowercase hex digits. Changing one byte of one file, or renaming a file, changes the ID. Files outside `quickstart/` do not change it.
  - `a_tree_without_the_page_has_no_release`: a tree with only `quickstart/style.css` gives `None`.
  - `a_release_request_removes_what_it_no_longer_ships`: `request(&["old.js".into(), "index.html".into()], false)` has `files["old.js"] == None`, `files["index.html"]` set, `title == None`, `label == Some(label())`, and `capabilities == Some(json!({"db":{}}))`. With `create = true`, `title == Some("Clax Quickstart")`.
  - `the_shipped_request_validates`: `validate(Release::from_tree(&DiskTree::new(<repo>/web/dist)).unwrap().request(&[], true)?)` is `Ok`, so the real page passes the publish rules (skipped with a message when `web/dist/quickstart` is absent, as `clax_testkit::web_ui` is empty then).
  - `gallery_quickstart_defaults_on_and_reads_env`: the default is `true`; `[gallery]\nquickstart = false` gives `false`; `load_with_env` with `CLAX_GALLERY__QUICKSTART=false` gives `false`.
- [ ] **Step 2: Run** `just test-affected` and confirm they fail to compile.
- [ ] **Step 3: Implement** `quickstart.rs` (`sha2`, already a `clax-core` dependency) and the config accessor.
- [ ] **Step 4: Run** `just test-affected`. Expected: `//crates/clax-core:clax_core_lib_test` passes. If a `clax config` listing golden in `crates/clax-cli/tests` names every default, update it to show `gallery.quickstart = true`.
- [ ] **Step 5: Commit** `Read the bundled quickstart release from the web UI, and add the [gallery] quickstart switch`.

### Task 3 (G3): The `quickstarts` table, seeding and the untouched rule

**Files:**
- Modify: `crates/clax-store/src/migrations.rs` (migration `QUICKSTART_MIGRATION`, its `Mark::Table("quickstarts")` at the end of `MARKS_AFTER_APP`, the sequence and fresh-database tests)
- Create: `crates/clax-store/src/quickstart.rs` (+ `mod quickstart;` / re-exports in `lib.rs`)
- Test: unit tests in `quickstart.rs` and `migrations.rs`

**Interfaces:**
- Consumes: `clax_core::quickstart::Release` (Task 2); `Store::create_artifact_then_async`, `Store::publish_version_then_async` (`crates/clax-store/src/artifacts.rs:1278,1438`).
- Produces:
  - `pub const QUICKSTART_MIGRATION: u32 = <last migration const> + 1;` with SQL `CREATE TABLE quickstarts (person TEXT PRIMARY KEY, artifact_id TEXT, release TEXT, version_n INTEGER, seeded_at TEXT NOT NULL, stopped_at TEXT);`
  - `pub struct Quickstart { pub person: String, pub artifact_id: Option<ArtifactId>, pub release: Option<String>, pub version_n: Option<u32>, pub seeded_at: String, pub stopped_at: Option<String> }`
  - `pub enum Seeded { Made { artifact: Artifact, version: Version }, Already, Skipped }`
  - `pub enum Updated { Made { artifact: Artifact, version: Version }, Current, Stopped }`
  - `impl Store { pub async fn seed_quickstart(&self, ctx: &AuditCtx, person: Option<&str>, release: &Release) -> Result<Seeded>; pub async fn update_quickstart(&self, ctx: &AuditCtx, person: Option<&str>, release: &Release) -> Result<Updated>; pub fn quickstart_of(&self, person: Option<&str>) -> Result<Option<Quickstart>>; }`. `person: None` is the owner (key `OWNER`).

- [ ] **Step 1: Write failing tests** in `quickstart.rs`, with a test helper `fn release(mark: &str) -> Release` that builds the same three files as `clax_testkit::quickstart_tree` in a `MemTree` (`clax-testkit` depends on `clax-store`, so the store's tests cannot use it):
  - `a_seed_makes_one_pinned_owner_artifact`: on a fresh store, `seed_quickstart(DAEMON, None, &a)` is `Made`. `list_artifacts()` has one artifact titled `Clax Quickstart`, `pinned`, `person` None, and `comments_on` true. Version 1's label is `Quickstart <a.id>`, its files are `index.html`, `clax.json` and `style.css`, and its declaration is `{"db":{}}`. `quickstart_of(None)` is `{release: a.id, version_n: 1, stopped_at: None}`.
  - `a_persons_seed_is_theirs`: `seed_quickstart(DAEMON, Some(pid), &a)` makes an artifact whose `person` is `pid`; `quickstart_of(Some(pid))` is set and `quickstart_of(None)` is not.
  - `a_second_seed_for_one_person_makes_nothing`: a second `seed_quickstart` is `Already`, and two run with `tokio::join!` give one `Made` and one `Already`, with `list_artifacts().len() == 1`.
  - `a_home_that_held_an_artifact_is_not_seeded`: after `insert_artifact_for_test`, the owner's seed is `Skipped`, nothing is made, and the row has `artifact_id` None. A later seed is `Already`. This applies to the owner only; a person's seed ignores other artifacts.
  - `an_untouched_quickstart_moves_to_a_new_release`: seed `a`, then `update_quickstart(DAEMON, None, &b)` is `Made` with version 2 labelled `Quickstart <b.id>` holding `b`'s files. A file `a` had and `b` lacks is gone from version 2. The row has `release == b.id` and `version_n == 2`. Updating with `b` again is `Current`.
  - `one_thread_stops_updates_for_good`: seed `a`, create a thread on it (`Store::create_thread` as the owner), and `update_quickstart(&b)` is `Stopped` with `stopped_at` set and the artifact still at version 1. Resolving and deleting the thread, then `update_quickstart(&c)`, is still `Stopped`.
  - `a_version_by_anyone_else_stops_updates`: seed `a`, then `publish_version` a version 2 as the owner, and `update_quickstart(&b)` is `Stopped`. Restoring version 1 (version 3) also leaves it `Stopped`.
  - `a_deleted_quickstart_stops_and_is_never_seeded_again`: seed, `delete_artifact`, `update_quickstart(&b)` is `Stopped`, and `seed_quickstart(&b)` is `Already` with no live artifacts.
  - `an_update_keeps_the_persons_title_and_pin`: seed `a`, then `update_meta` the title to `Mine` and `set_pinned(false)`. `update_quickstart(&b)` is `Made`, the title is `Mine` and the artifact is unpinned.
- [ ] **Step 2: Write the failing migration tests:** `a_fresh_database_reaches_<N>` asserts `QUICKSTART_MIGRATION == <N>` and `user_version == QUICKSTART_MIGRATION`. The sequence test asserts `MIGRATIONS[first + 15].contains("CREATE TABLE quickstarts (")` and `MIGRATIONS.len() == QUICKSTART_MIGRATION`.
- [ ] **Step 3: Run** `just test-affected` and confirm they fail to compile.
- [ ] **Step 4: Implement** the migration (its doc comment names the columns' meaning), then `seed_quickstart`. It returns `Already` when a row exists. For the owner with any `artifacts` row, it inserts a row with NULL `artifact_id` and returns `Skipped`. Otherwise it runs `validate(release.request(&[], true)?)` with `person` set and calls `create_artifact_then_async`, whose `then` runs `UPDATE artifacts SET pinned = 1` and `INSERT INTO quickstarts`; a primary-key conflict maps to `Already`. Then `update_quickstart`, which reads the row and, unless it is untouched, sets `stopped_at = now WHERE stopped_at IS NULL` and returns `Stopped`. With the same release it returns `Current`. Otherwise it runs `validate(release.request(&<version_n's paths>, false)?)` with `if_version = Some(version_n)` and calls `publish_version_then_async`, whose `then` re-checks that there are no `threads` (`Err(CoreError::Conflict)` otherwise) and updates `release` and `version_n`. A `Conflict` from either step marks the row stopped and returns `Stopped`.
- [ ] **Step 5: Run** `just test-affected`. Expected: `//crates/clax-store:clax_store_lib_test` passes, including every migration and adoption test (the new `Mark` keeps `adopt_earlier_flutter_app` counting the marks after the app's).
- [ ] **Step 6: Commit** `Seed the quickstart once per person, and move an untouched one to each release`.
- [ ] **Merge-time step (when D or any other migration lands on main first):** fetch with `git ls-remote` and rebase. Set `QUICKSTART_MIGRATION` to the last const on main plus 1, move its SQL to the end of `MIGRATIONS` and its `Mark` to the end of `MARKS_AFTER_APP` (the array length goes up by one), and change `first + 15` and `a_fresh_database_reaches_<N>` to the new number. Reset any dev home migrated by this branch (`~/.clax-dev`) rather than upgrading it. Run `just test-affected main`.

### Task 4 (G4): A local Clax seeds and updates at start

**Files:**
- Modify: `crates/clax-http/src/state.rs` (`pub quickstart: Option<Arc<clax_core::quickstart::Release>>` on `AppState`, `None` from `assemble`)
- Create: `crates/clax-server/src/quickstart.rs` (glue; `mod quickstart;` in `lib.rs`)
- Modify: `crates/clax-server/src/core.rs` (`CoreConfig.quickstart: bool`; in `start`, after `AppState::assemble`), `crates/clax-server/src/serve.rs:188,363` (`ServeConfig.quickstart` passed through), `crates/clax-cli/src/serve.rs` (`quickstart: HomeConfig::load(home.root()).map(|c| c.gallery_quickstart()).unwrap_or(true)`), `crates/clax-embed/src/core.rs:637` (`quickstart: false`), every other `CoreConfig {…}` literal (`crates/clax-server/tests/core_start.rs`, `tests/daemon.rs`: `false`)
- Modify (off switch for spawned binaries): `bazel/cargo.bzl` `test_kwargs` (`env["CLAX_GALLERY__QUICKSTART"] = "false"`), `web/BUILD.bazel` `E2E_ENV`, `web/e2e/fixtures.ts:75` (before `...opts.env`), `plugins/pi/test/daemon-fixture.ts:37`, `apps/clax_app/test/support/hosted_daemon.dart`, the `test-native` path in `scripts/run.sh`. The `env_clear()` harnesses that spawn `clax` (`crates/clax-cli/tests/{verify_harnesses,native_host,extension}.rs`, `crates/clax-content-git/tests/common.rs`, `crates/clax-git/tests/{common,receive_client}.rs`, `crates/clax-server/tests/{api_where_to_ask,golden_single_owner}.rs`, `crates/clax-testkit/src/clients.rs`) get `.env("CLAX_GALLERY__QUICKSTART", "false")`. Re-grep `env_clear` and `"serve"` under `crates/*/tests`, `web/e2e`, `plugins/pi/test` and `apps/clax_app/test` for any added since.
- Test: `crates/clax-server/tests/core_start.rs`

**Interfaces:**
- Consumes: Task 2 `Release::from_tree`, Task 3 `seed_quickstart` / `update_quickstart`.
- Produces: `pub(crate) async fn seed(s: &AppState, person: Option<&str>)` and `pub(crate) async fn update(s: &AppState, person: Option<&str>)` in `clax_server::quickstart`. Each is a no-op when `s.quickstart` is `None`, logs any error, and on `Made` publishes `Event::Version { artifact_id, n, by_page: false, title, at }` as the publish route does.

- [ ] **Step 1: Write failing tests** in `core_start.rs`, with `cfg()` taking `web` and `quickstart`, and with `web` an `Arc<MemTree>` from `clax_testkit::quickstart_tree`. Each lists `GET /api/artifacts` through `core.router()` with the core's owner token (`core.state().token`).
  - `a_fresh_home_starts_with_the_quickstart_pinned`: one artifact, `title == "Clax Quickstart"`, `pinned == true`.
  - `a_second_start_seeds_nothing_more`: `start`, `stop`, `start` on one home gives one artifact.
  - `a_new_release_reaches_an_untouched_quickstart_at_start`: start with mark `a`, stop, start with mark `b`. `GET /api/artifacts/<id>/versions` has 2 versions, the latest labelled `Quickstart <b's id>`.
  - `the_switch_off_seeds_nothing`: `quickstart: false` gives `[]` and no `quickstarts` row (`store.quickstart_of(None)` None).
  - `a_web_ui_without_the_quickstart_seeds_nothing`: a `MemTree` with only `index.html` gives `[]` and no row, and a later start with `quickstart_tree("a")` seeds.
- [ ] **Step 2: Run** `just test-affected` and confirm they fail (no field `quickstart`).
- [ ] **Step 3: Implement.** In `Core::start`, when `cfg.quickstart`, set `state.quickstart = Release::from_tree(&*state.web).map(Arc::new)`. When `state.hosted.is_none()`, `quickstart::update(&state, None).await` and then `quickstart::seed(&state, None).await`, both before `build_router_with_shutdown`. Thread `quickstart` through `ServeConfig` and the CLI. Add the env switch to every harness listed above.
- [ ] **Step 4: Run** `just test-affected main`. Expected: the new tests pass, and no existing CLI, server, Pi or e2e test sees an extra artifact. A failing gallery or `clax list` assertion names a harness the grep missed: add the env there.
- [ ] **Step 5: Commit** `Seed the quickstart in a new local home, and update an untouched one at start`.

### Task 5 (G5): A hosted Clax seeds each new person

**Files:**
- Modify: `crates/clax-server/src/github.rs:238` (after the `person_joined` announce: `quickstart::seed(&s, Some(&public_id)).await`; on `GitHubOutcome::Returning`: `quickstart::update(&s, Some(&public_id)).await`)
- Modify: `crates/clax-server/src/hosted.rs:368` (after `redeem_invite_in_browser` succeeds: `quickstart::seed(&s, Some(&signed.who.viewer.public_id)).await`)
- Modify: `crates/clax-worker/src/instance.rs:649` (`app.quickstart = Release::from_tree(&WebUi).map(Arc::new)`, unless the Worker's hosted settings turn it off; the Worker shares these routes)
- Test: `crates/clax-server/tests/api_hosted_github.rs`, `crates/clax-server/tests/api_hosted.rs`

**Interfaces:**
- Consumes: Task 4 `quickstart::seed` / `update`; a hosted `TestServer` spawned with `|s| s.quickstart = Some(Arc::new(Release::from_tree(&clax_testkit::quickstart_tree("a")).unwrap()))` (a `Site` constructor option in each file's helper).

- [ ] **Step 1: Write failing tests:**
  - `a_new_github_person_starts_with_the_quickstart` (`api_hosted_github.rs`): a member's first sign-in, then `GET /api/artifacts` with their cookie, lists one artifact titled `Clax Quickstart` with `pinned` true, whose `person` is their public ID. The owner's listing does not include it. A second sign-in of the same member makes no second artifact. The owner's own sign-in seeds nothing.
  - `a_returning_person_gets_the_new_release`: swap `state.quickstart` to mark `b` (a `TestServer` accessor, or a `Site` rebuilt on the same home). The member's second sign-in moves their quickstart to version 2.
  - `an_invited_person_starts_with_the_quickstart` (`api_hosted.rs`, beside `an_invite_joins_once_in_the_browser_as_a_person`): after `/api/hosted/invites/<code>/accept`, the new person's listing has the quickstart.
  - `a_hosted_start_seeds_no_one`: a hosted `Core::start` with `quickstart: true` lists no artifact for the owner.
- [ ] **Step 2: Run** `just test-affected` and confirm they fail.
- [ ] **Step 3: Implement** the three calls and the Worker's field. The hosted branch of Task 4's `Core::start` already skips the owner.
- [ ] **Step 4: Run** `just test-affected`. Expected: `//crates/clax-server:integration__N` (hosted modules) and `//crates/clax-worker:…` build and pass.
- [ ] **Step 5: Commit** `Give each person new to a hosted Clax the quickstart, and its new releases at sign-in`.

### Task 6 (G6): A new gallery opens on the quickstart (browser)

**Files:**
- Create: `web/e2e/quickstart.spec.ts`
- Modify: `web/quickstart/README.md` (the page ships with Clax; the repository's published copy is the one with ideas)

**Interfaces:**
- Consumes: `startDaemon({ env })` from `web/e2e/fixtures.ts` (its default env now carries `CLAX_GALLERY__QUICKSTART=false`).

- [ ] **Step 1: Write the failing Playwright tests:**
  - `a new home's gallery shows the quickstart, pinned`: `startDaemon({ env: { CLAX_GALLERY__QUICKSTART: "true" } })`. The gallery at `/` shows one card named `Clax Quickstart` with its pinned marker and no `.empty-gallery`. Opening it, the frame's `h1` reads `Your agents build pages. Clax gives them a home.` and `#tab-ideas` is hidden.
  - `the switch off leaves the gallery empty`: default `startDaemon()` shows `When an agent publishes a page, it lands here.`
- [ ] **Step 2: Run** `just web-e2e quickstart`. Expected: `2 passed`. Then swap the two tests' env values, run it again, and confirm both fail, so each test depends on its switch.
- [ ] **Step 3: Run** `just check` once (the branch's whole gate). Expected: every lane passes. A lane failing in code G did not touch is rerun alone, and both results are reported.
- [ ] **Step 4: Commit** `Test that a new gallery opens on the pinned quickstart`.
- [ ] **Step 5: Before merging:** run `qualifier diff main`, record any deferred item as a concern on its lines, then do the merge-time step of Task 3.

### Task 7 (G7, after E and F merge): The page signs in and points with its capabilities

Spec §5, §8. In `web/quickstart/index.html`, step 1 (`signin`) gains a Sign in button calling `(await claude.use("account"))?.signIn()` from its click handler. It marks the step done on `connected` and falls back to today's text when `use("account")` is `null`. The tour's "where is it" links call `shell.highlight("comment" | "send" | "publish" | "fork" | "versions")` and say where the control is when it resolves `false`. `clax.json` adds `"account": {}` and `"shell": {}`. Test: `web/e2e/quickstart.spec.ts` `the quickstart's sign-in step asks the shell` (a fake public Clax as in B's e2e: clicking Sign in shows the shell's consent). A changed page is a new release, and Tasks 3-4 move untouched copies to it at the next start. Commit `Sign in and point at Clax's controls from the quickstart`.
