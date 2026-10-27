# The sizzle reel: Clax's real UI in about 70 seconds

Status: draft for the owner's review (2026-10-27).

## 1. Intent

A short video of Clax's real UI: a cold open, then six to eight vignettes
of six to eight seconds, each behind a one-line title card ("Realtime
collaboration", …), then an end card. About 70 seconds in full, and a
cut of about 45 seconds. It plays on the landing page (the Clax
Quickstart, `web/quickstart/`, published on clax.dev) and is shown in the
README.

The reel is a **build artifact**. Its inputs live in the repository: the
vignette scripts, `reel.toml`, the card templates and the composer. Its
frames, clips, MP4, GIF and poster are build outputs, never committed.
Uploading is a separate, explicit, confirming step.

## 2. Owner decisions (2026-10-27, binding)

- D1. A script against the **real UI** drives each vignette: a scratch
  daemon and real browsers, reusing `web/e2e`'s harness (`startDaemon`
  and the API helpers in `web/e2e/fixtures.ts`, the people helpers in
  `web/e2e/people-helpers.ts` and `web/e2e/hosted-helpers.ts`, and the
  shell's test clock `claxTestClock`, `web/shell/src/clock.ts`). Each
  vignette is one script. The agent side is scripted through the API
  (register a session, publish the next version, reply on cue). No model
  runs.
- D2. Deterministic: a fixed viewport, scale, theme and fonts, seeded
  content, an injected visible cursor for the local person (remote
  cursors are Clax's own live layers), and eased human pacing.
- D3. Two recording modes: a **fast** mode (Playwright `recordVideo`) for
  iteration, and a **deterministic** mode (step the clock, one screenshot
  per frame) for final renders.
- D4. Composition with fframes from Clax's design tokens: title cards,
  transitions and the end card. `reel.toml` lists vignettes, titles and
  durations.
- D5. `bazel build //web/reel:reel` (tagged `manual`, so never in `//...`
  or `just ci`); `just reel` copies the outputs to the git-ignored
  `target/reel/`; `just reel <vignette>` renders one. Bazel's cache
  re-renders only what an input changed.
- D6. `just reel-upload` shows size, duration and source commit, asks,
  then uploads, and writes a manifest (commit, date, per-vignette
  hashes). Destinations: (b) a clax.dev artifact that plays the video,
  updated in place so its URL is stable (primary), and (a) a GitHub
  release asset at release time.
- D7. Shown on the landing page and embedded in the README.

## 3. The vignettes

Each vignette is one script, `web/reel/vignettes/<id>.ts`, exporting a
`Vignette` (§6.2). "Camera" is the local person's browser, the one
recorded; the other browsers act but are not filmed. All copy on the
seeded pages is invented for the reel (a fictional team, "Northwind",
planning a launch); no real person or company appears.

| # | ID | Title card | In the 45 s cut | Seconds |
|---|---|---|---|---|
| 0 | `cold-open` | none | yes (5 s) | 6 |
| 1 | `point` | Point at anything | yes | 6.5 |
| 2 | `version` | A new version, in seconds | yes | 6.5 |
| 3 | `realtime` | Realtime collaboration | yes | 7 |
| 4 | `history` | Every version kept | yes | 6 |
| 5 | `devserver` | Comment on your dev server | no | 6.5 |
| 6 | `more` | Pages that do more | no | 6.5 |
| 7 | `claxbot` | ClaxBot builds a page | no | 7 |
| 8 | `share` | Share to clax.dev | yes | 7 |

Full reel: 6 + 8 × (1.2 s card + vignette) + a 4 s end card ≈ 70 s. The
45 s cut: the cold open trimmed to 5 s, five vignettes behind 1.0 s
cards, the same end card ≈ 46 s. Durations are `reel.toml`'s; the table
is the starting point.

- **`cold-open`** (local daemon). Seed: an empty gallery but for two
  pinned pages. Beat: an agent session (`registerSession`, harness
  `claude`) publishes "Launch plan" (`publishAs`); its card arrives in
  the gallery live (the gallery's stream), the cursor opens it, the page
  fills the frame. Payoff: an agent's page has a home, seconds after it
  was made.
- **`point`** (local). Seed: "Launch plan" v3 (a chart and a table).
  Beat: the cursor enters comment mode, hovers the chart's March bar
  (the comment area outlines it), types "Make March stand out" in the
  composer, chooses Send to agent. Payoff: a comment pinned to exactly
  what was pointed at, handed to the agent.
- **`version`** (local). Continues `point`'s seed with its thread sent.
  Beat: the agent sets its working record (`setWorking`, the working
  pill shows "working on 1 thread"), publishes v4 addressing the thread
  (`publishNext` with `addresses`), replies "Highlighted March in the
  accent colour" (`replyAsAgent`). The page swaps to v4 in place; the
  thread shows the reply. Payoff: the round trip, with no page reload.
- **`realtime`** (a hosted daemon, as `web/e2e/live-layers.spec.ts`
  runs it: on a local daemon every browser is the owner). People: Alex
  (camera, the owner), Mia and Sam (`memberOf` with `rightsOn(aid)`).
  Seed: a shared to-do page ("Launch checklist") declaring the `db`
  capability. Beat: Mia's and Sam's pointers move over the list (their
  own `page.mouse` moves; Alex sees the shell's arrows and name tags);
  Mia starts a comment and her draft shows typing in Alex's view; Sam
  ticks "Book the venue", and the tick appears in Alex's list. Payoff:
  several people on one page, live.
- **`history`** (local). Seed: "Launch plan" with six versions, each with
  a label and a note. Beat: the cursor opens Versions, scrubs back
  through v6 → v2 (the page changes under the scrubber), restores v2 as
  v7. Payoff: nothing an agent did is lost.
- **`devserver`** (local, the extension). Uses
  `web/e2e/extension-fixtures.ts`: a real Vite dev server on a copy of
  `web/e2e/live-site/` and Chromium with the unpacked test extension
  (`launchPersistentContext`, `channel: "chromium"`). Beat: the overlay
  opens on `localhost`, the cursor points at a button, comments
  "Rounder corners", sends it; the agent's reply arrives in the overlay.
  Payoff: comments on any page, including your own app. The extension
  runs in Playwright's Chromium (the `chrome-overlay` project does it
  today), but only in new headless (`channel: "chromium"`), which bears
  on the deterministic mode (§7.2).
- **`more`** (local). Seed: a "Launch metrics" page declaring `net` for
  a loopback data server the script starts (`net.spec.ts`'s pattern) and
  `db`. Beat: the approval bar asks; the cursor approves; the page's
  numbers fill in and one ticks up as the data server's value changes.
  Payoff: pages are apps, with permissions the person grants.
- **`claxbot`** (local). A daemon whose `[bot] base_url` names a scripted
  Messages API (`web/e2e/fake-messages.ts`, `fakeMessages` with
  `scriptTurn` and `finishTurn`), as `claxbot-daemon.spec.ts` starts it:
  no route is stubbed and no model runs. Beat: the cursor sends a thread
  on a bare "Launch day" page to ClaxBot ("Add a countdown to launch
  day"); the run panel follows its steps; the new version appears.
  Payoff: no agent installed, still a change.
- **`share`** (a local daemon and a hosted daemon standing in for
  clax.dev, signed in by device code as `web/e2e/linked.spec.ts`
  connects them). Beat: the cursor chooses Publish and pushes; offstage
  the hosted owner shares the public copy by link (`shareByLink`); a
  second browser (Priya, a guest, `guestOn`) opens it and comments; the
  comment arrives in the camera's thread list through the link. Payoff: share a page with
  anyone, and their comments come home. The composer's window frame
  shows the page's title, never a URL: the hosted daemon runs on
  `localhost`, and the frame must not claim it is clax.dev.

The vignette scripts reach the UI by role and label, as the e2e specs
do, so a renamed control fails a render loudly rather than filming the
wrong thing.

## 4. Where things live

```
web/reel/                      the recorder (TypeScript, the web pnpm project)
  reel.toml                    vignettes, titles, durations, cuts, look
  config.ts                    reads and checks reel.toml
  director.ts                  the beat API a vignette uses (§6.2)
  cursor.ts                    the injected cursor (§6.3)
  pace.ts                      easing and human timings
  camera.ts                    fast and deterministic capture (§7)
  record.ts                    entry point: one vignette → clip or frames
  vignettes/<id>.ts            one per vignette
  seed/                        seeded pages (HTML, data) the vignettes publish
  BUILD.bazel, reel.bzl        the Bazel targets (§8)
  *.test.ts                    vitest units for config, pace, director
tools/reel-compose/            the composer (Rust, a Cargo workspace of its own)
  Cargo.toml, Cargo.lock       fframes 1.3.0 and nothing from Clax
  src/main.rs                  subcommands: clip, compose, loop, poster, probe
  src/tokens.rs                design/tokens.json → card colours, type, motion
  src/cards.rs                 title card, end card, transitions (fframes Video)
  src/reel.rs                  the composition: scenes from reel.toml
  BUILD.bazel                  //tools/reel-compose:reel-compose
scripts/reel-upload.sh         the upload (§10), and its test
```

- `web/reel/` is part of the web pnpm project, so it imports
  `../e2e/fixtures.ts` and the helpers directly and uses the same
  `@playwright/test` and Chromium (`@chromium//:browsers`). It is its
  own Bazel package, so nothing in it is an input of `//web:dist`: an
  edit to a vignette rebuilds no web UI and no Rust. `web/package.json`'s
  `lint` and `web/tsconfig.json`'s `include` gain `reel`.
- `tools/reel-compose/` is a Cargo workspace of its own (its own
  `[workspace]` table, and listed in the root `Cargo.toml`'s `exclude`,
  as `crates/clax-worker` is), depending on no Clax crate. It is not a
  member of the root workspace, so `scripts/check-crate-layers.py`
  (which reads the root's `cargo metadata`) never sees it, `cargo
  nextest --workspace` and clippy over the workspace never build it, and
  the layering in `docs/usage.md` is unchanged. Its manifest forbids
  `unsafe_code` itself (`[lints.rust]`), and `scripts/check-no-unsafe.sh`
  is extended to scan `tools/` too: today it scans only `crates/`.
- Its outputs and the recorder's are Bazel outputs; `just reel` copies
  them into `target/reel/` (git-ignored already: `target/`).

## 5. The look

- **Canvas:** 1920 × 1080, 30 fps, light theme (`prefers-color-scheme:
  light` forced in every browser context). The camera's browser
  viewport is 1280 × 800 CSS pixels at device scale 1.25, so a capture is
  1600 × 1000 pixels; the composer places it in a window frame on the
  canvas (§9.2) and never scales it up.
- **Fonts:** the web UI uses the system font stack
  (`design/tokens.json` `font.web`), so the camera renders with the
  platform's fonts. Final renders are made on macOS; the manifest records
  the macOS and Chromium versions. A render on Linux (CI) is for review
  only and looks different. Chromium runs with
  `--font-render-hinting=none` and `--disable-lcd-text`. The cards use
  Inter (SIL OFL 1.1), fetched by Bazel by checksum (`http_archive` in
  `MODULE.bazel`), never committed and never a system font.
- **Colours and motion** come from `design/tokens.json`: `bg`, `fg`,
  `muted`, `agent` (the accent), `you`, `card`, `border`, the
  `radius` and `elevation` scales, and `motion.curves` (`standard`,
  `decelerate`, `accelerate`) with `t-enter` (300 ms) and `t-exit`
  (200 ms). The Clax mark is drawn from `design/mark.json`. A token
  change re-renders the cards and nothing else.
- **Seeded content:** fixed names (Alex, Mia, Sam, Priya; agent sessions
  named `planner` and `builder`), fixed page copy under `web/reel/seed/`,
  and a fixed wall clock (§7.1) so "2 min ago" reads the same each run.

## 6. The recorder

### 6.1 One vignette, one process

`node reel/record.ts --vignette <id> --mode fast|deterministic --out
<dir>` starts what the vignette asks for (a local daemon by
`startDaemon`, or a hosted one by `hostedDaemon`, each on a scratch
`CLAX_HOME` under the action's temporary directory and a port the system
picks, never `~/.clax` or 7480), launches Chromium from
`PLAYWRIGHT_BROWSERS_PATH`, runs the vignette, writes its output, and
stops every daemon it started by the handle `startDaemon` returned. It
needs `CLAX_E2E_BIN` and `CLAX_E2E_WEB_DIST`, as the e2e tests do; the web
UI is `//web:dist_e2e`, the build with the test clock, whose production
bundle differs from `//web:dist` only by that clock
(`web/scripts/bundle-size.mjs` checks the production one carries none).

Output: in fast mode `clip.webm` and `beats.json`; in deterministic mode
`frames/000000.png …` and `beats.json`. `beats.json` names each beat
with its first frame, so the composer can trim a vignette to a beat.

### 6.2 The director

A vignette is

```ts
export type Vignette = {
  id: string;
  setup(stage: Stage): Promise<Cast>;     // daemons, people, seeded pages
  play(d: Director, cast: Cast): Promise<void>;
};
```

and the `Director` is the only way a vignette touches time:

- `d.move(target: Locator, opts?)`: the injected cursor travels to the
  target's centre along an eased path while real `mouse.move` events
  follow it (hover states happen). Duration `pace.travel(px)`: 250 ms +
  0.6 ms per pixel, at most 900 ms, `motion.curves.standard`.
- `d.click(target)`, `d.type(target, text)` (about 14 characters a
  second with a deterministic jitter from the vignette ID),
  `d.press(key)`.
- `d.hold(ms)`: the camera keeps rolling.
- `d.until(cond: () => Promise<boolean>)`: waits for a state (as
  `expect.poll` does), never for time. In fast mode real time passes; in
  deterministic mode no frame is taken while it waits (§7.2).
- `d.beat(name)`: marks a beat in `beats.json`.
- `d.offstage(fn)`: runs API calls and other people's actions (the
  agent publishing, Mia typing); the camera keeps its time.

No vignette calls `setTimeout`, `page.waitForTimeout` or a sleep: AGENTS.md
forbids sleeps standing in for a condition, and pacing is the director's.

### 6.3 The cursor

`cursor.ts` is added to the camera page by `page.addInitScript`: a
closed shadow root on the top document holding an arrow (the system's
look, 20 px at device scale) with `pointer-events: none` and the highest
z-index, a soft press ring on click, hidden until the first move. The
director moves it; the page never does. It is the camera's alone: other
people's pointers are Clax's live layers, which the shell draws.

## 7. Recording modes

### 7.1 Both modes

- One Chromium, the pinned `@chromium//:browsers`; the camera's context
  has the viewport, scale and colour scheme of §5, `reducedMotion:
  "no-preference"`, locale `en-US`, timezone `UTC`.
- Wall time in the pages is fixed: Playwright's `page.clock.install({
  time: REEL_EPOCH })` with `REEL_EPOCH = 2026-11-03T09:41:00Z`, so
  relative times and dates on screen are the same each run.
- The shell's own timers (consent arming, the hint, presence reports)
  run on its test clock; the director advances it with `advance()` from
  `web/e2e/time.ts` where an e2e test would, never for pacing.

### 7.2 Fast and deterministic

- **Fast:** `recordVideo` at 1600 × 1000 on the camera's context.
  Real time; frames are whatever Chromium painted. For iterating on a
  script: seconds per vignette, not identical twice.
- **Deterministic:** the camera's page runs on Chromium's virtual time
  (`Emulation.setVirtualTimePolicy`, policy `pause`): JavaScript timers,
  `requestAnimationFrame` and CSS animations advance only when the
  recorder grants a budget. A frame is: grant 1000/30 ms of virtual time,
  wait for the page to settle (two animation frames), `Page.captureScreenshot`
  (PNG). Between frames the director's actions and offstage work run
  with virtual time paused; network answers and stream events are still
  delivered, so a `d.until` waits for real state without frames passing.
  The result is one PNG per frame, identical on the same machine and
  inputs (§12, R1).
- The `devserver` vignette needs the extension and so new headless; if
  virtual time does not drive its frames, that vignette renders in fast
  mode in the final reel too, and the manifest says so. Task 2 of the
  plan finds out.

## 8. Under Bazel

| Target (`//web/reel:…`) | What it is |
|---|---|
| `<id>_fast` | the vignette in fast mode: `clip.webm`, `beats.json` |
| `<id>_frames` | the vignette in deterministic mode: `frames/`, `beats.json` |
| `<id>_clip` | `<id>.mp4`: the frames encoded by `reel-compose clip` |
| `reel` | `reel.mp4`, `reel-45.mp4`, `reel.gif`, `reel-poster.png` from every `<id>_clip` and `reel.toml` |
| `reel_fast` | the same from every `<id>_fast` |

- Every target is tagged `manual`, so `//...`, `just ci` and `just
  test-affected` never build one. The recorder targets are also
  `no-sandbox` (as the browser tests are, `docs/bazel.md` "Browser
  tests"), `no-remote` and `cpu:4`.
- Inputs of `<id>_frames`: the vignette, `web/reel/*.ts`, `web/reel/seed/`,
  `reel.toml`, the `web/e2e` helpers it imports, `//crates/clax-cli:clax`
  and `:clax_warm`, `//web:dist_e2e`, `//web:dist_extension_test`
  (the `devserver` vignette), `:node_modules` and `@chromium//:browsers`.
  So a Rust or web UI change re-renders every vignette, which is right
  (the UI is the subject); a vignette edit re-renders that vignette and
  the composition; a card or token edit re-composes without
  re-recording.
- `bazel/js/defs.bzl`'s `npm_script_build` runs a script of the package
  it is called in; it gains `project` (default: the calling package) and
  `skips`, so `//web/reel` targets run `web/package.json`'s
  `reel:record` with the binary and the browsers read in place, as
  `npm_script_test` does.
- The composer builds with a second crate_universe hub,
  `@reel_crates`, from `tools/reel-compose/Cargo.toml` and its own
  `Cargo.lock`. **This is not hermetic**, and says so: fframes reaches
  FFmpeg through `ffmpeg-sys-fframes` 9.0.0, whose build script
  downloads a prebuilt static FFmpeg from that crate's GitHub releases
  (no checksum) or compiles FFmpeg from a git clone (nasm and clang
  needed), runs bindgen against libclang, and with the `h264` feature
  links the system's x264 (Homebrew `x264`; `libx264-dev` on Linux). Its
  build-script action is tagged `requires-network`, `no-sandbox`,
  `no-remote`, and gets `LIBCLANG_PATH` and `FFMPEG_BINARIES_CACHE`
  (under `~/.cache/clax-reel/ffmpeg`, outside every worktree). Once
  built, the composer is cached like any output, in the cache every
  worktree shares.
- **fframes can composite video clips.** fframes 1.3.0
  (<https://crates.io/crates/fframes/1.3.0>, API at
  <https://docs.rs/fframes/1.3.0/fframes/>) renders each frame from an
  SVG tree (`Video::render_frame` → `svgr!`), places scenes back to back
  (`Video::define_scenes`), and embeds a video file's frame in a scene
  through `Frame::get_synced_video_frame(ctx, name,
  &SyncVideoFrameInput)` → `FFramesSyncedVideoFrame::into_image`, with the
  file registered by `DynamicMediaProvider::with_video`. So one fframes
  `Video` is the whole reel: cards, transitions, the window frame and the
  clips, encoded once (`fframes::render`, `EncoderOptions` with
  `codec_params` `crf`, `preset`, `tune`). No ffmpeg command line is
  needed. Its `Previewer` renders single frames to PNG (the poster) and
  `Previewer::inspect` reports missing fonts and clipped text (a test).
- `just reel` builds `//web/reel:reel` and copies its files into
  `target/reel/` with `build.json` (the commit `git rev-parse HEAD`, and
  whether the reel's inputs were dirty); `just reel <id>` builds
  `<id>_clip` (and `--fast`, `<id>_fast`) and copies it; `just reel
  --fast` builds `reel_fast`. All go through `scripts/run.sh`; without
  Bazel the recipe refuses and says the reel needs Bazel (no native
  twin: the recorder's native run would need `pnpm install` in the
  worktree).

### 8.1 The composer

`reel-compose` subcommands, each a pure function of its inputs:

- `clip <frames-dir> --fps 30 -o <id>.mp4`: a fframes `Video` whose
  frame `n` is the vignette's PNG `n`, x264 at `crf 12`, `preset slow`,
  `tune animation`: near-lossless, so composition re-encodes once.
- `compose --config reel.toml --cut full|45 --clips <dir> --tokens
  design/tokens.json --mark design/mark.json -o reel.mp4`: the reel.
  Final encode H.264 High, yuv420p, BT.709, `crf 20`, `preset slow`,
  `tune animation`, faststart, no audio track. Budget: under 18 MiB
  (the asset limit is 20 MiB, `asset_too_large`).
- `loop --clip <clips>/realtime.mp4 --from <beat> --seconds 8 -o reel.gif`:
  the README loop, 720 × 405, 12 fps, a caption bar, palette quantised
  by the `gif` crate (NeuQuant). Budget: under 5 MiB.
- `poster --config reel.toml --clips <dir> -o reel-poster.png`: the
  frame `reel.toml` names (`[poster] at`).
- `probe <file>`: JSON `{duration_s, width, height, bytes}`, for
  `reel-upload` and tests.

### 8.2 CI

A workflow of its own, `.github/workflows/reel.yml`: weekly
(`schedule`) and on demand (`workflow_dispatch`), on a Linux runner,
`apt-get install libx264-dev nasm clang`, then `bazelisk build
//web/reel:reel` and `actions/upload-artifact` with `retention-days: 7`.
It uploads nothing anywhere else and never gates a merge. Its fonts are
Linux's (§5): it is a check that the reel still renders, not a release.

## 9. The composition

### 9.1 Structure

`[cold open] → ([card] → [vignette])× → [end card]`. Transitions: a card
enters over the outgoing vignette's last 300 ms (`t-enter`,
`decelerate`) and leaves by a 200 ms cross-fade into the next vignette
(`t-exit`, `accelerate`). No other effects.

### 9.2 The cards

- **Title card:** the `bg` colour, the Clax mark at 64 px in the top
  left corner, the title in Inter 600 at 88 px in `fg`, centred, one
  line (the composer fails a title wider than 1600 px rather than
  wrapping it), and an `agent`-coloured underline that draws across in
  `t-enter`.
- **Window frame** around each vignette: a `card` rectangle with
  `radius.lg`, `elevation.2`'s shadow, a 44 px title bar with three dots
  and the page title in `muted` (no URL; §3 `share`), centred on `bg`.
- **End card:** the mark at 128 px, "Your agents build pages. Clax gives
  them a home." (the quickstart's headline) in Inter 600 at 64 px, and
  "clax.dev" in `agent` at 40 px.

### 9.3 `reel.toml`

```toml
[reel]
fps = 30
size = [1920, 1080]
capture = { viewport = [1280, 800], scale = 1.25, theme = "light" }
card_seconds = { full = 1.2, short = 1.0 }

[[vignette]]
id = "cold-open"
seconds = 6
short_seconds = 5
# no title: the cold open has no card

[[vignette]]
id = "realtime"
title = "Realtime collaboration"
seconds = 7
in_short = true
# … one table per vignette, in reel order (§3)

[end]
seconds = 4
headline = "Your agents build pages. Clax gives them a home."
footer = "clax.dev"

[poster]
vignette = "realtime"
at = 3.5

[loop]
vignette = "realtime"
from = 1.0
seconds = 8

[upload]
landing = ""   # the clax.dev landing page's URL (open question Q1)
```

`config.ts` and the composer read the same file and refuse: an unknown
key, a vignette ID with no script, a title in a vignette marked
`cold-open`, a duration outside 3–10 s, a full reel outside 60–80 s, a
short cut outside 40–50 s.

## 10. Uploading

`just reel-upload [--release vX.Y.Z]` runs `scripts/reel-upload.sh`:

1. Reads `target/reel/build.json`. Refuses when the build's inputs were
   dirty, or `HEAD` is not the build's commit (rebuild with `just reel`).
2. Prints `reel.mp4`'s size in MiB, its duration (`reel-compose probe`),
   the source commit and its subject, and each destination; asks
   `Upload? [y/N]`; anything but `y` stops with nothing changed. There is
   no flag that skips the question.
3. **clax.dev (always).** Through the owner's installed Clax: the `clax`
   on `PATH`, refused when it resolves into a worktree or `bazel-bin`
   (AGENTS.md: no unmerged build on `~/.clax`). The local artifact is the
   one in `~/.config/clax/reel/upload.toml` (`artifact = "<ID>"`); the
   script checks with `GET /api/artifacts/<ID>/remotes` that its link's
   `url` is `reel.toml`'s `[upload] landing`, and refuses otherwise.
   Then: `clax asset upload <ID> reel.mp4 reel-poster.png --json`;
   publish a version carrying `reel.json` (below), `reel.gif` and
   `reel-manifest.json`, labelled `Reel <short commit>`; `clax push
   <ID>`, which uploads the assets the version names to the public copy
   once and rewrites their addresses (`docs/contract.md`, "Pushing").
   The public artifact, and so its URL, stays the same.
4. **The release (with `--release`).** `gh release upload vX.Y.Z
   clax-reel.mp4 clax-reel.gif clax-reel-manifest.json`, after asking
   again; refuses a release that already has those assets (no
   `--clobber`). `scripts/release.sh`'s last step ("what is left by
   hand") names this command.
5. Writes `target/reel/manifest.json` and prints the landing URL.

`reel.json`, the page's pointer to its video:

```json
{"video": "/_clax/blob/<asset ID>", "poster": "/_clax/blob/<asset ID>",
 "duration_s": 71.2, "commit": "<sha>", "date": "2026-11-03"}
```

The manifest: `{commit, date, macos, chromium, mode: {<id>:
"deterministic"|"fast"}, clips: {<id>: "<sha256>"}, outputs: {"reel.mp4":
"<sha256>", …}}`.

## 11. Where it is shown

### 11.1 The landing page

The page contract keeps a page to its own files and assets
(`docs/contract.md`, "Page contract"; the site's lockdown policy,
`crates/clax-http/src/content_policy.rs`, `default-src 'self' data:
blob:`), so the video is an **asset of the landing artifact**, served at
`/_clax/blob/<asset ID>` on its own site, never a URL elsewhere.

- `web/quickstart/index.html` gains, after the lede, a hidden
  `<figure class="reel">`. On load the page fetches its own
  `reel.json`. When it parses and names `video`, the figure shows a
  `<video controls playsinline preload="metadata">` with that `src` and
  `poster`, and the duration as a caption ("Clax in 70 seconds"). No
  autoplay.
- **Seeded copies** (the binary's quickstart, `clax_core::quickstart`,
  copied into every new gallery) never carry the video: `reel.json` is
  not in `web/scripts/build-quickstart.mjs`'s `SHIPPED`, assets are per
  artifact, and `Release::request` removes any path the release lacks.
  Their `reel.json` fetch is a 404 (harmless, as `ideas-data.js`'s
  already is), and the figure shows instead a link card, "Watch Clax in
  70 seconds on clax.dev", to `reel.toml`'s landing URL, written into
  the page as a constant beside `CLAXDEV`. On clax.dev itself before the
  first upload, the figure stays hidden.
- **Byte ranges.** Safari plays a `<video>` only from a server that
  answers `Range` with 206, and seeking needs it everywhere.
  `serve_blob` (`crates/clax-server/src/routes/assets.rs`) answers every
  request 200 with the whole asset. It gains single-range support:
  `Accept-Ranges: bytes` on every answer; `Range: bytes=a-b`, `a-` and
  `-n` → 206 with `Content-Range`; an unsatisfiable range → 416 with
  `Content-Range: bytes */<size>`; a multi-range or malformed header → the
  whole asset, 200. The hosted Worker serves assets through the same
  route. Each range still reads the whole asset into memory first, an
  existing concern (record `a7ee35dc`), which a video's many range
  requests make more frequent.

### 11.2 The README

GitHub plays inline video only for attachments uploaded through its own
editor; an `.mp4` linked from anywhere else is a link. The README shows
a **linked animated GIF**:

```html
<p align="center">
  <a href="<landing URL>"><img src="<landing site>/reel.gif" width="720"
     alt="Clax in 70 seconds: an agent publishes a page, a person points
     at a chart and comments, the agent ships a new version, and three
     people work on one page at once"></a>
</p>
```

below the opening paragraph. `reel.gif` is a **published file** of the
landing artifact, not an asset, so its address
(`https://<landing ID>.<site domain>/reel.gif`, the current version's
file) never changes: each `reel-upload` replaces the GIF behind the same
URL, and the README is edited once, when the landing URL is known, never
per upload. GitHub's image proxy caches it; a new GIF can take hours to
show there. The release's `clax-reel.gif` is the copy that lasts with a
version. A GitHub user-attachment MP4 would play inline, but it is
uploaded only by hand in GitHub's editor, has no API, and its URL changes
with each upload, so the README would change each time (an alternative
the owner may still prefer, Q5).

## 12. Risks

- R1. **Virtual time and Clax's realtime UI.** The shell reacts to stream
  and WebSocket events; with virtual time paused, events must still be
  delivered and painted when the next budget is granted. If not, the
  deterministic mode falls back to Playwright's `page.clock` (timers,
  `requestAnimationFrame`, `Date`) and the CDP Animation domain (CSS
  animations paused and sought per frame). The plan proves it on
  `realtime` before any other vignette (Task 2).
- R2. **The composer's toolchain.** FFmpeg arrives unpinned (§8);
  x264 is GPL (`libav-agree-gpl`); a first build compiles FFmpeg when no
  prebuilt archive matches. The composer is a developer tool never
  shipped with Clax, so the GPL does not reach Clax's binaries or the
  video; the owner decides (Q2).
- R3. **Every UI change re-renders every vignette** under Bazel. A final
  render is minutes of Chromium per vignette; only `just reel` pays it,
  on demand.
- R4. **Fonts differ by machine** (§5). Final renders on macOS only.
- R5. **The landing page's assets grow** by one video per upload on
  clax.dev and locally: `push` uploads new assets and deletes none.
- R6. **A hosted daemon in `realtime` and `share`** shows the hosted
  shell's chrome where it differs from a local one (its sign-in name in
  the top bar).
- R7. **The image proxy and the site domain.** Camo must fetch
  `reel.gif` from the landing artifact's site without a cookie; a public
  page's files are served to anyone, but this is checked on the first
  upload, before the README links it.

## 13. Review Focus

- A vignette script that drifts from the UI (a renamed control) fails the
  render naming the locator, rather than filming a wrong screen.
- Two renders of one vignette in deterministic mode on one machine give
  byte-identical frames; the first divergence is the bug to find.
- `reel-upload` never reaches clax.dev without the person's `y`, never
  runs a worktree's `clax`, and never pushes to an artifact whose link
  is not the landing URL.
- A seeded quickstart never shows a broken video: no `reel.json`, a
  `reel.json` naming a missing asset, and a `reel.json` that does not
  parse all end in the link card or nothing.
- A Safari viewer of the landing page can play and seek the video
  (ranges, §11.1).

## 14. Open questions for the owner

- Q1. The landing page: which clax.dev artifact (its URL), and which
  local artifact of yours is linked to it? `reel-upload` and the README
  need both; until then `[upload] landing` is empty and `reel-upload`
  refuses.
- Q2. x264 (GPL, system library) in a composer that is never shipped,
  or macOS's VideoToolbox encoder (no GPL, macOS only, output not
  bit-identical across machines)?
- Q3. Silent, or a music bed? fframes mixes audio (`AudioMap`), but a
  track needs a licence; the draft is silent.
- Q4. Light theme only, or a dark-theme render too?
- Q5. README: the linked GIF (§11.2), or a GitHub user-attachment MP4
  uploaded by hand at each release?
