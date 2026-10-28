# The Sizzle Reel Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A short animated GIF of Clax's real UI in the README, linked to the landing page (the Clax Quickstart), and a ~70 s video (with a ~45 s cut) of the same UI, both recorded by scripts against a scratch daemon and real browsers. The GIF is its own render from the recorded frames (not a transcode of the video); the video is composed with fframes and kept as a build output; both are uploaded only by a confirming `just reel-upload`, the video's upload coming last.

**Phases (spec §1), in this order:**
1. **The README GIF** (Tasks 1–7): the recorder, deterministic mode, the `point`, `version` and `realtime` vignettes, the GIF render, its upload to the landing artifact, the README block.
2. **The full video as a build output** (Tasks 8–14): clips, cards, the composition, the remaining vignettes. `bazel build //web/reel:reel`, copied to `target/reel/`; nothing uploaded.
3. **Publishing the video, later** (Tasks 15–18): byte ranges, the landing-page player, `reel-upload --video` and the release's MP4, CI. Do not start phase 3 before the owner asks; phases 2 and 3 do not block phase 1.

**Architecture:** A TypeScript recorder in `web/reel/` (part of the web pnpm project, reusing `web/e2e`'s daemon, people and API helpers) drives one vignette per script through a `Director` that owns all timing, and captures a clip in fast mode (`recordVideo`) or one PNG per frame in deterministic mode (Chromium virtual time). A Rust composer in `tools/reel-compose/` (its own Cargo workspace, fframes 1.3.0) turns frames into the README GIF (its own cut list, size, frame rate and palette) and, in phase 2, frames into clips and clips, cards and transitions into the reel. Bazel runs both as `manual` targets; `scripts/reel-upload.sh` publishes through the owner's installed Clax to the landing artifact, which is published from `web/quickstart/` (the canonical Quickstart).

**Tech Stack:** TypeScript on Node 22 with Playwright (the pinned `@chromium//:browsers`), vitest; Rust with fframes 1.3.0 (FFmpeg through `ffmpeg-sys-fframes` 9.0.0, x264: GPL accepted, the composer is never shipped), the `gif` crate; Bazel (rules_js, rules_rust with a second crate_universe hub); bash.

**Spec:** `docs/superpowers/specs/2026-10-27-sizzle-reel-design.md`

## Global Constraints

- Every reel target is tagged `manual`; none is reached by `//...`, `just ci` or `just test-affected` (spec §8).
- Canvas 1920 × 1080 at 30 fps; camera viewport 1280 × 800 CSS px at device scale 1.25 (captures 1600 × 1000), light theme, locale `en-US`, timezone `UTC`, `REEL_EPOCH = 2026-11-03T09:41:00Z` (spec §5, §7.1).
- `reel.gif` under 2 MiB, 720 px wide, 15 fps, 6–10 s, made from the vignettes' frames and never from an MP4 (spec §9.4); `reel.mp4` under 18 MiB; full reel 60–80 s; short cut 40–50 s; each vignette 3–10 s (spec §8.1, §9.3).
- Silent (no audio track), light theme only, no Clax app or phone shown (spec D10, D11, D14).
- The landing artifact is published only by `reel-upload`, which stages `web/quickstart/`'s files with the reel's; `web/quickstart/` stays the canonical Quickstart (spec D8, R8). `[upload] landing_artifact` and `landing` in `web/reel/reel.toml` are set once, in the commit after the Quickstart's first publish; until then they are empty and `reel-upload` and the README block wait.
- No sleeps standing in for a condition, no wall-clock budgets in a check; pacing belongs to the `Director` alone (AGENTS.md; spec §6.2).
- Daemons run on scratch `CLAX_HOME`s on system-picked ports and are stopped by the handle that started them; never `~/.clax` or port 7480; never `pgrep -f`/`pkill -f` (AGENTS.md).
- `tools/reel-compose` depends on no Clax crate, is excluded from the root workspace, and forbids `unsafe_code` (spec §4).
- No reel output (frames, clips, MP4, GIF, PNG, manifest) is ever committed; `target/reel/` holds local copies (spec §1).
- Never hard links, in code, tests or scripts (AGENTS.md).
- Doc comments and commit messages state the contract; "ID" in prose. Before editing a file, `qualifier threads <path>`.
- Seeded copy is fictional (Northwind; Alex, Mia, Sam, Priya; sessions `planner`, `builder`): no real person or organisation (spec §3).
- `reel-upload` never skips its question, never runs a `clax` that resolves into a worktree or `bazel-bin`, never pushes to an artifact whose link is not `[upload] landing`, and uploads no MP4 without `--video` (spec §10).

## Review Focus

- A vignette whose control was renamed: the render fails naming the locator, it does not film the wrong screen (Task 1 `a missing control fails the vignette, naming it`).
- Two deterministic renders of one vignette on one machine: byte-identical frames (Task 2 `realtime_determinism`).
- The README GIF is rendered from frames, not the video: it builds with no MP4 present, is under 2 MiB, loops through its 6–10 s of segments, and two builds from the same frames are byte-identical (Task 5 `the_gif_is_its_cut_list_long`, `the_gif_is_small`, `the_gif_is_deterministic`).
- `reel-upload` answered anything but `y`, or with a worktree `clax`, or a mismatched link: nothing uploaded; and the landing artifact is never published without `reel.gif` (Task 6 cases `declined`, `worktree_clax`, `wrong_link`, `stages_quickstart_with_the_gif`).
- Phase 3 only: a seeded quickstart with no `reel.json`, one that does not parse, or one naming a missing asset: the link card or nothing, never a broken player (Task 16 three cases).
- Phase 3 only: Safari asking for byte ranges of the video: 206 with the right bytes, 416 past the end (Task 15).

---

## Phase 1: the README GIF

### Task 1: The recorder skeleton and the `realtime` vignette in fast mode (the proof)

**Files:**
- Create: `web/reel/reel.toml` (the `[reel]`, `[end]`, `[poster]` and `[upload]` tables of spec §9.3 and one `[[vignette]]`, `realtime`; the `[gif]` table waits for Task 5, since its segments name vignettes Task 4 writes), `web/reel/config.ts`, `web/reel/pace.ts`, `web/reel/cursor.ts`, `web/reel/director.ts`, `web/reel/camera.ts`, `web/reel/record.ts`, `web/reel/vignettes/realtime.ts`, `web/reel/seed/checklist.html`, `web/reel/BUILD.bazel`, `web/reel/reel.bzl`
- Create: `web/reel/config.test.ts`, `web/reel/pace.test.ts`, `web/reel/director.test.ts`
- Modify: `web/package.json` (`"reel:record": "node reel/record.ts"`, `"reel:test": "playwright test -c reel/playwright.config.ts"`; `lint` gains `reel`), `web/tsconfig.json` (`include` gains `"reel"`), `web/vitest.config.ts` (include `reel/**/*.test.ts` if its glob does not already), `web/BUILD.bazel` (`UNIT_DATA` gains `//web/reel:files`, so `//web:lint`, `//web:typecheck` and `//web:unit` see the reel's sources; they are not inputs of `//web:dist`)
- Modify: `bazel/js/defs.bzl` (`npm_script_build` gains `project = None` and `skips = []`), `justfile` (`reel *ARGS`), `scripts/run.sh` (task `reel`), `scripts/run.test.sh`

**Interfaces:**
- Produces, `web/reel/config.ts`:
  ```ts
  export type VignetteCfg = { id: string; title?: string; seconds: number; shortSeconds?: number; inShort: boolean };
  export type ReelCfg = { fps: number; size: [number, number]; capture: { viewport: [number, number]; scale: number; theme: "light" | "dark" };
    cardSeconds: { full: number; short: number }; vignettes: VignetteCfg[]; end: { seconds: number; headline: string; footer: string };
    poster: { vignette: string; at: number };
    gif?: { fps: number; size: [number, number]; captionPx: number; maxMib: number; segments: { vignette: string; beat: string; offset: number; seconds: number; caption: string }[] };
    upload: { landingArtifact: string; landing: string } };
  export function parseReel(toml: string, scripts: string[]): ReelCfg; // throws Error naming the key or ID
  export function totalSeconds(cfg: ReelCfg, cut: "full" | "short"): number;
  ```
  TOML parsed with `smol-toml` (add to `web/package.json` devDependencies, `pnpm-lock.yaml` updated by `pnpm install --lockfile-only` through Bazel's `pnpm` or the `-native` path; `scripts/pnpm-lock-check.py` must pass).
- Produces, `web/reel/pace.ts`: `export const travelMs = (px: number) => Math.min(900, 250 + 0.6 * px)`; `export function ease(t: number, curve: [number, number, number, number]): number` (cubic Bézier, the curves of `design/tokens.json` `motion.curves`); `export function typingDelays(text: string, seed: string): number[]` (mean 1000/14 ms, ±25 % from a seeded PRNG, deterministic).
- Produces, `web/reel/director.ts`:
  ```ts
  export interface Camera { frameMs: number; roll(ms: number): Promise<void>; still(): Promise<void>; beats: { name: string; frame: number }[] }
  export class Director {
    constructor(page: Page, camera: Camera, seed: string);
    move(target: Locator, opts?: { offset?: { x: number; y: number } }): Promise<void>;
    click(target: Locator): Promise<void>; type(target: Locator, text: string): Promise<void>; press(key: string): Promise<void>;
    hold(ms: number): Promise<void>; until(cond: () => Promise<boolean>, what: string): Promise<void>;
    beat(name: string): void; offstage<T>(fn: () => Promise<T>): Promise<T>;
  }
  ```
  `until` polls `cond` (10, 20, 50, then every 100 ms) with the camera stilled, and fails after 30 s with `until: <what>`. Every locator the director is given is first awaited with `waitFor({ state: "visible", timeout: 5000 })`, whose error names the locator.
- Produces, `web/reel/record.ts`: CLI `--vignette <id> --mode fast|deterministic --out <dir>`; writes `clip.webm` (fast) or `frames/NNNNNN.png` (deterministic, Task 2) and `beats.json` = `{ "fps": 30, "frames": <n>, "beats": [{ "name", "frame" }] }`.
- Produces, `web/reel/vignettes/<id>.ts`: `export default { id, setup(stage), play(d, cast) } satisfies Vignette` (spec §6.2); `Stage` = `{ browser: Browser; newCamera(): Promise<Page>; startDaemon: typeof startDaemon; hostedDaemon: typeof hostedDaemon; onStop(fn): void }`.
- Produces, `web/reel/BUILD.bazel`: `copy_to_bin(name = "files", srcs = glob(["**"], exclude = ["**/.qual"]), visibility = ["//web:__pkg__"])`.
- Produces, `web/reel/reel.bzl`: `reel_vignette(name = "<id>")` declaring `<id>_fast` (and, from Task 2, `<id>_frames`) with the srcs of spec §8, tags `["manual", "no-sandbox", "no-remote", "cpu:4"]`, env `CLAX_E2E_BIN`, `CLAX_E2E_WEB_DIST`, `PLAYWRIGHT_BROWSERS_PATH`, `CLAX_GALLERY__QUICKSTART=false`, as `web/BUILD.bazel`'s `E2E_ENV`.

- [ ] **Step 1: Write the failing unit tests.** `config.test.ts`: `parses the realtime table` (`parseReel(<the committed reel.toml>, ["realtime"])` gives `fps 30`, `size [1920,1080]`, `vignettes[0].title === "Realtime collaboration"`); `refuses an unknown key` (`[reel] colour = 1` throws `/unknown key reel.colour/`); `refuses a vignette with no script` (`scripts = []` throws `/realtime: no script/`); `refuses a duration outside 3–10 s` (`seconds = 11` throws `/realtime.seconds/`); `totals the cuts` (a fixture with the nine vignettes of spec §3 gives `totalSeconds(cfg,"full")` in [60,80] and `"short"` in [40,50]). `pace.test.ts`: `travel is capped` (`travelMs(2000) === 900`, `travelMs(0) === 250`); `ease ends at 0 and 1` for each curve; `typing is deterministic` (`typingDelays("Book the venue","realtime")` equal across two calls, each within [53.6, 89.3] ms). `director.test.ts` with a fake `Camera` and a fake `Page`/`Locator`: `hold rolls the camera` (`hold(500)` calls `roll(500)`); `until stills the camera and names what it waited for` (a cond never true fails `/until: the tick reached Alex/` under a fake clock, no real 30 s wait: inject the poll's `now` and `wait`); `a missing control fails the vignette, naming it` (a locator whose `waitFor` rejects makes `move` reject with its description).
- [ ] **Step 2: Run** `just web-test` (after adding `//web/reel:files` to `UNIT_DATA`) and confirm the three files fail (modules not found).
- [ ] **Step 3: Implement** `config.ts`, `pace.ts`, `director.ts` to the interfaces. `cursor.ts` exports `installCursor(page: Page): Promise<void>` (an init script: a closed shadow root on `document.documentElement`, a 20 px arrow SVG, `pointer-events:none`, `z-index:2147483647`, hidden until the first `__reelCursor.to(x, y)`, a 300 ms press ring on `__reelCursor.press()`); the director calls it through `quietEval` (`web/e2e/time.ts`) so no user activation is granted by the cursor itself.
- [ ] **Step 4: Run** `just web-test`; the units pass; `oxlint` and `tsc` pass over `reel/`.
- [ ] **Step 5: Write the vignette** `vignettes/realtime.ts` per spec §3: `hostedDaemon()`, `ownerOf(browser, origin, "Alex")` as the camera (its context gets the §5 viewport, scale, colour scheme; `page.clock.install({ time: REEL_EPOCH })`), `memberOf` for Mia and Sam with `rightsOn(aid)`, the page `seed/checklist.html` (a to-do list over the `db` capability, five items, published with `publishWith(..., { db: {} })`), beats `pointers`, `draft`, `tick`. Mia's and Sam's moves are `offstage` `page.mouse.move` steps; the tick is Sam's click; `d.until` waits for Alex's view to show Sam's tick and Mia's draft (the shell's `.peer-tag` and the page's checked box, as `web/e2e/live-layers.spec.ts` reads them).
- [ ] **Step 6: Implement** `camera.ts`'s fast camera (`recordVideo: { dir, size: { width: 1600, height: 1000 } }` on the camera context; `roll(ms)` lets `ms` of real time pass while the video records: the one wait on real time in the recorder, and the length of the shot, not a stand-in for a condition; `still()` is a no-op) and `record.ts` (start, play, save the video to `<out>/clip.webm`, write `beats.json`, stop every daemon through `stage.onStop`, in `finally`).
- [ ] **Step 7: Wire Bazel.** `npm_script_build` gains `project` (used in place of `native.package_name()` in `args`) and `skips` (passed as `--skip` like `npm_script_test`). `web/reel/BUILD.bazel`: `reel_vignette(name = "realtime")`. `scripts/run.sh` task `reel`: `bazel build //web/reel:<id>_fast` (`--fast <id>`), copy its outputs into `target/reel/<id>/` with `cp` (never `ln`), and say where; without Bazel, refuse naming that the reel needs Bazel. `justfile`: `reel *ARGS` → `./scripts/run.sh reel "$@"`. `scripts/run.test.sh`: `reel` is Bazel-only (`--dry-run reel --fast realtime` prints the `bazel build` line; `CLAX_BUILD=native` refuses).
- [ ] **Step 8: Run the proof.** `just reel --fast realtime`. Expected: `target/reel/realtime/clip.webm` exists and is larger than 100 KB, and `beats.json` lists `pointers`, `draft`, `tick` in that order with increasing frames; play it and see Alex's view with two named pointers, a live draft and the tick. Run `bazelisk build //web/reel:realtime_fast` again: `1 action cached` (no re-render). `just test-affected --list` names no `//web/reel` target.
- [ ] **Step 9: Commit** `Record the realtime collaboration vignette in fast mode, the reel's first clip`.

### Task 2: Deterministic mode, proven on `realtime`

**Files:**
- Modify: `web/reel/camera.ts` (`deterministicCamera`), `web/reel/record.ts`, `web/reel/reel.bzl` (`<id>_frames`)
- Create: `web/reel/camera.spec.ts`, `web/reel/playwright.config.ts` (Chromium from `PLAYWRIGHT_BROWSERS_PATH`, one worker), `web/reel/determinism.sh` (a manual `sh_test`)
- Modify: `web/reel/BUILD.bazel` (`camera_test`: `npm_script_test(project = "web", script = "reel:test", tags = ["manual", "no-sandbox"])`; `realtime_determinism`), `bazel/js/defs.bzl` (`npm_script_test` gains `project` as `npm_script_build` did in Task 1)

**Interfaces:**
- Consumes: Task 1's `Camera`, `Director`, `record.ts`.
- Produces: `export async function deterministicCamera(page: Page, out: string, fps = 30): Promise<Camera>`; frames named `frames/%06d.png`; `beats.json` as Task 1.

- [ ] **Step 1: Write the failing test** `camera.spec.ts`: against a page served from `about:blank` with a CSS transition of 300 ms and a `setTimeout`-driven counter, `deterministicCamera` with `roll(100)` writes exactly 3 frames, the transition's computed opacity in frame 3 equals its value at 100 ms (±0.02), and `still()` followed by a 2 s `page.evaluate` busy wait writes no frame. This runs Chromium, so it is a Playwright test run by `//web/reel:camera_test`, not vitest (the vitest glob must not match `*.spec.ts`).
- [ ] **Step 2: Run** `bazelisk test //web/reel:camera_test` and confirm it fails (no `deterministicCamera`).
- [ ] **Step 3: Implement** with a CDP session on the camera page: `Emulation.setVirtualTimePolicy({ policy: "pause" })` at start; `roll(ms)` repeats `ceil(ms / frameMs)` times: `setVirtualTimePolicy({ policy: "advance", budget: frameMs })`, await `Emulation.virtualTimeBudgetExpired`, `settle` (two animation frames, `web/e2e/time.ts`), `Page.captureScreenshot({ format: "png", captureBeyondViewport: false })`, write the next frame. `still()` sets `pause`.
- [ ] **Step 4: Run** the camera test; it passes.
- [ ] **Step 5: Fallback, only if Step 4 cannot pass** because the shell does not paint stream events under paused virtual time (spec R1): replace the virtual-time policy with `page.clock` (`install`, then `runFor(frameMs)` per frame) and the CDP Animation domain (`Animation.enable`, `setPlaybackRate({ playbackRate: 0 })`, and per frame `seekAnimations` of every started animation to its elapsed time), keep the same `Camera` interface, and reply to the `alternative` record on spec §7.2 saying which was taken and why (a `qualifier reply` naming the failing assertion).
- [ ] **Step 6: Write** `determinism.sh`: runs `record.ts --vignette realtime --mode deterministic` twice into two temporary directories and fails, printing the first differing frame's number, unless `shasum -a 256` of every frame matches and the frame counts are equal. `realtime_determinism` is an `sh_test` with the recorder's data, tagged `manual`, `no-sandbox`, `exclusive`.
- [ ] **Step 7: Run** `bazelisk test //web/reel:realtime_determinism`. Expected: PASS. If frames differ, find the source (a timestamp, a random ID on screen, a font fallback) and fix it in the vignette or the seed, not by loosening the comparison.
- [ ] **Step 8: Run** `bazelisk build //web/reel:realtime_frames`; `frames/` holds `7 × 30 = 210` frames (± one beat's rounding) of 1600 × 1000.
- [ ] **Step 9: Commit** `Record vignettes frame by frame on Chromium's virtual time`.

### Task 3: The composer's crate, its Bazel build and `probe`

**Files:**
- Create: `tools/reel-compose/Cargo.toml` (`[workspace]`; `[lints.rust] unsafe_code = "forbid"`; dependencies `fframes = { version = "=1.3.0", default-features = false, features = ["cpu_renderer", "h264", "libav-agree-gpl"] }`, `anyhow`, `clap` 4 with `derive`, `serde`, `serde_json`, `toml`, `sha2`, `gif`), `tools/reel-compose/Cargo.lock`, `tools/reel-compose/src/main.rs`, `tools/reel-compose/src/probe.rs`, `tools/reel-compose/BUILD.bazel`, `tools/reel-compose/README.md` (what it needs installed: x264, nasm, clang; spec §8)
- Modify: `Cargo.toml` (`exclude = ["crates/clax-worker", "tools/reel-compose"]`), `MODULE.bazel` (`crate.from_cargo(name = "reel_crates", cargo_lockfile = "//tools/reel-compose:Cargo.lock", manifests = ["//tools/reel-compose:Cargo.toml"])`, `crate.annotation(crate = "ffmpeg-sys-fframes", repositories = ["reel_crates"], build_script_env = {...}, build_script_tags = ["requires-network", "no-sandbox", "no-remote"])`, `use_repo(crate, "crates", "reel_crates")`; an `http_archive` `inter_font` for Inter 4.x by `sha256`), `MODULE.bazel.lock` (by `just bazel-sync`), `scripts/check-no-unsafe.sh` and `scripts/test-check-no-unsafe.sh` (scan `tools/` too; `tools/*/Cargo.toml` must forbid `unsafe_code`), `docs/bazel.md` ("Targets": the reel's targets and that the composer's build is not hermetic)

**Interfaces:**
- Produces: binary `//tools/reel-compose:reel-compose` (tags `manual`). `reel-compose probe <file>` prints `{"duration_s": f64, "width": u32, "height": u32, "bytes": u64, "frames": u32}`; this task reads GIFs (frames, the sum of their delays), Task 8 adds MP4.
- Produces: `pub fn probe(path: &Path) -> anyhow::Result<Probe>` in the crate (Tasks 5, 8 and 6's tests call it).

- [ ] **Step 1: Write the failing tests** (`#[cfg(test)]` in `probe.rs`, run by `//tools/reel-compose:reel-compose_test`, manual): `probes_a_gif` (a GIF written by the `gif` crate in the test, ten 160 × 100 frames of 10 cs each → `frames 10`, `duration_s` 1.0 ± 0.01, `width 160`, `height 100`); `refuses_what_it_cannot_read` (a text file → an error naming the file).
- [ ] **Step 2: Run** `bazelisk test //tools/reel-compose:reel-compose_test` and confirm it fails (no `probe`). This first build compiles or downloads FFmpeg; run it in the background, logging to `target/reel-compose-build.log`.
- [ ] **Step 3: Implement** `probe` for GIFs with the `gif` crate. Add the `fframes` dependency now (its features `cpu_renderer`, `h264`, `libav-agree-gpl`, which the caption in Task 5 and the clips in Task 8 use) so the non-hermetic FFmpeg build is paid and proven once, here; a `main.rs` that links it and prints its version is enough.
- [ ] **Step 4: Run** the tests; they pass. `scripts/test-check-no-unsafe.sh` passes with a case for a `tools/` manifest without `forbid`. `just bazel-sync` changes nothing more; `scripts/bazel-sync.sh --check` passes. `bazelisk query //...` and `just test-affected --list` finish without fetching `reel_crates`' sources (check `bazelisk info output_base`/external has no `reel_crates__fframes-*` after a fresh query; if one is fetched, move the hub's `use_repo` behind a `manual`-only package and say how in `docs/bazel.md`).
- [ ] **Step 5: Commit** `Add the reel's composer crate, with fframes, and its probe`.

### Task 4: The `point` and `version` vignettes

**Files:**
- Create: `web/reel/vignettes/point.ts`, `web/reel/vignettes/version.ts`, `web/reel/seed/launch-plan/v1.html` … `v4.html` (a chart of six monthly bars and a table; v4 highlights March in the page's accent), `web/reel/seed/launch.ts` (`seedLaunchPlan(base, token, upTo: number): Promise<{ aid: string; sid: string }>` publishing v1..`upTo` as session `planner`)
- Modify: `web/reel/BUILD.bazel` (`reel_vignette` for each), `web/reel/reel.toml` (their tables)

**Interfaces:**
- Consumes: Task 1's `Director`, `Stage`; `registerSession`, `publishAs`, `publishNext`, `setWorking`, `openArtifact`, `commentModeIn` from `web/e2e/fixtures.ts`; `replyAsAgent` from `web/e2e/questions-helpers.ts`.

- [ ] **Step 1:** For each vignette, write it per spec §3 with its beats (`point`: `comment-mode`, `pointed`, `typed`, `sent`; `version`: `working`, `v4`, `reply`), then `just reel --fast <id>` and watch it: each beat on screen, nothing off-frame, the cursor never still for more than 1.5 s outside a `hold`. The beats `pointed` and `v4` are the GIF's cut points (spec §9.3 `[gif]`): keep their names.
- [ ] **Step 2:** `bazelisk build //web/reel:<id>_frames` for each; `beats.json` lists the beats in order; the frame count is within 0.5 s (15 frames) of `reel.toml`'s seconds (adjust `hold`s, not `reel.toml`, unless the beat needs the time).
- [ ] **Step 3:** `bazelisk test //web/reel:realtime_determinism` still passes (shared code changed); add `point_determinism` the same way and run it.
- [ ] **Step 4: Commit** `Record the Point at anything and the new-version vignettes`.

### Task 5: The README GIF, its own render

**Files:**
- Create: `tools/reel-compose/src/tokens.rs`, `tools/reel-compose/src/config.rs` (`ReelConfig`: the keys and refusals of `parseReel`, here for `[reel]` and `[gif]`), `tools/reel-compose/src/gif.rs`
- Modify: `tools/reel-compose/src/main.rs` (`gif`), `tools/reel-compose/BUILD.bazel` (data: `//design:tokens`, `@inter_font//:files`), `design/BUILD.bazel` (`visibility` gains `//tools/reel-compose:__pkg__` and `//web/reel:__pkg__`), `MODULE.bazel` (an `http_archive` `inter_font` for Inter 4.x by `sha256`; `MODULE.bazel.lock` by `just bazel-sync`), `web/reel/reel.toml` (the `[gif]` table and its three `[[gif.segment]]` tables of spec §9.3), `web/reel/config.ts` and `config.test.ts` (parse and refuse the `[gif]` cases of spec §9.3), `web/reel/BUILD.bazel` (`reel_gif`: srcs the `point`, `version` and `realtime` `_frames`, `reel.toml`, tokens and the font; output `reel.gif`), `scripts/run.sh` (`reel --gif` builds `//web/reel:reel_gif`, copies `reel.gif` and writes `build.json`), `scripts/run.test.sh`

**Interfaces:**
- Consumes: Task 2's `<id>_frames` (`frames/%06d.png`, `beats.json`); Task 3's `probe`.
- Produces: `reel-compose gif --config reel.toml --frames <root> --tokens design/tokens.json --font-dir <dir> -o reel.gif`, where `<root>/<id>/frames/%06d.png` and `<root>/<id>/beats.json` exist for each vignette a segment names. Output per spec §9.4: 720 × (450 + 36 caption bar), 15 fps from every second source frame with delays 7, 7, 6 cs in turn, one global 256-colour NeuQuant palette over a sample of all segments' frames, no dithering, each frame after a segment's first stored as its changed rectangle with unchanged pixels transparent, infinite loop, under `max_mib`.
- Produces, `tokens.rs`: `pub struct Tokens { pub bg: Rgb, pub fg: Rgb, pub muted: Rgb, pub accent: Rgb, pub card: Rgb, pub border: Rgb, pub radius_lg: f32, pub shadow_2: String, pub enter: Motion, pub exit: Motion }`, `pub struct Motion { pub ms: u32, pub curve: [f32; 4] }`, `pub fn Tokens::load(tokens_json: &Path, theme: Theme) -> anyhow::Result<Tokens>` (resolves aliases such as `"accent": "agent"`; `Theme` has `Light` only for now).
- Produces, `gif.rs`: `pub fn render_gif(cfg: &ReelConfig, frames_root: &Path, t: &Tokens, font_dir: &Path, out: &Path) -> anyhow::Result<()>`, whose caption bar is drawn by fframes' SVG renderer with Inter 600 from `@inter_font` only (`load_system_fonts: false`).

- [ ] **Step 1: Write the failing tests** (synthetic frames: three vignettes of flat-colour 1600 × 1000 PNGs with a `beats.json` naming `pointed`, `v4` and `tick`): `the_gif_is_its_cut_list_long` (segments 2.5 + 2.5 + 3 s → 120 frames, `probe` duration 8.0 ± 0.1 s, width 720, height 486, `Repeat::Infinite`); `delays_are_7_7_6` (the first six frames' delays); `unchanged_pixels_are_transparent` (a segment of identical frames: every frame after the first has a rectangle no larger than the caption change and a transparent index); `the_gif_is_small` (flat-UI frames come under `max_mib`; a segment of noise frames fails, the error naming the frame that grew most); `the_gif_is_deterministic` (two builds, byte-equal); `a_segment_beat_must_exist` (an unknown beat errors naming vignette and beat); `a_missing_frame_is_named` (a gap errors naming `000002.png`); `tokens_resolve_aliases` (`accent` equals `agent`'s light value `#2f6f2a`); `the_caption_renders_with_no_diagnostics` (`Previewer::inspect` reports no missing font, no clipped text for "Realtime collaboration"). `config.test.ts`: `refuses a gif of one segment`, `refuses a gif over 10 s`, `refuses a gif segment naming no vignette`.
- [ ] **Step 2: Run** `bazelisk test //tools/reel-compose:reel-compose_test` and `just web-test`; the new tests fail.
- [ ] **Step 3: Implement** `tokens.rs`, `gif.rs` and the `gif` subcommand; the config parsing in Rust shares one fixture with `config.ts` (`tools/reel-compose/testdata/reel.toml`, shared through `//web/reel:reel.toml`). Source frames are scaled with a box filter (never up), so the render is deterministic.
- [ ] **Step 4: Run** the tests; they pass.
- [ ] **Step 5: Build the real one.** `just reel --gif` (background, log in `target/reel-gif.log`). Expected: `target/reel/reel.gif` under 2 MiB, `probe` 6–10 s, 720 px wide; open it in a browser and watch the three beats and the loop point. If it is over budget, shorten a segment or reduce `size` in `reel.toml`; do not cut the palette.
- [ ] **Step 6: Commit** `Render the README GIF from the vignettes' frames`.

### Task 6: `just reel-upload` for the GIF, and the landing artifact's config

**Files:**
- Create: `scripts/reel-upload.sh`, `scripts/test-reel-upload.sh` (against fakes of `clax`, `gh` and `reel-compose` on `PATH`, as `scripts/test-release-script.sh` fakes `gh`)
- Modify: `justfile` (`reel-upload *ARGS`), `scripts/BUILD.bazel` (`//scripts:reel_upload`, an `sh_test` in `just ci`: it runs no browser and no composer), `scripts/release.sh` (step 15's list gains `just reel-upload --release vX.Y.Z`), `scripts/test-release-script.sh` (that line), `web/quickstart/README.md` (the landing artifact is published from this directory by `just reel-upload`, which adds `reel.gif`; do not publish it by hand)

**Interfaces:**
- Consumes: `target/reel/{reel.gif,build.json}`, `web/quickstart/` (the files `clax publish index.html --dir web/quickstart` publishes), `web/reel/reel.toml` `[upload] landing_artifact` and `landing`, `~/.config/clax/reel/upload.toml` (`artifact = "<ID>"`, the local artifact linked to the landing artifact; `CLAX_REEL_CONFIG` overrides its path for tests), `reel-compose probe` (`CLAX_REEL_COMPOSE` names it; else `bazelisk run //tools/reel-compose:reel-compose --`).
- Produces: `target/reel/manifest.json` (spec §10); a version of the local artifact holding `web/quickstart/`'s files plus `reel.gif` and `reel-manifest.json`, label `Reel <short commit>`, staged in a scratch directory by copying (never linking); a push; with `--release vX.Y.Z`, release assets `clax-reel.gif` and `clax-reel-manifest.json`. No MP4 is uploaded in this phase.

- [ ] **Step 1: Write the failing test** `scripts/test-reel-upload.sh`, one case per line, each asserting the fakes' call logs: `declined` (stdin `n`: no `clax` or `gh` call after the summary, exit 1); `dirty_build` (`build.json` `dirty: true`: refuses before asking); `stale_build` (`HEAD` differs: refuses, says `just reel --gif`); `worktree_clax` (`clax` on `PATH` resolves under the checkout: refuses); `wrong_link` (the fake's remotes `url` differs from `landing`: refuses, no upload); `empty_landing` (`landing` or `landing_artifact` empty: refuses naming the `[upload]` keys); `landing_mismatch` (`landing` not ending in `landing_artifact`: refuses); `stages_quickstart_with_the_gif` (stdin `y`: the published directory lists exactly `web/quickstart/`'s published files plus `reel.gif` and `reel-manifest.json`, none a link); `uploads` (calls in order: a publish with the label, `push <ID>`; no `asset upload`; `manifest.json` holds `commit`, `date`, `clips` with one SHA-256 per vignette frame set and `outputs`); `summary` (the printed summary names `reel.gif`'s size in MiB with one decimal, the commit and subject); `release` (`--release v1.2.3`, `y` twice: `gh release upload v1.2.3` with the two names and no `--clobber`); `release_has_assets` (the fake lists them: refuses); `no_video_without_flag` (no `reel.mp4` is named in any call).
- [ ] **Step 2: Run** `bazelisk test //scripts:reel_upload`; it fails.
- [ ] **Step 3: Implement** `scripts/reel-upload.sh` (`set -euo pipefail`; `read -r` from the terminal; JSON through `python3 -I -c`; the publish through the CLI's own flags, checked with `clax publish --help` first; the frame-set hashes from the directories `just reel --gif` copied into `target/reel/frames-sha/`).
- [ ] **Step 4: Run** the test; it passes. `just ci`'s script gates pass (`scripts/check-pipefail-pipes.sh`, the justfile test).
- [ ] **Step 5: Commit** `Upload the README GIF to the landing artifact, and to a release, only on a yes`.
- [ ] **Step 6: Hand the first publish to the owner.** The owner publishes the Quickstart (`clax publish index.html --dir web/quickstart`, then `clax push <ID>`), writes the local artifact ID to `~/.config/clax/reel/upload.toml`, and gives the clax.dev artifact ID and URL. A follow-up commit sets `[upload] landing_artifact` and `landing` in `web/reel/reel.toml` (spec D8). Do not publish or push anything to clax.dev from this task.

### Task 7: The README GIF and the docs

**Files:**
- Modify: `README.md` (the linked GIF of spec §11.2 below the opening paragraph, once `[upload]` is set and the first `reel-upload` has run; until then this step waits and the task commits the rest), `docs/usage.md` ("Developing Clax": a paragraph "The sizzle reel": `just reel --gif`, `--fast`, `<id>`, `just reel-upload`, where outputs go, that nothing is committed and that the landing artifact is published only by `reel-upload`), `docs/bazel.md` ("Targets": the reel's targets and that the composer's build is not hermetic)

- [ ] **Step 1:** After the first `reel-upload` (the owner's), check that `curl -sI <landing site>/reel.gif` answers 200 `image/gif` without a cookie (spec R7), then add the README block with the alt text of spec §11.2.
- [ ] **Step 2:** `just check` once before the branch merges; `qualifier diff main`.
- [ ] **Step 3: Commit** `Show the reel's GIF in the README and document the reel`.

## Phase 2: the full video, as a build output

### Task 8: `clip`: a vignette's frames into an MP4

**Files:**
- Create: `tools/reel-compose/src/clip.rs`
- Modify: `tools/reel-compose/src/probe.rs` (MP4), `tools/reel-compose/src/main.rs` (`clip`), `web/reel/reel.bzl` (`<id>_clip`)

**Interfaces:**
- Consumes: Task 3's crate; Task 2's `<id>_frames`.
- Produces: `reel-compose clip <frames-dir> --fps 30 -o <out.mp4>`: x264 `crf 12`, `preset slow`, `tune animation`, yuv420p, BT.709, no audio (D10). `pub fn clip(frames: &Path, fps: usize, out: &Path) -> anyhow::Result<()>`; `probe` reads MP4.

- [ ] **Step 1: Write the failing tests** (`#[cfg(test)]` in `clip.rs`): `clips_ten_frames_into_a_third_of_a_second` (ten 160 × 100 PNGs of distinct flat colours → `probe` gives `duration_s` within 0.34 ± 0.04, `width 160`, `height 100`); `refuses_a_gap_in_the_frames` (frames 0, 1, 3 → error naming `000002.png`); `refuses_mixed_sizes` (error naming the odd frame); `has_no_audio_track`.
- [ ] **Step 2: Run** `bazelisk test //tools/reel-compose:reel-compose_test` and confirm it fails (no `clip`).
- [ ] **Step 3: Implement** `clip` as a fframes `Video` whose `render_frame` returns one `<image>` of frame `n`, the frames registered by `DynamicMediaProvider::with_image` for one vignette at a time, rendered with `fframes::render`. Peak memory for a 240-frame 1600 × 1000 vignette must stay under 3 GB (`/usr/bin/time -l`); if it does not, register frames in chunks of 60 and concatenate the chunk clips, and say so in the commit message.
- [ ] **Step 4: Run** the tests; they pass.
- [ ] **Step 5: Wire** `<id>_clip` into `reel.bzl` (a rule running `reel-compose clip` on `<id>_frames`) and run `bazelisk build //web/reel:realtime_clip`; `probe` of the output gives about 7 s at 1600 × 1000.
- [ ] **Step 6: Commit** `Encode a vignette's frames into a clip with x264`.

### Task 9: Cards from the design tokens

**Files:**
- Create: `tools/reel-compose/src/cards.rs`
- Modify: `tools/reel-compose/src/tokens.rs` (Task 5's; gains `Tokens` fields the cards need, none removed)

**Interfaces:**
- Consumes: Task 5's `Tokens`, `Motion` and `Tokens::load`.
- Produces: `pub fn title_card(t: &Tokens, mark: &Mark, title: &str, progress: f32) -> String` (an SVG document 1920 × 1080; `progress` 0..1 draws the underline), `pub fn end_card(t: &Tokens, mark: &Mark, headline: &str, footer: &str) -> String`, `pub fn window_frame(t: &Tokens, title: &str) -> String` (the frame without its content, the content box at a fixed `CONTENT: Rect = { x: 160, y: 62, w: 1600, h: 1000 }` minus the 44 px bar: content placed at y = 62 + 44 is checked to fit 1080), `pub fn title_fits(title: &str) -> anyhow::Result<()>` (width ≤ 1600 px at Inter 600 88 px, by fframes' `estimate_text_width`).

- [ ] **Step 1: Write the failing tests:** `a_title_card_renders_with_no_diagnostics` (`Previewer::inspect` of a one-scene `Video` over `title_card(..., "Realtime collaboration", 1.0)` reports no missing font, no clipped text); `a_title_card_is_the_bg_colour_at_its_corner` (pixel (5, 1075) of the rendered frame is `bg` `#f7f7f5` ± 1); `an_overlong_title_is_refused` (`title_fits("Realtime collaboration across every device you own today")` errs naming the title); `the_end_card_names_clax_dev`.
- [ ] **Step 2: Run** `bazelisk test //tools/reel-compose:reel-compose_test`; the new tests fail.
- [ ] **Step 3: Implement** the cards per spec §9.2 with `svgr!`; the mark from `design/mark.json` (its `you` and `agent` paths and `dot`, stroked in `you` and `agent`); fonts registered from `@inter_font` only (`load_system_fonts: false`).
- [ ] **Step 4: Run** the tests; they pass.
- [ ] **Step 5: Commit** `Draw the reel's title, end and window cards from design/tokens.json`.

### Task 10: The composition, the poster and `//web/reel:reel`

**Files:**
- Create: `tools/reel-compose/src/reel.rs`, `tools/reel-compose/src/poster.rs`; `config.rs` is Task 5's, extended here with the vignette, cut and `[end]` keys
- Modify: `tools/reel-compose/src/main.rs` (`compose`), `web/reel/reel.toml` (all nine vignettes of spec §3, titles and seconds), `web/reel/BUILD.bazel` (`reel`, `reel_fast`; outputs `reel.mp4`, `reel-45.mp4`, `reel-poster.png`), `scripts/run.sh` (`reel` with no ID builds `//web/reel:reel`; writes `target/reel/build.json`), `scripts/run.test.sh`

**Interfaces:**
- Consumes: Task 3's `probe`; Task 8's `clip`; Task 9's cards; Task 5's `config.rs`.
- Produces: `reel-compose poster --config reel.toml --clips <dir> -o reel-poster.png` (the full reel's frame at `[poster]`, 1920 × 1080, through `Previewer::render`).
- Produces: `reel-compose compose --config reel.toml --cut full|short --clips <dir> --tokens <tokens.json> --mark <mark.json> --font-dir <dir> -o <out.mp4>`; `pub fn plan(cfg: &ReelConfig, cut: Cut) -> Vec<Scene>` with `pub enum Scene { Clip { id: String, seconds: f32, title: Option<String> }, Card { title: String, seconds: f32 }, End { seconds: f32 } }`. `ReelConfig` reads the same keys and refusals as Task 1's `parseReel` (one fixture file, `tools/reel-compose/testdata/reel.toml`, shared by both test suites through `//web/reel:reel.toml`).
- Produces: `target/reel/build.json` (written by Task 5's `--gif` run too; this task extends it to the video) = `{"commit": "<git rev-parse HEAD>", "dirty": <git status --porcelain -- web/reel tools/reel-compose design web/e2e is non-empty>, "built_at": "<UTC ISO date>"}`.

- [ ] **Step 1: Write the failing tests:** `the_full_cut_runs_cold_open_cards_vignettes_end` (`plan(full)` starts `Clip cold-open` with no card, alternates `Card`/`Clip` for the eight titled vignettes in `reel.toml` order, ends `End`); `the_short_cut_keeps_in_short_vignettes` (`point, version, realtime, history, share`, cards of 1.0 s, cold open 5 s); `cut_lengths_are_in_range` (full in [60, 80] s, short in [40, 50] s); `config_refuses_what_parse_reel_refuses` (the Task 1 refusal cases, same messages). `the_poster_is_the_named_frame` (pixels at the content box's centre match the clip's frame at `at` ± 3 per channel). A manual composition test `composes_three_flat_clips` (three 2 s clips of flat colours, one card each, `probe` ≈ 2+1.2+2+1.2+2+4 s ± 0.1).
- [ ] **Step 2: Run** them; they fail.
- [ ] **Step 3: Implement** `plan` and the fframes `Video` over it: `define_scenes` from `plan`; each `Clip` scene draws `window_frame` and the clip's synced frame (`get_synced_video_frame`, the clip registered by `with_video`) into `CONTENT`; transitions per spec §9.1; final encode per spec §8.1 (`crf 20`, faststart). A missing clip fails naming its ID. Vignettes not yet scripted (Tasks 11–14) render as a card "<title>, not recorded yet" so the reel composes from this task on, and `compose` prints a warning per such vignette.
- [ ] **Step 4: Run** the tests; they pass. Then `just reel` (background, log in `target/reel.log`). Expected: `target/reel/reel.mp4`, `reel-45.mp4` and `reel-poster.png` (1920 × 1080), `probe` durations in range, `reel.mp4` under 18 MiB, `build.json` written.
- [ ] **Step 5: Commit** `Compose the reel from its clips and cards, with its poster: bazel build //web/reel:reel and just reel`.

### Task 11: The `cold-open` and `history` vignettes

**Files:** Create `web/reel/vignettes/cold-open.ts`, `web/reel/vignettes/history.ts` (seed: `seedLaunchPlan(..., 6)` from Task 4 with labels and notes per version). Modify `web/reel/BUILD.bazel`, `web/reel/reel.toml`.

- [ ] **Step 1:** `cold-open` per spec §3 (beats `published`, `card`, `opened`; consumes the Task 4 helpers), then `history` per spec §3 (beats `versions`, `scrub`, `restored`), reaching the versions panel and its scrubber by role as the shell's own tests name them (read `web/shell/src` for the panel's accessible names first). `just reel --fast <id>` and watch each: each beat on screen, the cursor never still for more than 1.5 s outside a `hold`.
- [ ] **Step 2:** `bazelisk build //web/reel:<id>_clip` for each; `probe` durations within 0.5 s of `reel.toml`'s seconds.
- [ ] **Step 3: Commit** `Record the cold open and the Every version kept vignettes`.

### Task 12: The `devserver` vignette

**Files:** Create `web/reel/vignettes/devserver.ts`, `web/reel/stage-extension.ts` (`extensionStage(stage): Promise<{ ctx: BrowserContext; siteUrl: string; daemon: Daemon }>` reusing `launch`, `freePort` and the install steps of `web/e2e/extension-fixtures.ts`; export what it needs from there rather than copying). Modify `web/e2e/extension-fixtures.ts` (export the launch and install helpers), `web/reel/reel.bzl` (srcs gain `//web:dist_extension_test`).

- [ ] **Step 1:** Write it per spec §3 (beats `overlay`, `pointed`, `sent`, `reply`) on the new-headless context the fixture launches, at the §5 viewport and scale.
- [ ] **Step 2:** `just reel --fast devserver`, watch; then `bazelisk build //web/reel:devserver_clip`. If the deterministic camera cannot drive this context (spec §7.2), make `reel_vignette(name = "devserver", mode = "fast")` build `<id>_clip` from `<id>_fast` (`reel-compose clip` accepts a video file as well as a frame directory), and record the mode for the manifest in `web/reel/reel.toml` (`mode = "fast"` on that vignette, a key `config.ts` and the composer accept).
- [ ] **Step 3:** `bazelisk test //web:e2e_chrome_overlay` still passes (the fixture's exports changed).
- [ ] **Step 4: Commit** `Record the dev server vignette with the Clax extension`.

### Task 13: The `more` and `claxbot` vignettes

**Files:** Create `web/reel/vignettes/more.ts`, `web/reel/seed/metrics.html`, `web/reel/seed/data-server.ts` (a loopback HTTP server whose `/metrics` answers JSON the script sets; started and stopped through `stage.onStop`), `web/reel/vignettes/claxbot.ts`, `web/reel/seed/launch-day.html`. Modify `web/reel/BUILD.bazel`.

- [ ] **Step 1:** `more` per spec §3 (beats `asks`, `approved`, `live`): `publishWith` declaring `net` for the data server's origin and `db`; the camera approves in the approval bar by its accessible name (the shell's, as `web/e2e/permissions.spec.ts` reaches it), never through `approve()`.
- [ ] **Step 2:** `claxbot` per spec §3 (beats `sent`, `steps`, `new-version`): `startDaemon` with `[bot] base_url` at `fakeMessages([...])`'s address as `claxbot-daemon.spec.ts`'s `botDaemon`, three turns whose last publishes the countdown version.
- [ ] **Step 3:** `just reel --fast more`, `just reel --fast claxbot`, watch; build both `_clip`s; durations within 0.5 s.
- [ ] **Step 4: Commit** `Record the Pages that do more and ClaxBot vignettes`.

### Task 14: The `share` vignette

**Files:** Create `web/reel/vignettes/share.ts`, `web/reel/stage-linked.ts` (a local daemon signed in to a hosted one by device code, taken from `web/e2e/linked.spec.ts`'s setup; move that setup into a helper both use rather than copying it). Modify `web/e2e/linked.spec.ts` (use the helper), `web/reel/BUILD.bazel`.

- [ ] **Step 1:** Write it per spec §3 (beats `publish`, `guest`, `comment-home`). The camera's window title is the page's title (Task 9 `window_frame`); no URL is drawn.
- [ ] **Step 2:** `just reel --fast share`, watch; build `share_clip`.
- [ ] **Step 3:** `bazelisk test //web:e2e_rest` passes (the moved setup).
- [ ] **Step 4:** `just reel`: every vignette recorded, no "not recorded yet" card, `reel.mp4` under 18 MiB, durations in range.
- [ ] **Step 5: Commit** `Record the Share to clax.dev vignette; the reel is complete`.

## Phase 3: publishing the video (later; do not start until the owner asks)

### Task 15: Byte ranges on assets

**Files:**
- Modify: `crates/clax-server/src/routes/assets.rs` (`serve_blob`, and a `fn byte_range(header: Option<&HeaderValue>, len: u64) -> Range` with `enum Range { Whole, Part(u64, u64), Unsatisfiable }`), `docs/contract.md` (where `/_clax/blob/` is described: ranges)
- Test: `crates/clax-server/tests/api_assets.rs`

**Interfaces:**
- Produces: `serve_blob` answers `Accept-Ranges: bytes` always; 206 + `Content-Range: bytes a-b/len` + `Content-Length: b-a+1` for one satisfiable range; 416 + `Content-Range: bytes */len` for an unsatisfiable one; 200 whole for none, several, or a malformed header. The other headers as today.

- [ ] **Step 1: Write the failing tests** in `api_assets.rs` (upload a 1000-byte asset of bytes `i % 251`, then `GET /_clax/blob/<id>` on its site): `a_range_gets_206_and_those_bytes` (`bytes=10-19` → 206, body bytes 10..=19, `Content-Range: bytes 10-19/1000`); `an_open_range_runs_to_the_end` (`bytes=990-` → 10 bytes); `a_suffix_range_takes_the_last_bytes` (`bytes=-5` → bytes 995..=999); `a_range_past_the_end_is_416` (`bytes=1000-` → 416, `Content-Range: bytes */1000`); `several_ranges_get_the_whole_asset` (`bytes=0-1,5-6` → 200, 1000 bytes); `every_answer_says_it_takes_ranges` (`Accept-Ranges: bytes` on the 200). Unit tests of `byte_range` for `bytes=5-2` (Whole), `items=0-1` (Whole), `bytes=0-99999` (Part(0, 999)).
- [ ] **Step 2: Run** `just test-affected`; the new tests fail.
- [ ] **Step 3: Implement.** The asset is still read whole before slicing (record `a7ee35dc`); reply to that thread that video playback makes it more frequent, citing this task's commit.
- [ ] **Step 4: Run** `just test-affected`; they pass.
- [ ] **Step 5: Commit** `Answer byte ranges on asset bytes, so Safari plays an asset video`.

### Task 16: The video on the landing page

**Files:**
- Modify: `web/quickstart/index.html` (the `<figure class="reel" id="reel" hidden>` after `.lede`; `const REEL_PAGE = ""` beside `CLAXDEV`, set from `reel.toml`'s `[upload] landing` (spec D8, set after the first publish); the loader), `web/quickstart/style.css` (`.reel`: full width of the header column, `radius` 10 px, `aspect-ratio: 16 / 9`, the link card), `web/quickstart/README.md` (`reel.json` is uploaded, never shipped; `web/quickstart/` stays the one canonical Quickstart)
- Test: `web/e2e/quickstart.spec.ts`

**Interfaces:**
- Consumes: `reel.json` of spec §10 (`video`, `poster`, `duration_s`).
- Produces: `#reel` shows a `video` whose `src` and `poster` are `reel.json`'s and a caption `Clax in <round(duration_s)> seconds`; without a usable `reel.json`, the link card `a.reel-link[href=REEL_PAGE]` when `REEL_PAGE` is set and the page is not on it, else `#reel` stays hidden.

- [ ] **Step 1: Write the failing tests** (each publishes the quickstart's files as `quickstart.spec.ts` does, with `REEL_PAGE` replaced in the published copy by `https://clax.dev/a/reeltest`): `no reel.json shows the link card`; `an unparsable reel.json shows the link card`; `a reel.json naming a missing asset shows the link card once the video errs` (the loader listens for the `video` element's `error` and swaps in the card); `a reel.json with an uploaded asset shows the video` (upload a small asset through `POST /api/artifacts/<id>/assets` with content type `video/mp4`, publish `reel.json` naming its `url`; expect `video[src$="<asset ID>"]` visible, the caption, no `autoplay` attribute); `the seeded quickstart release carries no reel.json` (`SHIPPED` in `web/scripts/build-quickstart.mjs` excludes it: a vitest in `build-quickstart.test.ts`).
- [ ] **Step 2: Run** `bazelisk test //web:e2e_rest --test_arg=quickstart` and `just web-test`; they fail.
- [ ] **Step 3: Implement** the figure and loader (`fetch("reel.json")` wrapped in try/catch; no other origin; `preload="metadata"`, `playsinline`, `controls`).
- [ ] **Step 4: Run** them; they pass. Reply to record `6e8c06e5` only if this change touches its lines (it should not).
- [ ] **Step 5: Commit** `Play the reel on the quickstart when it has one, and link to it from seeded copies`.

### Task 17: `reel-upload --video` and the release's MP4

**Files:**
- Modify: `scripts/reel-upload.sh`, `scripts/test-reel-upload.sh`, `docs/usage.md` (the paragraph gains `--video`)

**Interfaces:**
- Consumes: Task 6's script and fakes; `target/reel/{reel.mp4,reel-45.mp4,reel-poster.png}` from Task 10; Task 16's `reel.json` reader.
- Produces: with `--video`, `clax asset upload <ID> reel.mp4 reel-poster.png --json` before the publish, and the version also carrying `reel.json` (spec §10) so `push` uploads the assets once and rewrites their addresses; with `--release vX.Y.Z --video`, the release asset `clax-reel.mp4` too. The summary names `reel.mp4`'s size in MiB and its duration (`reel-compose probe`).

- [ ] **Step 1: Write the failing cases** in `scripts/test-reel-upload.sh`: `video_uploads_assets_first` (stdin `y`, `--video`: calls in order `asset upload <ID> reel.mp4 reel-poster.png --json`, a publish carrying `reel.json` naming the returned asset URLs, `reel.gif` and the manifest, `push <ID>`); `video_summary` (names size in MiB with one decimal and the duration); `video_release` (`--release v1.2.3 --video`, `y` twice: `gh release upload` names `clax-reel.mp4` as well, no `--clobber`); `video_over_budget` (an `reel.mp4` over 18 MiB: refuses before asking); `reel_json_is_valid` (the staged `reel.json` has `video`, `poster`, `duration_s`, `commit`, `date`).
- [ ] **Step 2: Run** `bazelisk test //scripts:reel_upload`; the new cases fail.
- [ ] **Step 3: Implement** them; the cases of Task 6 still pass unchanged.
- [ ] **Step 4: Run** the test; it passes. The first `--video` upload is the owner's.
- [ ] **Step 5: Commit** `Upload the reel's video to the landing artifact and the release, only on a yes`.

### Task 18: The weekly CI render and the video's docs

**Files:**
- Create: `.github/workflows/reel.yml` (`schedule: cron "0 6 * * 1"`, `workflow_dispatch`, Blacksmith Linux runner as `ci.yml` uses, actions pinned by SHA as in `ci.yml`, `apt-get install -y libx264-dev nasm clang`, `scripts/bazel-test.sh`'s setup action, `bazelisk build //web/reel:reel`, `actions/upload-artifact` of `bazel-bin/web/reel/reel/*` with `retention-days: 7`; `permissions: contents: read`)
- Modify: `docs/usage.md` ("The sizzle reel": `just reel`, the video, `--video`), `docs/bazel.md` (the video's targets)

- [ ] **Step 1:** `actionlint .github/workflows/reel.yml` passes (CI's own lint of workflows).
- [ ] **Step 2:** `gh workflow run reel.yml` is the owner's to run; do not run it. Check the workflow by reading it against `ci.yml`'s setup steps.
- [ ] **Step 3:** `just check` once before the branch merges; `qualifier diff main`.
- [ ] **Step 4: Commit** `Render the reel weekly in CI and document the video`.
