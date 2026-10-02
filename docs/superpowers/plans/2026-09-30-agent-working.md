# Echo redesign + agent working, changelog and batch send: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give Clax the Echo look the owner chose, and build three features in it.
- **Echo.** Plex Sans Condensed for structure and Plex Mono for what people and agents write, the Echo symbol, sentence case, plain-verb buttons, a red-orange Comment button with a 3px rule under the top bar, a gallery that floats what needs your eyes, a `?` sheet with the C keycap, haiku in the gallery footer and while an agent works, and two easter eggs ("rally of 10", the mark's halves meeting).
- **Agent working signal.** An agent says, through Clax, that it is working on an artifact and on named threads. The signal is set automatically when comment feedback reaches the agent, and explicitly through a new `working` tool. A heartbeat lapses it after 2 minutes. It shows in the top bar's roster and summary, on gallery cards, on thread cards and pins, and to the page (a Clax extension of the `comments` capability).
- **Version changelog.** Every version records the threads it addressed and a short note from the agent. The changelog shows as an "Addressed in vN" group at the top of the sidebar, as version-tagged history on each thread ("v3 alex commented · claude worked on it · v5 claude addressed it"), as a quiet dot on the version button, and in a version menu that reads as a changelog. Nothing covers or moves the artifact. What is new is decided per viewer.
- **Batch send.** The viewer ticks several threads, or presses "Send N unsent", and they go to one agent as one batch with an optional note. The agent receives them as one grouped delivery, led by the note, on every tier. Send goes to the agent you last sent to if it is live, else the most recently active live agent receiving the artifact's comments; with none live it goes untargeted, as today. A send that names an agent reaches only that agent, and later comments on the thread follow it.

**Architecture:**
- **Shell.** The Svelte 5 shell from `docs/superpowers/plans/2026-09-29-svelte-port.md`. View state lives in `ArtifactController` (`web/shell/src/view/artifact-controller.ts`). The logic lives in framework-free `view/*-model.ts` modules, and the components are `web/shell/src/ui/*.svelte` islands that read the controller's store with `fromStore` and `$derived`. Echo is a rewrite of `web/shell/src/theme.css` (tokens, two type voices, components), a new skeleton for the artifact view's top bar, and new components.
- **Time to usable.** Everything not needed for first paint loads by dynamic `import()`: the `?` sheet, the people panel, the version menu's panel, the Addressed group, the haiku, and the selection bar.
- **Working.** An in-memory registry in the daemon (`clax_core::working::Working`), keyed by (session, artifact), with an injected clock and a 2 minute heartbeat expiry. The daemon marks work whenever it hands feedback to a session, since every delivery tier runs through `take_feedback`. Changes go out as one SSE event, `working`, and ride along on `GET /api/artifacts` and `GET /api/artifacts/<id>`, which the bootstrap block embeds.
- **Changelog.** Persisted: a nullable `versions.note`, a `version_threads` link table, and `viewer_seen` (the latest version each viewer has viewed).
- **Participants.** Comments record their author's viewer public ID, which drives "threads you're in". @mentions are parsed from comment text. `viewer_threads` holds per-viewer looked-at marks. Sessions gain an opaque public handle (`agent_handle`), so the shell can name and target an agent without ever seeing a session ID. A viewer's per-artifact attention (addressed and not looked at, new version, new replies, open threads you're in) is computed in the daemon. The artifact view gets it in the bootstrap block, and the gallery fetches it once, beside the artifact list.
- **Presence.** In-memory, like working: here or away, plus an optional location, announced by the `presence` event.
- **Batch send.** One route and one store transaction (`send_batches`, `batch_threads`, `feedback.batch_id`), with an optional agent target. The grouping lives in the one payload renderer every tier already uses, `render_items`.

**Tech Stack:** Rust 2024 (axum 0.8, rusqlite, chrono, serde, rmcp); Svelte 5 (runes), TypeScript and Vite 6; Vitest 3 with jsdom and `@testing-library/svelte` (through `web/shell/src/test/svelte.ts`); `svelte-check`; Playwright (Chromium); Python 3 (scripts); TypeBox (Pi extension, Pi 0.73.1); IBM Plex Mono and IBM Plex Sans Condensed (SIL OFL 1.1, self-hosted WOFF2).

**Spec:**
- The design spec: `docs/superpowers/specs/2026-09-28-clax-design.md`. On the Svelte branch and on main after the port merges it has this name. Task 1 amends it and `docs/contract.md`.
- The design record: `docs/superpowers/specs/2026-10-01-echo-design.md` (Task 1) states the Echo design, the thread model, the three features' decisions and Q1–Q12 as contract. Where it and anything older disagree, it wins, and the spec and contract it amends state the details.
- The approved mockup: `.superpowers/sdd/2026-09-30-redesign/concept-3-echo/index.html` and its `shots/`, with `concepts.md` §7 "Echo v2" and `marks/1-echo.svg`.
- The feature decisions are in the design record (§4–§7). There is no changelog banner: no band over the page. The returning viewer's summary is in the top bar's summary line and on the version button's dot.

**Decisions Q1–Q12:** twelve design questions the brief did not settle, all decided on 2026-10-01. They are recorded as contract in `docs/superpowers/specs/2026-10-01-echo-design.md` §8 (Task 1), which also records the Echo design and the three features' decisions, and which committed docs cite in place of any working file. Each step that rests on one is marked **(decided: Qn)**, to show where the decision lands.

**Precondition:** the rename plan (`docs/superpowers/plans/2026-09-29-clax-rename.md`) and the Svelte port (`docs/superpowers/plans/2026-09-29-svelte-port.md`) are both merged to main (the port merged before this plan runs). Check before Task 1:

```bash
test -f web/shell/src/view/artifact-controller.ts && test -f web/shell/src/ui/TopbarIsland.svelte \
  && test -f web/shell/src/view/boot.ts && test -f web/scripts/bundle-size.mjs && test -f web/perf/bundle-budget.json \
  && test -f web/shell/src/view/skeleton.ts && test ! -e web/shell/src/artifact.tsx && ! grep -q '"preact"' web/package.json && echo ok
```

Expected: `ok`. Anything else means the port is not merged: stop.

## Global Constraints

- **Ports and homes.** Never bind or connect to port 7480 or 7481. Tests, smokes and browser checks start daemons with `--port 0` and a temporary `CLAX_HOME` (`mktemp -d`). Never read, write or delete the real `~/.clax`, `~/.clax-dev`, `~/.claude` or `~/.codex`, nor the home directory Clax used before its rename (spec D15). `scripts/smoke-codex.sh` reads `~/.codex/auth.json`, so no task runs it; Task 11 only edits it, for the person to run.
- **Agents stage; the controller commits.** An implementing agent never runs `git commit`, `git push`, `git rebase` or `git reset`. It stages with `git add` and explicit paths only, and ends the task with `git status --short`. Each task's last step gives the commit message the controller uses. The controller commits signed, with plain `git commit` (never `--no-gpg-sign`), and checks `git cat-file commit HEAD | grep -q '^gpgsig '`.
- **UI is verified in a browser.** Every task that changes what the shell shows ends with a browser step. It runs a scratch daemon (`--port 0`, temporary home), opens the changed routes in Playwright's Chromium, and saves screenshots in light and in dark, at 1440×900 and at 390×844, to `.superpowers/sdd/2026-09-30-redesign/build-shots/task-NN/` (`NN` is the task number). The agent looks at every screenshot and writes in its task report what it saw: the change is visible, it is styled like the rest of Echo, and nothing scrolls sideways at phone width. A passing test run is not verification for frontend work. `web/e2e/shots.spec.ts` (Task 2), with its scenes in `web/e2e/scenes.ts`, is the one script that takes them.
- **Words.**
  - In prose, comments, doc comments and commit messages, write "ID", never "id", except as a literal symbol in code.
  - Doc comments and commit messages describe the contract or the change. They never mention this plan, the conversation, the mockup's history, or the history of names.
- **The thread model.** The interaction model is comment threads, like PR comments or Google Docs comments. Product copy uses comment, thread, reply, resolve, addressed and outdated. There are no turns, no "whose move", no rounds, no "asks" and no "facts" about the artifact. History is shown as version-tagged annotations on threads. The rule covers product copy: what the shell shows people (`web/shell/src`, including the haiku). "Turn" in its harness-protocol sense (the Stop hook, "turn end", "end your turn") stays in technical text only: the spec, the contract, agent-facing tool descriptions, skills, hooks and doc comments. This gate, scoped to product copy, prints nothing after every web task:

  ```bash
  grep -rniE "your move|whose move|agents' move|'s move|\bround [0-9]|(your|their|whose|next) turn|nothing waits on you|settled\." web/shell/src --include=*.svelte --include=*.ts --include=*.json | grep -v '\.test\.ts:'
  grep -niE "\bturns?\b" web/shell/src/view/haiku.json
  ```

- **Voice.**
  - Buttons are plain verbs: Comment, Reply, Resolve, Send to claude, Clear.
  - Playful words appear only in status lines, hints and empty states.
  - Labels are sentence case in Plex Sans Condensed, never tracked capitals. `grep -rn "text-transform: *uppercase" web/shell/src` prints nothing after Task 2.
  - Haiku appear in the gallery footer and on the line while an agent works. Never in comment mode, never animated.
- **Time to usable does not regress.**
  - The port's gates hold unchanged: `npm run perf` (budgets in `web/perf/budget.json` for five measures: first paint, comment ready, frame paint, ready latency and cold ready latency; `enforceTargets` is on) and `node scripts/bundle-size.mjs` (`web/perf/bundle-budget.json`). Task 2 adds a `fonts` budget to the second.
  - A budget is never raised. If a gate fails, first lazy-load what first paint does not need. If it still fails, stop and report the numbers to the controller. Raising a budget is the owner's call.
  - Nothing new is fetched before the shell's first paint. Working state, changelog notes, participants and this viewer's attention ride on the artifact response and the bootstrap block.
  - Fonts use `font-display: swap`, are never preloaded, and are never render-blocking. There are at most three WOFF2 files: Plex Mono 400, Plex Mono 600, and Plex Sans Condensed 600.
- **Svelte.** Runes mode only: no `export let`, no `svelte/legacy`. `web/shell/src/caps/**`, `web/shell/src/view/**` and `web/bridge/**` import no `svelte`: `grep -rlE "from \"svelte" web/bridge web/shell/src/caps web/shell/src/view` prints nothing. Components render and forward events. Logic lives in `view/*-model.ts` and `ArtifactController`.
- **Lint and types.**
  - Rust edition 2024. `cargo clippy --workspace --all-targets -- -D warnings` and `RUSTFLAGS=-Dwarnings cargo check --workspace` pass.
  - `npm run lint` and `npm run typecheck` pass in `web/`. The typecheck is `tsc --noEmit && svelte-check --fail-on-warnings`, and a11y warnings count.
- **Frame modes.** Every e2e spec that opens an artifact runs in both frame modes, `subdomain` and `sandbox` (`for (const mode of ["subdomain", "sandbox"] as const)`).
- **Gates.** Every task ends with `bash scripts/quality_gates.sh; echo "exit=$?"` printing `exit=0`. Check the status itself, not only the last line of output.
- **Skills and tools.** The three skill copies stay word for word identical in their shared sections (`scripts/test-plugins.sh`). Tool blocks are regenerated with `python3 scripts/sync-skill-tools.py`, never edited by hand. A tool description is one string that appears verbatim in `plugins/pi/test/fixtures/contract.json`, `crates/clax-mcp/src/tools.rs` and `plugins/pi/src/clax.ts`.
- **What views may carry.**
  - No working view, participant view, presence view, thread, comment, event or page ever carries a session ID, a working directory or a PID. The exceptions are token-only: `GET /api/sessions/<id>/working`, and `GET /api/artifacts` and `GET /api/artifacts/<aid>` with the token. Until Task 15 those two also name sessions without the token (a known follow-up, fixed there).
  - Agents are named to the shell by `agent_handle` (`a_` and 22 lowercase hex digits), never by session ID.
  - Viewers are named by `public_id`, never by cookie.
  - A viewer's per-thread looked-at marks are served only to that viewer.
- **The page and store IDs.** The page never learns a thread's store ID. The capability hands it opaque handles only, and only for threads the page created in its current document.
- **The clock in Rust tests.** Rust tests never sleep on the wall clock to observe expiry. They use `ManualClock` and call `sweep` directly. This applies to working and to presence.

## Review Focus

1. **A stale "working" after the agent stopped.** Every path that ends work must clear it:
   - the reply to the last named thread;
   - a publish of the artifact by that session;
   - the Stop hook allowing the stop;
   - Pi's `agent_end`;
   - session end (PATCH, the reaper, the `SessionEnd` hook, the shim exiting);
   - artifact deletion.

   A path that is missed shows "working" for up to 2 minutes. Tests: `api_working_auto.rs`, and the working steps Task 11 adds to the comment-loop smoke.
2. **Renewal that never lapses.** The shim's 60 s session heartbeat, the Pi injection long-poll, `wait_for_feedback` polls and `codex queue` must not renew a record. Test: "a heartbeat and a wait poll do not renew".
3. **Session IDs leaking.** No working view, participant view, presence event, capability answer or thread view may carry `session_id`. Agent handles are random, not derived from the session ID. Tests: "working views carry no session ID", and "participants name agents by handle only" (Task 15).
4. **The capability leaking store IDs.** `working()` and `onWorking` name threads by the handles the page already holds. Test: "working() names only this document's own threads, by handle".
5. **The automatic changelog link.** A publish links the threads in the publishing session's record *before* the publish clears the record. Linking never resolves a thread. An agent resolve links to the current version only when the thread has no link yet.
6. **Attention is per viewer and private.**
   - A viewer's looked-at marks and attention are read only with that viewer's cookie, and are never in an event or in another viewer's response.
   - Seeing a thread clears "addressed, not looked at" and "new replies" for it, and nothing else.
   - The Addressed group does not empty itself under the viewer's eyes (decided: Q4).

   Tests: `api_attention.rs` and the e2e "attention across viewers".
7. **Echo's first paint.**
   - The Plex Sans Condensed file is `swap` and not preloaded, and the fallback face is metric-adjusted, so the swap barely moves the top bar.
   - The theme script runs before first paint, so there is no light flash in dark mode.
   - Everything lazy stays out of the entry's closure. Tests: the `fonts`, `gallery` and `artifact` budgets, `time to usable`, and the shots.
8. **The version menu replacing the native `<select>`.** Keyboard (Escape returns focus; rows are links in tab order), phone width (a full sheet under the top bar) and version switching through `ctl.chooseVersion` must match what the select did.
9. **The PostToolUse throttle.** `scripts/tool-hook.sh` must exit 0 and print nothing on every path, keep its stamp under the Clax home, and start `clax` at most once a minute per session. Test: `scripts/test-tool-hook.sh`.
10. **Batch atomicity, grouping and the target.**
    - A batch with any unknown, foreign or resolved thread, or an unknown agent handle, writes nothing.
    - A written batch wakes each target once.
    - With `to`, only that agent's session gets rows, and the thread's later comments follow it while it is live.
    - Tests: `api_batch.rs`, `api_push.rs` "a batch reaches codex as one queued message", and the hook, MCP and Pi goldens.
11. **Selection and keyboard state.**
    - Selection lives only in the controller and is pruned whenever threads change.
    - Shortcut keys never fire while focus is in a text field, and never reach into the frame (decided: Q6).
12. **No turn language anywhere.** The grep gate in Global Constraints, and the owner's model: comment threads with version-tagged history.

---

## File Structure

Rust:

| Path | Responsibility |
|---|---|
| `crates/clax-core/src/working.rs` | `Working` registry, `Clock`, `SystemClock`, `ManualClock`, `WorkingView`, `SessionWorking`, `clean_message`, `clean_line`, TTL and bounds (Task 7) |
| `crates/clax-core/src/events.rs` | `Event::Working` (Task 7), `Event::Presence` (Task 24) |
| `crates/clax-core/src/changelog.rs` | `clean_note`, `MAX_NOTE_CHARS`, `MAX_ADDRESSES`, `LinkSource` (Task 12) |
| `crates/clax-core/src/store/changelog.rs` | Links, notes, version seen marks (Task 12) |
| `crates/clax-core/src/store/attention.rs` | Comment authors, mentions, looked-at marks, attention, agent handles (Task 15) |
| `crates/clax-core/src/mentions.rs` | `@name` parsing (Task 15) |
| `crates/clax-core/src/presence.rs` | The in-memory presence registry (Task 24) |
| `crates/clax-core/src/store/migrations.rs` | Migration 10 (Task 12), 11 (Task 15), 12 (Task 20), 13 (Task 21) |
| `crates/clax-server/src/working.rs` | `announce`, `sweep_and_announce`, `mark_items`, `renew_for_tier` (Tasks 8–9) |
| `crates/clax-server/src/routes/working.rs` | Working routes, and the debug-build clock skew route (Task 8) |
| `crates/clax-server/src/routes/viewers.rs` | Seen routes (Task 13), looked-at and attention routes (Task 15), presence routes (Task 24) |
| `crates/clax-server/src/boot.rs` | The bootstrap carries `attention` (Task 15) |
| `crates/clax-server/tests/api_working.rs`, `api_working_auto.rs`, `api_changelog.rs`, `api_attention.rs`, `api_batch.rs`, `api_presence.rs` | Route tests |
| `crates/clax-mcp/src/tools.rs`, `client.rs` | `working` tool; `publish` `addresses`/`note` (Tasks 10, 14) |
| `crates/clax-hooks/src/events.rs`, `crates/clax-cli/src/commands/hook.rs` | `tool` hook event; the Stop hook ends the turn (Task 11) |
| `crates/clax-cli/src/commands/publish.rs` | `--note`, `--addresses` (Task 14) |
| `crates/clax-cli/src/commands/haiku.rs` | A parity test against the shell's haiku list (Task 6) |
| `crates/clax-core/src/store/batches.rs`, `crates/clax-core/src/feedback.rs` | Batch send, the grouped payload (Task 20) |

Web (the Svelte port's layout):

| Path | Responsibility |
|---|---|
| `web/shell/src/theme.css` | Echo tokens, the two type voices, base and component styles (Tasks 2–6, then each UI task appends its section) |
| `web/shell/public/_clax/fonts/ibm-plex-sans-condensed-latin-600.woff2` | The display face (Task 2) |
| `web/scripts/bundle-size.mjs`, `web/perf/bundle-budget.json` | The `fonts` budget (Task 2) |
| `web/e2e/shots.spec.ts`, `web/e2e/scenes.ts` | The screenshot script every UI task runs, and its scenes (Task 2) |
| `web/shell/src/ui/Mark.svelte`, `web/shell/public/_clax/mark.svg` | The Echo symbol and favicon; the halves meet on click (Task 3) |
| `web/shell/src/view/theme-model.ts`, `web/shell/src/ui/ThemeSwitch.svelte` | Follow the system, plus a light/dark switch (Task 3) |
| `web/shell/src/view/keys.ts`, `web/shell/src/ui/KeysSheet.svelte` (lazy) | The keyboard layer and the `?` sheet (Task 3) |
| `web/shell/src/view/skeleton.ts`, `web/shell/artifact.html`, `web/shell/index.html` | The Echo top bar skeleton and the theme script (Tasks 3–4) |
| `web/shell/src/ui/TopbarIsland.svelte`, `web/shell/src/ui/MoreMenu.svelte` | The top bar in Echo (Task 4) |
| `web/shell/src/view/history-model.ts` | Version-tagged thread history and the outdated test (Task 5) |
| `web/shell/src/ui/ThreadCard.svelte`, `Sidebar.svelte`, `Pins.svelte` | Mirrored messages, history line, outdated tag, pin states (Tasks 5, 16, 18) |
| `web/shell/src/ui/Gallery.svelte`, `GalleryCard.svelte`, `HaikuLine.svelte` (lazy), `web/shell/src/view/haiku.json` | The gallery in Echo (Tasks 6, 19) |
| `web/shell/src/view/working-model.ts`, `web/shell/src/ui/Roster.svelte`, `WorkingSummary.svelte`, `working-feed.svelte.ts` | Working in the top bar, cards and threads (Task 16) |
| `web/shell/src/caps/comments.ts`, `caps/host.ts`, `web/bridge/src/caps/comments.ts`, `web/bridge/src/capabilities.ts`, `web/contract/clax-extensions.d.ts` | `working()` and `onWorking(fn)` (Task 17) |
| `web/shell/src/view/changelog-model.ts`, `version-rows.ts` (lazy, with the panel), `web/shell/src/ui/AddressedGroup.svelte` (lazy), `VersionMenu.svelte`, `VersionPanel.svelte` (lazy) | Changelog in Echo (Task 18) |
| `web/shell/src/view/attention-model.ts` | Needs your eyes and card markers (Task 19) |
| `web/shell/src/view/batch-model.ts`, `send-target.ts`, `web/shell/src/ui/SelectionBar.svelte` (lazy), `SendButton.svelte` | Batch send, the selection bar and the agent picker (Task 23) |
| `web/shell/src/view/presence-model.ts`, `web/shell/src/ui/PeoplePanel.svelte` (lazy) | Presence and the people panel (Task 24) |
| `web/e2e/echo.spec.ts`, `working.spec.ts`, `changelog.spec.ts`, `attention.spec.ts`, `batch.spec.ts`, `presence.spec.ts` | Browser tests |

Plugins and scripts:
- `scripts/tool-hook.sh` and its copies `plugins/claude-code/scripts/tool-hook.sh` and `plugins/clax/scripts/tool-hook.sh`; `scripts/test-tool-hook.sh`.
- `plugins/pi/src/clax.ts`, `plugins/pi/src/client.ts`, `plugins/pi/test/clax.test.ts` and `plugins/pi/test/fixtures/contract.json`.
- `plugins/*/hooks/hooks.json`, `plugins/*/skills/clax/SKILL.md` and `plugins/*/README.md`.
- `scripts/test-plugins.sh`, `scripts/smoke-comment-loop.sh` and `scripts/smoke-codex.sh`.

---

## Design: Echo

### Principles

- The artifact is the star. Clax's chrome never covers or moves it. Pins and the comment-mode outline are the only things drawn over the page.
- There are two voices with fixed jobs. Clax's structure speaks in **Plex Sans Condensed 600**: titles, numerals, labels, buttons, group heads. What people and agents write, and all meta, is **Plex Mono**.
- There are two colours with fixed meanings, and they never take turns:
  - **red-orange** `--you` is people: their comments, pins, comment mode;
  - **green** `--agent` is agents: their notes, working, primary actions;
  - brown is ink;
  - pink is an accent only: selection and the comment tint.
- The two arcs of the Echo symbol are the two kinds of participant. People sit on the left and open toward the centre. Agents sit on the right and open toward it. The page, a brown dot, is between them. The same layout recurs in the mark, the top bar's roster and each gallery card's roster.

### Type (numbered after Univers)

| Style | Face | Size | Use |
|---|---|---|---|
| 68 Display | Plex Sans Condensed 600 | 40px (gallery), 30px (phone), 22px (version button) | version numerals |
| 67 Title | Plex Sans Condensed 600 | 20px (top bar), 17–20px (cards), 19px (group heads), 26px (gallery heads) | titles, group heads |
| 57 Label | Plex Sans Condensed 600 | 13–15px | buttons, chips, summary line 1 |
| 45 Mono | Plex Mono 400 | 12–13.5px | comments, replies, notes, meta, history |
| 65 Mono strong | Plex Mono 600 | as 45 | emphasis inside mono |

`font-synthesis: none` stops a faux bold. Nothing uses tracked capitals.

### Tokens

`theme.css` keeps the port's palette and contrast ratios, and adds these:

| Token | Light | Dark | Meaning |
|---|---|---|---|
| `--you` | `#ed5439` | `#ed5439` | people, pins, comment mode |
| `--on-you` | `#2f0b04` | `#2f0b04` | text on red-orange (5.06:1) |
| `--agent` | `#457d26` | `#8cc46b` | agents, working |
| `--agent-ink` | `#3d6f21` | `#8cc46b` | agent text on the grounds |
| `--accent-tint` | `#eef4ea` | `#22291a` | a quiet green ground |
| `--pink` | `#f9c8bf` | `#f9c8bf` | selection, accents only |
| `--grot` | `"IBM Plex Sans Condensed", "Plex Condensed Fallback", "Arial Narrow", sans-serif` | | 67/68/57 |
| `--mono` | the port's `--font` stack | | 45/65 |

### Components

- **Buttons.** Plex Sans Condensed 600 14px, sentence case, `min-height: 32px` (40px under `pointer: coarse`), a 1px `--border-strong` border, square corners.
  - `.primary` is green.
  - The pressed Comment button is red-orange (`--you` on `--on-you`).
  - `.ghost` has no border.
  - Icon buttons are 32×32 with a 16px glyph.
- **Comment mode.** The Comment button is pressed and red-orange, and a 3px `--you` rule runs under the top bar (`box-shadow: inset 0 -3px 0 var(--you)`). The C keycap shows on the Comment button only, and is hidden at phone width.
- **Top bar (60px).** From left to right:
  - the mark (a link to the gallery);
  - the title over the "published by" line;
  - the roster and summary (Task 16);
  - Comment with its C keycap;
  - Threads with its count;
  - the version button (`v5` in 68 Display at 22px, then `of 5 ▾` in mono);
  - a ⋯ menu (open raw, copy link);
  - the theme switch.

  At phone width (≤700px) only the mark, title, roster (one token per side) and Comment show. A Page | Threads switch sits at the foot.
- **Thread cards.**
  - People's comments carry a 3px `--you` rule on the left.
  - An agent's reply carries a 3px `--agent` rule on the right, its author line right-aligned. When the reply is linked to a version, it reads `<agent> · addressed in vN`.
  - One line of version-tagged history sits under a dashed rule.
  - An `outdated` tag sits in the header (Task 5).
  - The actions are Reply, Resolve, and `Send to <agent> ▾`.
- **Sidebar groups.** Each head is a 7×16 half-disc swatch, a 19px title and a muted count. The swatch is red-orange for Open, green for Addressed in vN, an outline for Detached, and a dot for Resolved. Detached and Resolved collapse into a tail. The groups are, in order:
  - **Addressed in vN** (Task 18), then **Open**;
  - **Detached**, which keeps the port's meaning: the anchor is gone;
  - **Resolved**.
- **Pins.**
  - Open: red-orange.
  - An agent is working on it: split red-orange and green.
  - Addressed in a version you have not looked at: white with a green ring and a `vN` flag.
  - Selected: a green ring.
  - Being written: dashed.
- **Gallery.**
  - The bar is 60px: the mark, `Clax` in 22px Title, `local artifacts · seen as <name>`, search, and the theme switch.
  - **Needs your eyes** comes first, then **Everything else**, with pinned cards first and then the most recent activity. Group heads are 26px Title over a 2px ink rule.
  - Each card leads with its version numeral in 68 Display. Under it are the title, `<agent> · <time>`, the markers (Task 19), and a footer with the roster and `seen vK`.
  - There are no thumbnails (decided: Q1).
  - The footer holds one haiku, a new one each visit.
- **Empty states.** The gallery with no artifacts shows the mark large with its halves apart: "When an agent publishes a page, it lands here." A sidebar with no open threads reads "Nothing open. Press C and click anything to comment on it."
- **Motion.**
  - The mark's halves meet over 300ms. The working dot breathes over 1.8s. A 2px green sweep runs along the top bar's bottom edge while an agent works.
  - Under `prefers-reduced-motion: reduce` nothing moves: no sweep, no breathing (the dot stays solid), and no transitions.
- **Keys** (Task 3). `?` opens the sheet, and Esc closes it or leaves comment mode. The keys are:
  - C: comment mode;
  - T: threads;
  - J and K: next and previous thread;
  - Enter: reply to the selected thread;
  - S: send it;
  - R: resolve it;
  - X: tick it;
  - Shift+S: send the ticked threads;
  - V: versions;
  - P: people.

  Keys act only when focus is in the shell and not in a text field (decided: Q6).
- **Easter eggs.**
  - "rally of 10": a muted chip on the gallery card of an artifact at v10, and once per viewer in the top bar summary when they first view v10 (decided: Q9).
  - Click the mark and its halves meet; click again and they part.

### Haiku

The shell shows the same ten haiku as `clax haiku` (`crates/clax-cli/src/commands/haiku.rs`). They live in `web/shell/src/view/haiku.json`, which is loaded by dynamic `import()` after first paint, and a Rust test keeps the two lists equal (Task 6). A haiku appears:
- in the gallery footer, a random one on each visit;
- on the line under an agent's working status, in the sidebar strip and the people panel. It is chosen by the working record's `key`, so it stays put while the record lives.

A haiku never appears in comment mode and is never animated.

## Design: participants, attention and presence

### Who is in a thread

A viewer is **in** a thread when any of these holds:
- they wrote a comment in it, so `comments.author_public_id` is their `public_id`;
- a comment in it @mentions them;
- they resolved it.

Comments written before migration 11 have no author ID and count for nobody. An @mention is `@` followed by a viewer's display name, matched case-insensitively at a word boundary. A name with spaces matches only when written in full (decided: Q3). `@agent` keeps its existing meaning (send to the agent) and names no viewer.

### Attention, per viewer, per artifact

| Field | Rule |
|---|---|
| `addressed` | IDs of open threads the viewer is in that are linked to a version (`version_threads`) after the viewer last looked at the thread |
| `new_replies` | IDs of threads the viewer is in with a comment by someone else newer than the viewer's last look at the thread |
| `open_in` | IDs of open threads the viewer is in |
| `seen` | `viewer_seen.seen_n`: the latest version the viewer has viewed, or null |
| `looked` | `{thread ID: looked_at}` for this artifact's threads (served only on the artifact view) |

- An artifact **needs your eyes** when any of these holds: `addressed` is non-empty, `seen` is non-null and less than `current_version`, or `new_replies` is non-empty. A never-viewed artifact (`seen` null) does not need your eyes for its version alone.
- **Looking** at a thread is its card being at least half visible in the sidebar for 1 second, or the thread being selected (by its card, its pin, or J and K) (decided: Q4). Looking writes `viewer_threads(viewer_id, thread_id, looked_at)`, which clears that thread from `addressed` and `new_replies`.
- **Viewing** the latest version at `/a/<aid>` (not a `/v/<n>` URL) writes `viewer_seen`. Resolving is a separate act, and it is never needed to clear anything.
- The Addressed in vN group is decided when the view loads, and again when a new version arrives. Looking at a thread writes the mark at once, so the gallery clears, but the thread stays in the group until the view is decided again (decided: Q4).

### Agents

A session gets `agent_handle` (`a_` and 22 lowercase hex digits from 11 random bytes) when it registers. The handle is never derived from the session ID. The **agents on an artifact** are its owner session, the sessions watching it, and the sessions that published any of its versions, as `{handle, harness, live}`, at most 10.
- `live` means a send can reach the agent: its session is live and is the owner or a watcher (`Store::live_agent` accepts exactly these).
- The list is ordered live first, then most recently active first. An agent's activity on the artifact is the newest of its versions of it, its comments on its threads (`comments.via_session_id`), and its watch's `created_at`, else its session's `started_at`.
- The shell names an agent by its harness (`claude`, `codex`, `pi`). When two agents on the artifact share a harness, each gains the first four hex digits of its handle (`claude 7f3a`).
- `participants.people[]` carries `seen`, the person's `viewer_seen` on the artifact: public, so the people panel can show it (decided: Q7).

### Send target

- Send, the selection bar and Send N unsent all go to one agent. The default is the agent this viewer last sent to on this artifact, kept in `localStorage` under `clax.sendTo.<artifact ID>`, if it is live. Otherwise it is the first live agent in the participants list, which is the most recently active live owner or watcher. With no live agent the default is none, and the shell sends without `to`.
- The `▾` caret lists the other live agents on the artifact. With one live agent there is no caret.
- The routes take an optional `to` (an agent handle). With `to`, only that agent's session gets rows, and it becomes the thread's target (`threads.target_session_id`, migration 13, Task 21). A `to` naming no live agent is 400 `unknown_agent`; the shell sends one only when it names a live agent from the list, so this is the error of an explicit, stale `to`.
- Without `to` (no live agent, `@agent` on a thread never sent, the capability's `sendToClaude`), rows go to the live owner and every live watcher, or wait untargeted when none is live, and the thread's target is cleared.
- A later viewer comment on a sent thread goes to the thread's target while that session is live; once it has ended, the comment goes as a send without `to`.

### Presence (decided: Q5)

- A viewer with the artifact open reports `here` while its tab is visible, and `away` when the tab is hidden or there has been no input for 5 minutes. A 30-second heartbeat keeps the report fresh. It may also report `where`: the anchor label of the thread it has selected, or of the composer it is writing in, at most 80 characters.
- The daemon keeps presence in memory, keyed by (artifact, viewer public ID). An entry lapses 90 s after its last report: it becomes "last here <time>" and is dropped after 10 minutes. Changes go out as the `presence` event, `{artifact_id, people: [{public_id, display_name, state, where, since}]}`.
- The roster shows here and away. The people panel shows the location. A per-person switch in the panel, "Share where I'm looking", stops sending `where`.
- Another viewer's last seen version (`seen vK`) is shown in the panel, from `participants.people[].seen` (Task 15). Their per-thread marks are never shown (decided: Q7).

## Design: the working record

### Why in memory

The record is kept in memory, in the daemon, and never written to SQLite:

- It is a claim about the present, true only while its session keeps renewing it, with a 2 minute lifetime. A restart kills every heartbeat that would renew it. A persisted row would be stale by definition after a restart and would need an explicit purge on start. An in-memory map starts empty, so "a daemon restart should not show stale work" holds by construction.
- It changes on every hook run and tool call (renewal). Writing that to SQLite would put a write transaction on the hot path of every tool call, behind the store's single connection mutex.
- Nothing needs its history. The changelog (Tasks 12–14 and 18) persists what matters, the threads a version addressed. It copies them out of the record at publish time.
- The session identity it needs (`session_id`, `harness`) is read from the persisted `sessions` table when the record is made.

### The record

| Field | Meaning |
|---|---|
| key (`session_id`, `artifact_id`) | One record per session per artifact |
| `key` (JSON) | A fresh ULID minted when the record is created; kept while it lives; never derived from the session |
| `harness` | `claude`, `codex` or `pi`, from the session row |
| `message` | Optional, at most 140 characters after whitespace is collapsed and control characters dropped; longer text is cut to 139 characters plus `…` |
| `thread_ids` | Optional, at most 20 open threads of the artifact, in the order first named |
| `started_at` | When the record was created (RFC 3339, milliseconds, UTC) |
| `last_heartbeat` | The latest renewal; the record lapses at `last_heartbeat + 120 s` |

### What sets, renews and clears it

| Event | Effect |
|---|---|
| Feedback handed to session S on artifact A by tier `piggyback`, `stop_hook`, `prompt_hook`, `wait`, `inject` (daemon, inside the feedback route) | mark: create or renew (S, A), add the items' thread IDs |
| `codex queue` exits 0 for S's claimed rows (daemon, `push::dispatch`) | mark, as above |
| `working` tool / `PUT /api/sessions/<S>/working/<A>` | set: create or renew; `message` and `thread_ids` replace the stored ones when given |
| A feedback take by tier `piggyback`, `stop_hook` or `prompt_hook`, even an empty one | renew every record of S |
| `POST /api/sessions/<S>/working/renew` (Claude Code and Codex `PostToolUse` hook, Pi `tool_call`) | renew every record of S |
| Agent reply or agent resolve by S on thread T of A | remove T from (S, A), renew S; a record left with no threads, after having had some, is cleared |
| Viewer resolve, any thread delete | remove T from every record on A (same rule) |
| Publish of A with `X-Clax-Session: S` | clear (S, A), after linking its threads to the new version (Task 13) |
| `working` tool with `done: true` / `DELETE /api/sessions/<S>/working/<A>` | clear (S, A), or only the named threads |
| Stop hook allows the stop (nothing blocks) / Pi `agent_end` → `POST /api/sessions/<S>/working/end` | clear every record of S |
| Session S ends (PATCH `ended`, reaper, `SessionEnd`, shim exit) | clear every record of S |
| Artifact A deleted | clear every record on A |
| `last_heartbeat + 120 s` passes | the record is invisible to reads at once, and the sweeper (every 5 s) removes it and announces the change |

Not renewals: the shim's 60 s `PATCH /api/sessions/<S>` heartbeat, `wait` and `inject` polls that return nothing, and `codex queue` runs. These mean the session is alive, not that it is working.

### What cannot be automatic, per harness

| Harness | Automatic | Not automatic, stated plainly |
|---|---|---|
| Claude Code | Marked when comments arrive by any tier. Renewed by tool calls (the `PostToolUse` hook, throttled to once a minute per session by `scripts/tool-hook.sh`) and every hook. Cleared by reply, publish, the Stop hook at the real end of a turn, and `SessionEnd`. | Work the person asks for in the terminal, not through a comment, is never marked: no hook knows which artifact a prompt is about. The agent must call `working`. Automatic marks carry no message. When the person interrupts a turn (Esc), Claude Code runs no Stop hook, so the mark stays until it lapses, up to 2 minutes later. Without the plugin's hooks (a plain `.mcp.json` install), only clax tool calls renew, so long work with other tools lapses after 2 minutes. |
| Codex | Marked when comments arrive (Stop hook, `SessionStart`, tier 1, `wait_for_feedback`, and `codex queue` on exit 0). Cleared by reply, publish, the Stop hook at turn end, and `SessionEnd`. | `codex queue` exiting 0 means "queued", not "seen". For a session with no TUI attached the mark is false and lapses after 2 minutes. Renewal by tool calls depends on Codex running the plugin's `PostToolUse` hook. Codex 0.159.0 names the event, but this is not measured yet. Task 11 adds the check to `scripts/smoke-codex.sh --hooks`, and the person runs it. The contract says "not yet measured" until their result is recorded. Until then, only clax tool calls and the Stop hook are known to renew. Hooks run only with `features.hooks = true` and after the person trusts them. `codex exec` skips untrusted hooks, so there is no turn-end clear: the mark lapses. No prompt hook is wired, so terminal requests are never marked. |
| Pi | Marked when comments arrive (tier 1, tier 5 injection, `wait_for_feedback`). Renewed by every tool call (`tool_call`, at most every 15 s). Cleared by reply, publish, `agent_end` and `session_shutdown`. | Terminal requests are never marked unless the agent calls `clax_working`. A Pi process that dies without `session_shutdown` leaves the mark to lapse (2 minutes). |

### What the person sees when several sessions work at once

Echo shows working agents rather than records. The words use the agent's name (`claude`, `codex`, `pi`; decided: Q8):
- **Top bar, line 1:** `claude working on N` for one working agent. For several, they are listed: `claude, codex working on 5`. N counts the distinct threads the records name, by any author. With no named threads it reads `claude working`. A record's `message` replaces the count: `claude: Rebuilding the chart`.
- **Line 2** is about you:
  - `all yours` or `N yours` (how many of the worked-on threads you are in), then the elapsed time since the newest record's `started_at`;
  - else `N open threads`;
  - and `codex idle` for an agent on the artifact that is not working, when there is room.
- **Roster.** A working agent's token is solid green with a breathing dot. An idle agent's token is an outline.
- **Gallery card.** One `claude working on N` chip per working agent (filled green).
- **Thread card.** `claude is working on it · 0:42` under the messages, for the newest record naming the thread. The pin is split red-orange and green.
- **Sidebar strip.** `claude is working on #1 (yours) and #3 · 0:42`, with a haiku under it.
- **Top bar sweep.** A 2px green sweep along the bottom edge (none under reduced motion).

The summary is a polite live region whose text changes only when the set of records changes, so renewals stay silent. The elapsed time sits in a separate element outside the live region.

## Design: the version changelog

### Storage

- `versions.note TEXT` (nullable): the agent's note for that version, at most 280 characters after whitespace is collapsed and control characters dropped. Longer notes are cut to 279 characters plus `…`, and the publish result says `note_truncated: true`.
- `version_threads(artifact_id, version_n, thread_id, source, created_at)`, primary key `(artifact_id, version_n, thread_id)`: a thread is addressed in a version.
  - `source` is `working` (automatic at publish), `explicit` (`addresses` on publish) or `resolve` (an agent resolve with no earlier link).
  - A thread may be linked to several versions. Deleting a thread deletes its links.
- `viewer_seen(viewer_id, artifact_id, seen_n, updated_at)`, primary key `(viewer_id, artifact_id)`: the latest version this viewer has viewed unpinned. It only moves forward. It is bounded to the 200 most recently updated artifacts per viewer, and older rows are pruned on write. Deleting an artifact deletes its rows.

### Linking rules

1. A publish of A by session S links every thread in S's working record on A (source `working`), then clears the record.
2. `addresses: [thread IDs]` on a publish links those threads (source `explicit`). Each must be a thread of A, else 400 `unknown_thread` and nothing is published. There are at most 50, else 400 `invalid_args`. Resolved threads may be named.
3. An agent resolve of thread T links T to A's current version (source `resolve`), but only when T has no link at all yet.
4. Linking never changes a thread's status. A person resolves it, one click from the Addressed group.

### Views

- Version views (`GET /api/artifacts/<id>`, `GET .../versions`, publish results) gain `note` (string or null) and `addresses` (thread IDs, in link order).
- Thread views gain `addressed_in` (version numbers, ascending).
- `GET /api/viewers/me/seen?artifact=<id>` answers `{seen: n | null}`. `PUT /api/viewers/me/seen` with `{artifact_id, version}` answers `{seen: n}`. These are viewer routes: no token, a cookie is required for PUT, and a foreign `Origin` is refused.

### What the viewer sees (no banner)

The brief rules out a band over the page. A new version shows in four quiet places, and none of them covers or moves the artifact:
- **The version button's dot.** A green dot on `v5` while the latest version is newer than this viewer's `seen` (the mark from before this load).
- **Top bar line 1.** One of these, when nothing is working:
  - `v5 addressed 3` when the newest version addressed threads you are in that you have not looked at;
  - `3 new versions · 7 addressed` for a viewer returning after several versions (decided: Q11).

  Line 2 then reads `yours, not looked at yet`.
- **The Addressed in vN group** at the top of the sidebar. It holds the open threads you are in that the newest version addressed and that you had not looked at when the view was decided. Each card shows the agent's reply as `<agent> · addressed in vN`, with Reply and Resolve. Under the head, a muted line reads `claude addressed these. Have a look, then resolve each one or reply.`
- **The version menu.** Each version lists who published it and when, the threads it addressed as numbered chips (a green ring, or a red-orange ring when still open after your reply), what you did about them (`you resolved it`, `you replied on 9, still open`), and its note.

Viewing the latest version unpinned writes `seen = latest` after the view is decided. A pinned view writes nothing.

### Time to usable

Notes, addresses, participants and the viewer's attention ride on `GET /api/artifacts/<id>` and the bootstrap block (`boot.rs` `assemble`), which already embeds that response, the thread views and the viewer. So they arrive in the HTML with no request at all. The cost is one indexed query each of `version_threads`, `viewer_threads` and `viewer_seen` per artifact. The seen write and the looked-at writes happen after load, batched (at most one request a second).

## Design: batch send to agent

Decisions: "Batch send to agent" in the decisions file. The labels follow Echo.

### Route and access

`POST /api/artifacts/<aid>/threads:send` takes `{thread_ids, note?, to?}`. Access is exactly the single send's (`POST .../threads/<tid>/send`, `routes/threads.rs::send`): no token, a foreign `Origin` refused (403 `forbidden_origin`), usable by LAN viewers. The viewer cookie names the sender (`viewer::author_name`: the display name, else `Viewer`). `to` is an agent handle (Task 15). With it, only that agent's live session gets rows and each sent thread takes it as its target; without it, the batch fans out to the owner and watchers and clears the threads' targets (see "Send target" above). The single send gains the same optional `to` (`{to}` body).

### All or nothing

One SQLite transaction validates every thread, then writes every feedback row (each carrying the batch's ID), the `send_batches` row and its `batch_threads`. One `feedback::apply` then fans out for the whole batch, so every target's long-poll wakes once and `codex queue` runs once per target with every row. Errors are checked in this order, and none writes anything:

| Case | Answer |
|---|---|
| No threads, more than 20 (one working record's bound), or an ID that is not a ULID | 400 `invalid_args` |
| Note over 280 characters after whitespace is collapsed (the shell caps the field at 280) | 400 `note_too_long` |
| `to` that names no live agent on the artifact | 400 `unknown_agent` |
| A thread that does not exist, was deleted, or is on another artifact | 400 `unknown_thread`, naming every such ID |
| A resolved thread | 400 `thread_resolved`, naming every such ID (as the single send refuses one) |
| Every thread already sent, with nothing new to send | 409 `nothing_to_send` |
| Unknown or deleted artifact | 404 `not_found` |

Duplicate IDs collapse. An already-sent thread is accepted, as the single send is idempotent. Only its viewer comments that have no feedback row yet are sent, and it is reported in `unchanged` when there are none. The batch holds the threads in `sent`.

### Delivery

Every tier renders through `render_items`. A run of items from one batch is led by one line: `[clax] N comments on "<title>", sent together by <name>.`, then ` Note: "<note>"` when there is a note (JSON-quoted, like comment bodies).
- So the piggyback, the Stop hook, the prompt hook, `wait_for_feedback`, `codex queue` and Pi's `sendUserMessage` each hand the agent one grouped delivery, note first.
- Each thread keeps its own feedback rows, so sent state, acknowledgement, resends, the working marker and the changelog link stay per thread.
- A delivered batch marks every thread working through the usual `mark_items`, and a publish then links every thread in the record.

### What the person sees

- Each open thread card has a checkbox, and Shift-click ticks a range. While any card is ticked, a selection bar sits at the top of the sidebar, under the working strip and above the groups. Its parts:
  - the converging dots (red ones meeting a green one; still under reduced motion);
  - `3 selected` over `sent together`;
  - Clear;
  - `Send 3 to claude ▾`;
  - an optional one-line note (Cmd+Enter or Ctrl+Enter sends).
- When open threads have not been sent, a `Send N unsent to claude` button sits at the top of the sidebar.
- Each sent thread's history shows the send: `alex sent it to claude with 2 others · "note"`.

### The page capability: no batch `sendToClaude`

The `comments` capability keeps one-thread `sendToClaude`. The reasons:
- claude.ai's contract has no batch verb, and adding one moves Clax's copy further from pages written for claude.ai.
- A page may only send threads it created in this document, so a batch adds little over calling `sendToClaude` per thread.
- Every call sits in the strict gesture tier (`frameGestureStrict`, `caps/gesture.ts`) and the 10-writes-a-minute budget. A batch verb would let one gesture, possibly a forged one within the activation window, send up to 20 threads.

The viewer's batch is the sidebar's. The spec and the contract say so in Task 1.

---

### Task 1: Spec, contract and design amendments

Task 1's review amended its text: the committed design record `docs/superpowers/specs/2026-10-01-echo-design.md`, the public `seen` in participants, the send target and its later comments, and the selection bar at the top of the sidebar. Where the steps below differ from the committed spec and contract, the committed text is the contract the later tasks build.

Docs only. This task writes down everything the later tasks build: Echo, the working signal, the changelog without a banner, participants and attention, presence, the send target, and batch send. Where this plan extends the spec, the amendment is here. The owner's decisions Q1–Q12 (open-questions.md) are written into the spec, and listed together as one row of §2 Decisions. The tool-count lists (`Twenty-two tools:` in `docs/contract.md` and the READMEs) are not touched here: `scripts/sync-skill-tools.py --check` compares them with the fixture, which gains `working` only in Task 10.

**Files:**
- Modify: `docs/superpowers/specs/2026-09-28-clax-design.md` (§2, §5, §6, §8, §9, §10, §11, §12, §13, §14, §15, §16)
- Modify: `docs/contract.md` (`### publish`, `## Sessions`, `### The comments capability`, `### Tools` under "Comments and feedback", `### Payload`, `### What the person sees`)

**Interfaces:** none (the documentation of Tasks 2–25).

- [ ] **Step 1: §5 Storage and data model**

In the `versions(...)` bullet, replace `session_id, files_json)` with `session_id, files_json, note)`, and append to that bullet: ``; `note` is the agent's short change note for the version (at most 280 characters, null when none).``

In the `sessions(...)` bullet, add `agent_handle` after `id`, and append: ``; `agent_handle` (`a_` and 22 lowercase hex digits from 11 random bytes, unique, assigned at registration, never derived from the ID) names the session's agent to the shell, which never sees a session ID.``

In the `comments(...)` bullet, add `author_public_id` after `author_name`, and append: ``; `author_public_id` is the writing viewer's `public_id` (null for agents, viewers without a cookie, and comments written before it existed).``

After the `session_env(...)` bullet, add:

```markdown
- `version_threads(artifact_id, version_n, thread_id, source, created_at)`:
  the threads a version addressed (§10, "Version changelog"); `source`
  is `working`, `explicit` or `resolve`. Deleting a thread deletes its links.
  Linking never changes a thread's status.
- `viewer_seen(viewer_id, artifact_id, seen_n, updated_at)`: the latest
  version this viewer (the `clax_viewer` cookie's row) has viewed unpinned.
  It only moves forward; at most 200 rows per viewer (the least recently
  updated are pruned on write); deleting an artifact deletes its rows.
- `viewer_threads(viewer_id, thread_id, looked_at)`: when this viewer last
  looked at the thread (§10, "Participants and attention"). Served only to
  that viewer. Deleting a thread deletes its rows.
- `mentions(comment_id, public_id)`: the viewers a comment @mentions.

Working records (§10, "Working") and presence (§10, "Presence") are not
stored: the daemon keeps them in memory, so a restart starts with none.
```

- [ ] **Step 2: §6 HTTP API**

After the `GET /api/events?artifact=<aid>` bullet, add:

```markdown
- The event stream also carries `working` (`{artifact_id, working: [view]}`,
  the artifact's whole list after any change, §10 "Working"). An optional
  `types=<name>,<name>` narrows the stream to those event names (`ready` and
  `resync` are always sent); the gallery opens `?types=working` with no
  `artifact` filter.
```

After the `Feedback:` bullet, add:

```markdown
- Working (§10, "Working"): `GET /api/artifacts/<aid>/working` (no token;
  `{working: [view]}`), `GET /api/sessions/<sid>/working` (token; views plus
  `session_id`, `artifact_id`, `expires_at`), `PUT
  /api/sessions/<sid>/working/<aid>` (W; `{thread_ids?, message?}`; `{working,
  message_truncated}`), `DELETE /api/sessions/<sid>/working/<aid>` (W;
  optional `?thread_ids=<id>,<id>`; `{cleared, working}`), `POST
  /api/sessions/<sid>/working/renew` (W; `{renewed}`), `POST
  /api/sessions/<sid>/working/end` (W, turn end; `{cleared}`). A view is
  `{key, harness, message, thread_ids, started_at, last_heartbeat}`; `key` is
  a ULID minted for the record, never a session ID. `GET /api/artifacts` and
  `GET /api/artifacts/<aid>` carry each artifact's `working` list.
- Changelog (§10, "Version changelog"): version views carry `note` and
  `addresses` (thread IDs); thread views carry `addressed_in` (version
  numbers). `GET /api/viewers/me/seen?artifact=<aid>` (`{seen}`) and `PUT
  /api/viewers/me/seen` (`{artifact_id, version}`, monotonic; `{seen}`) are
  viewer routes (no token, foreign `Origin` refused; PUT without a viewer
  cookie is 400 `no_viewer`).
- Participants and attention (§10, "Participants and attention"): `GET
  /api/artifacts/<aid>` and the bootstrap block carry `participants`
  (`{people: [{public_id, display_name}], agents: [{handle, harness, live}]}`)
  and, when the request's viewer cookie names a viewer, `attention`
  (`{addressed, addressed_v, new_replies, open_in, seen, looked}`); `GET /api/artifacts`
  carries `participants` per artifact. `GET /api/viewers/me/attention`
  answers `{artifacts: {<aid>: {addressed, addressed_v, new_replies, open_in, seen}}}`
  for every artifact (`{artifacts: {}}` without a cookie). `PUT
  /api/viewers/me/looked` takes `{artifact_id, thread_ids}` (at most 50) and
  answers `{looked: {<thread ID>: <time>}}`. Both are viewer routes; every
  response carrying `attention` is `private, no-cache` with `Vary: Cookie`.
- Presence (§10, "Presence"): `PUT /api/viewers/me/presence` takes
  `{artifact_id, state: "here" | "away", where?}` (viewer route; a cookie is
  required) and answers `{people}`; `GET /api/artifacts/<aid>/presence`
  answers `{people: [{public_id, display_name, state, where, since}]}`; the
  event stream carries `presence` with the same body.
```

In "### Publish body", add these two lines to the JSON example after the `"label"` line:

```json
  "note": "Two columns; third bullet dropped",  // optional, ≤ 280 chars, longer is cut
  "addresses": ["01J9..."],                    // optional, ≤ 50 threads of this artifact
```

and after the example add: ``A thread in `addresses` that is not a thread of the artifact is 400 `unknown_thread`, and nothing is published.``

- [ ] **Step 3: §8 Shell UI and viewer: Echo**

Replace the Gallery paragraph with:

```markdown
Gallery (`/`): two groups. **Needs your eyes** holds the artifacts where,
for this viewer, a thread they are in was addressed after they last looked
at it, a version newer than the last one they viewed exists, or a thread
they are in has a reply from someone else they have not seen. **Everything
else** follows, pinned first, then by the latest version or reply. Each card
leads with its version numeral, then the title, the publishing agent and
time, markers (`N addressed in vK`, `vK new`, `N new replies`, `<agent>
working on N`, `N open`), and a footer with the roster (people on the left,
agents on the right, at most 3 a side) and `seen vK`. Search, open, pin and
delete as before. The footer holds one haiku, a new one each visit.
```

Replace the Header bullet with:

```markdown
- Top bar: the Echo mark (a link to the gallery), the title over the
  "published by" line, the roster and its two-line summary (who is working
  on what; what is new for you; opens the people panel), Comment (red-orange
  when on, with a 3px red-orange rule under the bar, and the C keycap),
  Threads with the open count, the version button (`v5 of 5`, a green dot
  while a version newer than this viewer's last view exists) opening the
  version menu, a menu with open raw and copy link, and the theme switch. At
  phone width: the mark, the title, the roster (one per side) and Comment; a
  Page | Threads switch sits at the foot.
- Version menu: a panel listing every version newest first, each with who
  published it and when, the threads it addressed as numbered chips, what
  this viewer did about them, and its note; each a link to that version
  (older versions read-only). A full sheet at phone width.
- People panel (P, or the roster): one row per person (threads they are in,
  presence and location, the last version they viewed) and per agent (the
  threads it is working on and whose, elapsed time with a haiku, or idle),
  and the viewer's own name, edited here.
- Keys: `?` opens a sheet listing them; C comment mode; Esc leaves it or
  closes a menu; T threads; J and K next and previous thread; Enter reply;
  S send; R resolve; X tick; Shift+S send the ticked threads; V versions;
  P people. Keys act only when focus is in the shell and not in a text field.
- Theme: follows the system; the switch flips light and dark, and a choice
  equal to the system's clears back to following it.
- Nothing Clax draws covers or moves the artifact, except pins and the
  comment-mode outline.
```

Replace the Thread sidebar bullet with:

```markdown
- Thread sidebar: groups **Addressed in vN** (open threads this viewer is
  in that the newest version addressed and they had not looked at when the
  view was decided), **Open**, **Detached** (anchor not found on its own page
  in this version) and **Resolved**. A card shows the anchor summary, an
  `outdated` tag when its element changed in a later version but still
  exists (resolved by selector or quote while its `html_hash` differs), the
  clip, the messages (people's with a red-orange rule on the left, an
  agent's with a green rule on the right, `<agent> · addressed in vN` when
  linked), one line of version-tagged history (`v3 alex commented · v4 Mia
  replied · claude worked on it · v5 claude addressed it · alex resolved`),
  and Reply, Resolve and `Send to <agent> ▾`. Anyone may resolve; the
  history records who. Clicking a thread scrolls the frame to its anchor and
  flashes it (a static outline under reduced motion); a thread on another
  page is labelled "on <file>" and clicking it navigates there first. Pins
  show only for the page in the frame: red-orange; split red-orange and
  green while an agent works on the thread; white with a green ring and a
  `vN` flag when addressed and not looked at; a green ring when selected;
  dashed while being written.
- Working: while an agent works on the artifact, the top bar's summary reads
  `claude working on N` (or its message), the roster's agent token is solid
  green with a breathing dot, a 2px green sweep runs under the bar, each
  named thread shows `claude is working on it` with the elapsed time, and the
  sidebar starts with a strip naming the threads and a haiku. The summary is
  a polite live region that changes only when the records change.
```

In the Live updates bullet, replace `a new version shows a
  "v4 published, reload" banner` with `a new version shows "v4 published"
  in the top bar's summary with a Reload button beside it`, and append:
``Nothing is drawn over the page for a version: viewing an older version
shows a Latest link beside the version button, the version button gets its
dot, the summary line reads `v5 addressed 3` (or `3 new versions · 7
addressed` for a viewer returning after several), and the Addressed group
fills.``

Bring the spec's other version-band mentions up to date: in §8's URL paragraph, "the reload banner" becomes "the Reload button"; in §16's **Browser** bullet, "version banner" becomes "the version moment (the version button's dot and the Reload button)". §7's phase list (Phase 1's "version banner") and §17 describe what was built then; leave them as history.

Everywhere else the spec and the contract name the button **Send to agent**
(spec §1, §10 "Data flow"; the contract's comments section), write
**Send to <agent>** (the button names the agent it sends to, for example
**Send to claude**).

Add a paragraph after the Comment mode bullets:

```markdown
Look: Echo. Plex Sans Condensed 600 (one self-hosted WOFF2, `swap`, never
preloaded) sets titles, numerals, labels and buttons in sentence case;
Plex Mono sets everything people and agents write. Red-orange is people,
green is agents, brown is ink, pink is an accent only. Haiku appear in the
gallery footer and under an agent's working line, never in comment mode,
never animated. Buttons are plain verbs; playful words appear only in status
lines, hints and empty states. The mark's halves meet when it is clicked; a
"rally of 10" chip marks an artifact's tenth version.
```

- [ ] **Step 4: §9 comments capability**

Append to the **comments** bullet: ``Clax extension, not part of claude.ai's contract, declared in `web/contract/clax-extensions.d.ts` (`ClaxExtensions.Comments`; the `0.2.61/` files stay claude.ai's, unchanged): `working()` resolves `{working, agents: [{harness, label, message, since, threads, otherThreads}]}`, and `onWorking(fn)` calls `fn` with that state now and on every change and resolves an unsubscribe function. `threads` holds the handles of the threads this document created that the agent names, and `otherThreads` counts the rest. Both are available under either declaration form, need no consent or gesture, and never carry a store ID, session ID or record key.``

- [ ] **Step 5: §10 Comments and the feedback loop**

After "### Watch semantics" and its paragraphs, add:

```markdown
### Working

A working record says that a harness session is acting on an artifact, and
optionally on some of its threads, now. The daemon keeps records in memory
(a restart starts with none) keyed by session and artifact; each holds a
`key` (a ULID), the session's `harness`, an optional `message` (at most 140
characters; longer is cut), up to 20 open thread IDs, `started_at` and
`last_heartbeat`. A record lapses 120 s after its last heartbeat: reads stop
showing it at once, and a sweep every 5 s removes it and sends `working`.

- Marked (created, or renewed with the threads added) whenever the daemon
  hands feedback to the session by tier `piggyback`, `stop_hook`,
  `prompt_hook`, `wait` or `inject`, and when `codex queue` exits 0.
- Set explicitly by the `working` tool.
- Renewed by every feedback take of tier `piggyback`, `stop_hook` or
  `prompt_hook`, by `POST .../working/renew` (the `PostToolUse` hooks and
  Pi's `tool_call`), by the `working` tool, and by the session's agent replies
  and resolves. The shim's session heartbeat, `wait` and `inject` polls and
  `codex queue` never renew.
- Cleared by: the session's agent reply or resolve on the last thread a
  record names; a publish of the artifact by the session; `done: true`; the
  turn ending (the Stop hook allowing the stop, Pi's `agent_end`); the
  session ending; the artifact's deletion. A viewer resolve or a delete takes
  the thread out of every record, with the same last-thread rule.

What cannot be automatic, per harness, is listed in `docs/contract.md`
("Working").

### Version changelog

Each version may carry a `note` and a set of threads it addressed. A
publish by a session links the threads of that session's working record on
the artifact (then clears the record); `addresses` names more; an agent
resolve links the thread to the current version when it has no link yet.
Linking never resolves: a person does, from the Addressed group.

### Participants and attention

A viewer is in a thread when they wrote a comment in it (`author_public_id`),
a comment in it @mentions them (`@` and their display name, any case, at a
word boundary; `@agent` names no viewer), or they resolved it. For each
artifact the daemon computes, per viewer: `addressed` (open threads they are
in linked to a version after they last looked at the thread), `new_replies`
(threads they are in with someone else's comment newer than their last
look), `open_in`, and `seen` (`viewer_seen`). Looking at a thread is its card
being at least half visible for a second, or selecting it; it writes
`viewer_threads`. Viewing a version unpinned writes `viewer_seen`. Resolving
is never needed to clear anything.

The agents on an artifact are its live owner session, the live sessions
watching it, and the sessions that published its versions, named by
`agent_handle`. Send, single or batch, takes an optional `to` (an agent
handle): only that agent's session gets rows. Without `to`, rows go to the
owner and the watchers, as before. The shell defaults `to` to the agent this
viewer last sent to on the artifact, else the latest publisher's agent.

### Presence

A viewer with the artifact open reports `here` (tab visible) or `away`
(hidden, or 5 minutes without input) every 30 s, and optionally `where` (the
anchor label of the thread they have selected or are writing on, at most 80
characters; a per-person switch stops it). The daemon keeps reports in
memory; one lapses 90 s after the last, shows as "last here" for 10 minutes,
then goes. Changes go out as `presence`.
```

- [ ] **Step 6: §11 Sessions and identity**

Append to the "Heartbeats:" paragraph: ``The session heartbeat keeps the session row alive only; it never renews working records (§10, "Working"). Registration assigns `agent_handle`.``

- [ ] **Step 7: §12 MCP tool surface**

In the Artifacts paragraph, after `label;` in the `publish (...)` list, insert ``note (the version's change note), addresses (thread IDs this version addresses);``. In the Comments paragraph, replace `` `comments_resolve`
(url_or_id, thread_id), `` with `` `comments_resolve`
(url_or_id, thread_id; an agent resolve links the thread to the current version when it has no link yet), `working` (url_or_id, thread_ids, message, done), ``.

- [ ] **Step 8: §13 Plugins**

In the Claude Code `hooks/hooks.json` bullet, replace `` `Stop` →
  `hook --agent claude stop` (tier 2, timeout 10 s); `` with `` `Stop` →
  `hook --agent claude stop` (tier 2, timeout 10 s; when it allows the stop it
  ends the turn's working records); `PostToolUse` → `scripts/tool-hook.sh claude`
  (renews working records at most once a minute per session: a shell check
  of a stamp file under `~/.clax/run/tool-hook/` skips starting `clax` when
  the last renewal is under 60 s old; always exits 0; timeout 5 s); ``. In the Codex `hooks/hooks.json` bullet, after `` `Stop` → the same with `stop` `` add `` (which also ends the turn's working records when it allows the stop), `PostToolUse` → `bash "${PLUGIN_ROOT}/scripts/tool-hook.sh" codex` (the same once-a-minute renewal; not yet measured on Codex, see `docs/contract.md`) ``. In the Pi bullets, replace `for the fourteen tools` with `for the twenty-three tools`, and add a bullet: ``- Working: `tool_call` renews the session's working records (at most once every 15 s), and `agent_end` ends them.``

- [ ] **Step 9: §14, §15, §16**

§14, add a bullet: ``- Working views and events carry the record's `key`, `harness`, `message`, thread IDs and times, and are readable without the token, like threads: LAN viewers see which harness is working and its message. They never carry a session ID, working directory or PID. Messages and version notes are agent text, rendered by the shell as text only.`` Add a second bullet: ``- Participants carry viewers' public IDs and display names (already public through comments) and agents' handles and harnesses. Attention and looked-at marks are served only to the viewer whose cookie the request carries. Presence carries public IDs, display names, here or away, and the optional location the person chose to share.``

§15, in "Hook timeouts", after `prompt submit 4 s` add `, tool 2 s (1 s per daemon request)`.

§16, append to the **clax-server** bullet: ``, working records (set, mark, renew, clear, expiry by an injected clock), the changelog links and seen marks, attention and looked-at marks, agent handles, the send target, and presence (expiry by an injected clock)``. Append to **Browser**: ``, Echo (fonts, theme switch, keys, the mark), the working summary, roster, card chips, thread marker, pins and capability, the Addressed group, version menu and history line, needs your eyes, batch send with the agent picker, and presence; every UI change is checked in screenshots, light and dark, desktop and phone``.

- [ ] **Step 10: `docs/contract.md`**

In `### publish`, add two rows to the arguments table after `label`:

```markdown
| `note` | string | no | This version's change note for the person, at most 280 characters (longer is cut, with `note_truncated: true`). |
| `addresses` | array of thread IDs | no | Threads of this artifact this version addresses (at most 50). Linking does not resolve them. |
```

and after the table's paragraph about omitted fields, add: ``The result also carries `note` and `addressed` (every thread linked to the new version: those named, plus those this session was marked working on for the artifact).``

At the end of `## Sessions` (before `### Claude Code`), add:

```markdown
The hooks also keep working records current: the `stop` hook, when it allows
the stop, ends the turn (`POST /api/sessions/<id>/working/end`), and the
`PostToolUse` hook renews them (`POST /api/sessions/<id>/working/renew`) at
most once a minute per session. The hook command is `scripts/tool-hook.sh`,
a POSIX shell gate. It reads the hook input, takes the `session_id`, and
exits 0 without starting `clax` when the stamp file
`$CLAX_HOME/run/tool-hook/<harness>-<session ID>` (default home `~/.clax`)
was modified less than 60 s ago. Otherwise it touches the stamp and runs
`clax hook --agent <harness> tool` (2 s, 1 s per request). It always exits 0
and prints nothing. The `session-end` hook removes the session's stamp.
```

In "Comments and feedback" → `### Tools`, add a table row:

```markdown
| `working` | `url_or_id`; optional `thread_ids` (at most 20 open threads), `message` (at most 140 characters), `done` | `{artifact_id, url, working: true, message, thread_ids, started_at, expires_in_s, message_truncated}` or `{artifact_id, url, working: false, cleared}` |
```

and after that table's paragraphs add this subsection:

```markdown
### Working

`working` tells the person you are acting on an artifact. The top bar
shows `claude working on N` (or `claude: <message>`), its gallery card a
chip, and each thread named in `thread_ids` `claude is working on it`. Comments sent to you
mark you working automatically; call `working` for work that did not start
from a comment, or to add a message. `thread_ids` and `message` replace the
stored ones when given; `done: true` clears the record, or with `thread_ids`
only those threads. Errors: `invalid_id`, `invalid_args` (a thread ID that is
not a ULID, more than 20), `not_found` (no such artifact), `unknown_thread`
(not a thread of the artifact), `thread_not_open`, `no_session` (the
daemon's `/mcp`), `unknown_session`, `daemon_unreachable`.

It clears when you reply to or resolve the last thread it names, publish the
artifact, end your turn, or go 120 s without renewing it, and when your
session ends. Renewal is automatic:

| Harness | Renewed by | Not automatic |
|---|---|---|
| Claude Code | tool calls, at most once a minute (`PostToolUse` hook), every hook, clax tool calls | work asked for in the terminal (call `working`); a turn you interrupt with Esc runs no Stop hook, so its mark lapses within 2 minutes |
| Codex | clax tool calls, the Stop hook; tool calls through the `PostToolUse` hook: not yet measured (`scripts/smoke-codex.sh --hooks`) | a `codex queue` delivery to a session with no TUI marks it for up to 2 minutes; `codex exec` without trusted hooks never ends the turn; terminal requests |
| Pi | tool calls (at most every 15 s) | terminal requests; a Pi process killed without `session_shutdown` |
```

In `### The comments capability`, add at its end:

```markdown
Clax extension (not in claude.ai's contract; declared in
`clax-extensions.d.ts`, `ClaxExtensions.Comments`, served at
`<daemon_url>/_clax/contract/clax-extensions.d.ts`): `working()` and
`onWorking(fn)` report which agents are working on this artifact:
`{working: boolean, agents: [{harness, label, message, since, threads,
otherThreads}]}`. `threads` are handles of threads this document created;
`otherThreads` counts the rest. Available under either declaration form,
without consent or gesture.
```

In `### What the person sees`, add at its end:

```markdown
An open thread a working record names shows "claude is working on it"
instead of its waiting indicator. A new version puts nothing over the page:
the version button gets a dot, the top bar's summary reads "v5 addressed 3",
the sidebar's "Addressed in v5" group lists the threads it addressed that
the viewer is in, with your reply shown as "claude · addressed in v5", and
the version menu lists every version's note. Each thread's history line
shows what happened on which version. When the person sends several
threads, or several agents work on one artifact, the person picks the agent;
the payload is the same.
```

- [ ] **Step 11: Batch send to agent (spec and contract)**

Spec §5: in the `feedback(...)` bullet, add `batch_id` to the column list, and append: ``; `batch_id` names the batch send that created the row, if any.`` After the `viewer_seen` bullet from Step 1, add:

```markdown
- `send_batches(id, artifact_id, note, sent_by, size, created_at)` and
  `batch_threads(batch_id, thread_id)`: batch sends to the agent (§10,
  "Batch send"); `sent_by` is the sender's display name as a comment author
  gets it, never a cookie. Deleting a thread or its artifact deletes its rows.
```

Spec §6: after the `.../threads/<tid>/send` sentence in the Comments bullet, add: ``POST .../threads:send`` (no token; foreign `Origin` refused, as the single send) takes `{thread_ids, note?, to?}` and sends 1 to 20 threads as one batch, all or nothing. It answers `{batch, sent, unchanged, threads}`, or 400 `invalid_args` / `note_too_long` / `unknown_agent` / `unknown_thread` / `thread_resolved`, or 409 `nothing_to_send`, and writes nothing on any error. Thread views carry `sends`, the batches that sent them. The single send takes an optional JSON body `{to}` (an agent handle; 400 `unknown_agent` when it names no live agent on the artifact).``

Spec §8: in the Thread sidebar bullet, append: ``Open thread cards carry a checkbox (Shift-click ticks a range; X ticks the selected thread). While any is ticked, a selection bar at the top of the sidebar reads `N selected · sent together`, with Clear, `Send N to <agent> ▾` and an optional one-line note (Cmd+Enter or Ctrl+Enter sends; Shift+S sends). A `Send N unsent to <agent>` button sits at the sidebar top whenever open threads have not been sent. A sent thread's history shows the send and its note. A thread that disappears leaves the selection.``

Spec §9: append to the **comments** bullet: ``There is no batch form of `sendToClaude`: a page sends threads it created one call at a time, each in the strict gesture tier; batches are the viewer's, from the sidebar.`` In the Clax extension sentence from Step 4, after `under either declaration form`, insert ``(for `composer_only`, a Clax extension to that form, which otherwise grants only `openComposer` and `anchorFor`)``.

Spec §10: after "### Version changelog", add:

```markdown
### Batch send

The viewer can send several threads at once (`POST .../threads:send`). One
transaction checks every thread and writes every feedback row, marked with
the batch; one fan-out follows, so every tier hands the batch over together.
The payload leads the batch's comments with `[clax] N comments on "<title>",
sent together by <name>.` and ` Note: "<note>"` when there is one. Each
thread keeps its own rows, sent state, working marker and changelog link.
```

`docs/contract.md`, "Comments and feedback" → `### Payload`: append:

```markdown
Comments the person sent together arrive together. After the counted
header, a line `[clax] N comments on "<title>", sent together by <name>.`
(and ` Note: "<note>"`, the note JSON-quoted like a comment body) leads the
batch's items. The note is the person's words for the whole batch, and like
comment text it is a request to weigh.
```

and in `### The comments capability`, add: ``Clax adds no batch `sendToClaude`; the batch send is the viewer's, from the sidebar.``

- [ ] **Step 12: §2 Decisions**

Append to the §2 Decisions table, as one row:

```markdown
| D-Echo | Decided on 2026-10-01 (`2026-10-01-echo-design.md`): gallery cards without thumbnails (Q1); the theme switch's return to the
  system (Q2); @mention matching (Q3); what counts as looking, and the
  Addressed group holding still until the view is decided again (Q4);
  presence built now, with location from the selected thread (Q5); keys only
  while focus is in the shell (Q6); others' last seen version public, their
  per-thread marks private (Q7); agents named by harness, no "publishing"
  state (Q8); rally of 10 at v10 only (Q9); any viewer may resolve (Q10); the
  returning-viewer summary in the top bar (Q11); the version bands moved
  into the top bar (Q12). | Settled before the Echo build so no task waits on a design question. |
```

Write the row on one line, as the table's other rows are. If the table's columns differ, keep its column count and put the second sentence in its last column.

- [ ] **Step 13: Check and stage**

Run: `grep -n "fourteen tools\|banner, then\|Changes section\|Send to agent" docs/superpowers/specs/2026-09-28-clax-design.md docs/contract.md; bash scripts/test-plugins.sh | tail -1`
Expected: no lines from the grep; `plugin checks passed`.

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add docs/superpowers/specs/2026-09-28-clax-design.md docs/contract.md
git status --short   # staged; the controller commits ("Specify Echo, the agent working signal, the version changelog, attention, presence and batch send")
```

---

### Task 2: Echo tokens, the two type voices, the font budget and the screenshot script

The base layer of Echo: tokens, Plex Sans Condensed beside Plex Mono, sentence case, plain-verb buttons, and a font budget in the bundle gate. It also adds the screenshot script that every later UI task runs. Components keep their current layout. Tasks 4–6 restyle them.

**Files:**
- Create: `web/shell/public/_clax/fonts/ibm-plex-sans-condensed-latin-600.woff2` (copied from `.superpowers/sdd/2026-09-30-redesign/concept-3-echo/fonts/ibm-plex-sans-condensed-latin-600-normal.woff2`, 19,816 bytes)
- Create: `web/scripts/measure-fallback.mjs`, `web/shell/src/echo-theme.test.ts`, `web/e2e/shots.spec.ts`, `web/e2e/scenes.ts`, `web/e2e/pages/sample-report.html`
- Modify: `web/shell/src/theme.css`, `web/scripts/bundle-size.mjs`, `web/scripts/bundle-size.test.ts`, `web/perf/bundle-budget.json`, `web/shell/public/_clax/fonts/OFL.txt`

**Interfaces:**
- CSS custom properties (both themes): `--you`, `--on-you`, `--agent`, `--agent-ink`, `--accent-tint`, `--pink`, `--grot`, `--mono` (the port's `--font` stays as an alias of `--mono`).
- Classes: `.g` (Plex Sans Condensed 600), `.kc` (a keycap), `button.ghost`, `button.icon`.
- `web/perf/bundle-budget.json` gains `"fonts": 50144`: the gzip-free byte total of `dist/_clax/fonts/*.woff2`. `bundle-size.mjs` also fails when a WOFF2 file is preloaded, when there are more than three of them, or when an `@font-face` lacks `font-display: swap`.
- `web/e2e/scenes.ts`: `type Seeded = { base: string; token: string; aid: string; sid: string; threads: string[] }`, `type Scene = { name: string; path(s: Seeded): string; prepare?(page: Page, s: Seeded): Promise<void> }`, `export const SCENES: Scene[]`, `export async function seed(base, token): Promise<Seeded>`. Later tasks append scenes.

- [ ] **Step 1: The theme test, first**

`web/shell/src/echo-theme.test.ts`:

```ts
// Echo's base layer (spec §8, "Look"): two type voices, sentence case,
// swap-only fonts, and the people/agent colours in both themes.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const css = readFileSync(join(__dirname, "theme.css"), "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
const faces = [...css.matchAll(/@font-face\s*\{([^}]*)\}/g)].map(m => m[1]);
const block = (sel: string) => [...css.matchAll(/([^{}]+)\{([^{}]*)\}/g)].filter(m => m[1].split(",").map(s => s.trim()).includes(sel)).map(m => m[2]).join(";");

describe("Echo theme", () => {
  it("self-hosts exactly three faces, every one swap, the condensed one at 600 only", () => {
    const real = faces.filter(f => /url\(/.test(f));
    expect(real).toHaveLength(3);
    for (const f of real) expect(f).toMatch(/font-display:\s*swap/);
    const condensed = real.filter(f => /IBM Plex Sans Condensed/.test(f));
    expect(condensed).toHaveLength(1);
    expect(condensed[0]).toMatch(/font-weight:\s*600/);
    expect(condensed[0]).toMatch(/\/_clax\/fonts\/ibm-plex-sans-condensed-latin-600\.woff2/);
  });

  it("sets nothing in tracked capitals", () => {
    expect(css).not.toMatch(/text-transform:\s*uppercase/);
    expect(css).not.toMatch(/letter-spacing:\s*\.0[4-9]em/);
  });

  it("defines people and agent colours for light and both dark paths", () => {
    expect(block(":root")).toMatch(/--you:\s*#ed5439/);
    expect(block(":root")).toMatch(/--agent:\s*#457d26/);
    expect(block(':root[data-theme="dark"]')).toMatch(/--agent:\s*#8cc46b/);
    expect(css).toMatch(/:root:not\(\[data-theme="light"\]\)\s*\{\s*@media \(prefers-color-scheme: dark\)\s*\{[^}]*--agent:\s*#8cc46b/);
  });

  it("sets buttons in the condensed face, sentence case, and stops faux bold", () => {
    expect(block("button")).toMatch(/font:\s*600 14px\/1(\.\d+)? var\(--grot\)/);
    expect(block(":root")).toMatch(/font-synthesis:\s*none/);
  });
});
```

Run: `cd web && npx vitest run shell/src/echo-theme.test.ts`
Expected: FAIL (two faces, uppercase rules, no `--you`).

- [ ] **Step 2: The font, its licence, and the fallback metrics**

```bash
cp .superpowers/sdd/2026-09-30-redesign/concept-3-echo/fonts/ibm-plex-sans-condensed-latin-600-normal.woff2 \
   web/shell/public/_clax/fonts/ibm-plex-sans-condensed-latin-600.woff2
test "$(wc -c < web/shell/public/_clax/fonts/ibm-plex-sans-condensed-latin-600.woff2)" -eq 19816 && echo size-ok
```

Expected: `size-ok`. In `web/shell/public/_clax/fonts/OFL.txt`, change the first copyright line so that it names the IBM Plex family as a whole (`Copyright © 2017 IBM Corp. with Reserved Font Name "Plex"`), if it names only Plex Mono. The licence text itself is the same for both faces.

`web/scripts/measure-fallback.mjs`. It measures how much wider or narrower each local fallback is than Plex Sans Condensed 600 for the shell's own words, and prints the `@font-face` overrides. It runs Chromium from `@playwright/test`, with no daemon and no network:

```js
// Prints metric overrides for the local faces that stand in for IBM Plex
// Sans Condensed 600 until it loads (font-display: swap), so the swap barely
// moves the top bar. Plex's ascent and descent are 1.025 and 0.275 em.
import { chromium } from "@playwright/test";
import { readFileSync } from "node:fs";

const woff = readFileSync(new URL("../shell/public/_clax/fonts/ibm-plex-sans-condensed-latin-600.woff2", import.meta.url)).toString("base64");
const SAMPLE = "Checkout latency, week 39 Comment Threads v5 of 5 Send to claude Resolve Reply Needs your eyes Addressed in v5 0123456789";
const FALLBACKS = {
  "Plex Condensed Fallback": ["Avenir Next Condensed Demi Bold", "AvenirNextCondensed-DemiBold", "Helvetica Neue Condensed Bold", "HelveticaNeue-CondensedBold"],
  "Plex Condensed Fallback L": ["DejaVu Sans Condensed Bold", "DejaVuSansCondensed-Bold", "Liberation Sans Narrow Bold", "LiberationSansNarrow-Bold", "Arial Narrow Bold", "ArialNarrow-Bold"],
};
const browser = await chromium.launch();
const page = await browser.newPage();
await page.setContent(`<style>@font-face{font-family:P;src:url(data:font/woff2;base64,${woff}) format("woff2");font-weight:600}</style>`);
await page.evaluate(() => document.fonts.load("600 100px P"));
const width = (family, weight) => page.evaluate(([f, w, s]) => {
  const c = document.createElement("canvas").getContext("2d");
  c.font = `${w} 100px ${f}`;
  return c.measureText(s).width;
}, [family, weight, SAMPLE]);
const plex = await width("P", 600);
for (const [name, locals] of Object.entries(FALLBACKS)) {
  for (const local of locals) {
    // A face this machine lacks falls through to monospace: skip it.
    const w = await width(`"${local}", monospace`, 400);
    if (Math.abs(w - (await width("monospace", 400))) < 0.5) continue;
    const sa = plex / w;
    const pct = n => `${(n * 100).toFixed(2)}%`;
    console.log(`${name} (${local}): size-adjust: ${pct(sa)}; ascent-override: ${pct(1.025 / sa)}; descent-override: ${pct(0.275 / sa)}; line-gap-override: 0%;`);
    break;
  }
}
await browser.close();
```

Run: `cd web && node scripts/measure-fallback.mjs`
Expected: one line per fallback family, for whichever local faces this machine has. Put the printed values into the two `@font-face` rules in Step 3. On a machine without one of those faces, keep the starting values given in Step 3 for that rule, and say so in the task report.

- [ ] **Step 3: theme.css, the base layer**

Replace the file's head, from its first comment through the `a { … }` rule, with:

```css
/* IBM Plex Mono 400/600 and IBM Plex Sans Condensed 600, Latin subsets
   (SIL OFL 1.1, see /_clax/fonts/OFL.txt). Every face is `swap` and none is
   preloaded: text shows at once in a metric-matched local face and swaps
   when the file arrives. Mono carries what people and agents write; the
   condensed face carries Clax's structure (titles, numerals, labels,
   buttons). */
@font-face { font-family: "IBM Plex Mono"; font-weight: 400; font-display: swap; src: url("/_clax/fonts/ibm-plex-mono-latin-400.woff2") format("woff2"); }
@font-face { font-family: "IBM Plex Mono"; font-weight: 600; font-display: swap; src: url("/_clax/fonts/ibm-plex-mono-latin-600.woff2") format("woff2"); }
@font-face { font-family: "IBM Plex Sans Condensed"; font-weight: 600; font-style: normal; font-display: swap; src: url("/_clax/fonts/ibm-plex-sans-condensed-latin-600.woff2") format("woff2"); }
@font-face { font-family: "Plex Fallback"; src: local("Menlo Regular"), local("Menlo-Regular"), local("DejaVu Sans Mono"), local("DejaVuSansMono"); size-adjust: 99.66%; ascent-override: 102.85%; descent-override: 27.59%; line-gap-override: 0%; }
@font-face { font-family: "Plex Fallback C"; src: local("Consolas"); size-adjust: 109.13%; ascent-override: 93.92%; descent-override: 25.2%; line-gap-override: 0%; }
/* Measured by web/scripts/measure-fallback.mjs against the shell's words. */
@font-face { font-family: "Plex Condensed Fallback"; font-weight: 600; src: local("Avenir Next Condensed Demi Bold"), local("AvenirNextCondensed-DemiBold"), local("Helvetica Neue Condensed Bold"), local("HelveticaNeue-CondensedBold"); size-adjust: 100%; ascent-override: 102.5%; descent-override: 27.5%; line-gap-override: 0%; }
@font-face { font-family: "Plex Condensed Fallback L"; font-weight: 600; src: local("DejaVu Sans Condensed Bold"), local("DejaVuSansCondensed-Bold"), local("Liberation Sans Narrow Bold"), local("LiberationSansNarrow-Bold"), local("Arial Narrow Bold"), local("ArialNarrow-Bold"); size-adjust: 100%; ascent-override: 102.5%; descent-override: 27.5%; line-gap-override: 0%; }
/* Light ratios (WCAG 2.x): fg ≥15.8, muted ≥6.7, accent-ink ≥5.3, danger ≥6.8
   on every ground; border-strong ≥3.35 and focus ≥4.4 (UI); brown on
   red-orange 5.06. Red-orange is people, green is agents, brown is ink,
   pink is an accent only. */
:root {
  --bg: #fbf4f1; --card: #ffffff; --raised: #ffffff; --fg: #2f0b04; --muted: #6f4b42;
  --border: #e8d6d1; --border-strong: #9e7b72;
  --accent: #457d26; --accent-ink: #3d6f21; --accent-hover: #3d6f21; --on-accent: #ffffff; --accent-tint: #eef4ea;
  --danger: #a3123a; --danger-tint: #f9eef1; --focus: #457d26;
  --you: #ed5439; --on-you: #2f0b04; --agent: #457d26; --agent-ink: #3d6f21; --pink: #f9c8bf;
  --pin: var(--you); --on-pin: var(--on-you); --pin-ring: #ffffff;
  --selection-bg: var(--pink); --selection-fg: #2f0b04; --comment-hl: #fceeea; --shadow: rgba(47,11,4,.16);
  --mono: "IBM Plex Mono", "Plex Fallback", "Plex Fallback C", ui-monospace, Menlo, Consolas, monospace;
  --font: var(--mono);
  --grot: "IBM Plex Sans Condensed", "Plex Condensed Fallback", "Plex Condensed Fallback L", "Arial Narrow", sans-serif;
  --t: .12s cubic-bezier(.4,0,.2,1);
  --radius: 0; --gutter: 16px; color-scheme: light dark; font-synthesis: none;
}
/* Dark ratios: fg ≥12.4, muted ≥6.4, accent ≥7.2, danger ≥6.6 on every ground;
   border-strong ≥3.13, pin ≥4.18 (UI). */
:root:not([data-theme="light"]) { @media (prefers-color-scheme: dark) {
  --bg: #1a0d09; --card: #261410; --raised: #331c16; --fg: #f7e8e4; --muted: #c4a49b; --border: #45291f; --border-strong: #94695e;
  --accent: #8cc46b; --accent-ink: #8cc46b; --accent-hover: #7db85b; --on-accent: #1a0d09; --accent-tint: #22291a; --danger: #ff8a9e; --danger-tint: #3c201e; --focus: #8cc46b;
  --agent: #8cc46b; --agent-ink: #8cc46b; --comment-hl: #3e1e16; --shadow: rgba(0,0,0,.55);
} }
:root[data-theme="dark"] {
  --bg: #1a0d09; --card: #261410; --raised: #331c16; --fg: #f7e8e4; --muted: #c4a49b; --border: #45291f; --border-strong: #94695e;
  --accent: #8cc46b; --accent-ink: #8cc46b; --accent-hover: #7db85b; --on-accent: #1a0d09; --accent-tint: #22291a; --danger: #ff8a9e; --danger-tint: #3c201e; --focus: #8cc46b;
  --agent: #8cc46b; --agent-ink: #8cc46b; --comment-hl: #3e1e16; --shadow: rgba(0,0,0,.55); color-scheme: dark;
}
:root[data-theme="light"] { color-scheme: light; }
*, *::before, *::after { box-sizing: border-box; }
html, body { margin: 0; height: 100%; }
body { background: var(--bg); color: var(--fg); font: 14px/1.5 var(--mono); -webkit-font-smoothing: antialiased; }
::selection { background: var(--selection-bg); color: var(--selection-fg); }
::placeholder { color: var(--muted); opacity: 1; }
:focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; }
a { color: inherit; text-decoration: none; }
/* Structure speaks in the condensed face (68 Display, 67 Title, 57 Label). */
.g, h1, h2, h3 { font-family: var(--grot); font-weight: 600; letter-spacing: 0; }
.kc { font: 600 10px/14px var(--mono); border: 1px solid currentColor; padding: 0 4px; opacity: .7; }
```

Replace the `button { … }` rule and the three rules after it (`button:not(:disabled):hover`, `button.primary`, `button.primary:not(:disabled):hover`) with:

```css
button { font: 600 14px/1.15 var(--grot); min-height: 32px; padding: 7px 12px; background: var(--card); color: var(--fg); border: 1px solid var(--border-strong); border-radius: 0; cursor: pointer; display: inline-flex; align-items: center; justify-content: center; gap: 8px; white-space: nowrap; transition: color var(--t), background-color var(--t), border-color var(--t); }
button:not(:disabled):hover { border-color: var(--fg); }
button.primary { background: var(--accent); color: var(--on-accent); border-color: var(--accent); }
button.primary:not(:disabled):hover { background: var(--accent-hover); border-color: var(--accent-hover); }
button.ghost { background: none; border-color: transparent; }
button.ghost:not(:disabled):hover { border-color: var(--border-strong); }
button.icon { width: 32px; padding: 0; flex: none; }
button.icon svg { width: 16px; height: 16px; }
```

Then remove every `text-transform: uppercase;` declaration and every `letter-spacing: .0Nem;` declaration that remains in the file. They are in the rules for `.topbar > h1:first-child`, `.topbar > h1:first-child + .muted`, the bracketed topbar actions, `.card .meta > span:nth-child(-n+2)`, `.banner a`, `.sidebar h2` and `.prompt h2`. Delete a rule that is left empty. In the bracketed topbar actions rule, also delete the `::before` and `::after` rules that draw `[ ` and ` ]`. Task 4 replaces those actions with a menu. In `.sidebar h2` and `.prompt h2`, set `font-size: 15px`, which suits the condensed face.

Run: `cd web && npx vitest run shell/src/echo-theme.test.ts shell/src/topbar-style.test.ts`
Expected: `echo-theme` PASS. If `topbar-style.test.ts` asserted the brackets, change that assertion to check that open raw and copy link render inside the island. Do not delete the test.

- [ ] **Step 4: The font budget in the bundle gate**

In `web/scripts/bundle-size.mjs`, after the `markers` loop, add:

```js
// Fonts: at most three WOFF2 files, every @font-face swap, none preloaded;
// their bytes (already compressed) are budgeted as `fonts`.
const fontDir = new URL("_clax/fonts/", dist);
const woffs = readdirSync(fontDir).filter(f => f.endsWith(".woff2"));
if (woffs.length > 3) throw new Error(`dist/_clax/fonts holds ${woffs.length} WOFF2 files; at most 3`);
for (const html of ["index.html", "artifact.html"]) {
  const text = read(html).toString();
  if (/<link[^>]+rel="?preload"?[^>]+\.woff2/.test(text)) throw new Error(`dist/${html} preloads a font; fonts must never block or jump the queue`);
  for (const face of text.matchAll(/@font-face\s*\{([^}]*)\}/g)) {
    if (/url\(/.test(face[1]) && !/font-display:\s*swap/.test(face[1])) throw new Error(`dist/${html}: an @font-face without font-display: swap`);
  }
}
const fontBytes = woffs.reduce((n, f) => n + statSync(new URL(f, fontDir)).size, 0);
```

Add `readdirSync` and `statSync` to the file's existing `node:fs` import (`import { existsSync, readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";`). Then:
- add `fonts: fontBytes` to `sizes`, after the part sizes are filled in;
- print it in the `console.log` line (`fonts ${sizes.fonts}`);
- add `"fonts"` to `MEASURED` (`["gallery", "artifact", "bridge", ...Object.values(partKeys), "fonts"]`). `KEYS` is built from `MEASURED`, and the check and record loops already walk `MEASURED`, so nothing else changes.

`--record` never raises a budget, as before.

`web/perf/bundle-budget.json`: add `"fonts": 50144`. This is 14,708 + 15,620 + 19,816 bytes: the three files exactly. Any further font byte is the owner's call.

In `web/scripts/bundle-size.test.ts`, first make the existing fixture pass the new checks: in `run()`, write three small files (`a.woff2`, `b.woff2`, `c.woff2`, a few bytes each) under `dist/_clax/fonts`, and add `fonts` (larger than their total) to the `full` budget. Every existing case then passes as before. Add cases in the file's existing style:
- a fixture `dist` with a fourth WOFF2 fails with `at most 3`;
- an `index.html` with `<link rel="preload" href="/_clax/fonts/x.woff2">` fails with `preloads a font`;
- a budget file without `fonts` fails with `lacks a numeric budget for: fonts`.

Run: `cd web && npx vitest run scripts/bundle-size.test.ts && npm run build && node scripts/bundle-size.mjs; echo "exit=$?"`
Expected: PASS and `exit=0`. The printed `fonts` is 50144. The `gallery` and `artifact` sizes may grow by the CSS. If either goes over its budget, stop and report the numbers.

- [ ] **Step 5: The screenshot script**

`web/e2e/pages/sample-report.html`: a stand-in for an agent's page, the mockup's sample (`concept-3-echo/index.html`, the `.art` markup from `<header class="a-hero">` to the closing `</section>` of `.a-band`, with the `.a-*` rules from its `<style>`), with the mockup's `pin`, `ov` and `ov-cursor` spans removed. It is never themed by Clax.

`web/e2e/scenes.ts`:

```ts
// The states every UI task photographs (light and dark, desktop and phone),
// on one seeded scratch daemon. Tasks append scenes as they add UI.
import type { Page } from "@playwright/test";
import { readFileSync } from "node:fs";
import { api, postThread, publishAs, registerSession } from "./fixtures";

export type Seeded = { base: string; token: string; aid: string; sid: string; threads: string[] };
export type Scene = { name: string; path(s: Seeded): string; prepare?(page: Page, s: Seeded): Promise<void> };

const REPORT = readFileSync(new URL("./pages/sample-report.html", import.meta.url), "utf8");

/** One artifact by a claude session, three threads by "alex", two quieter artifacts. */
export async function seed(base: string, token: string): Promise<Seeded> {
  const s = await registerSession(base, token, "claude", "shots");
  const { artifact } = await publishAs(base, token, s.id, "Checkout latency, week 39", { "index.html": REPORT });
  const threads: string[] = [];
  for (const body of ["Is p95 measured at the edge or at the app server? Say which in the label.", "Mark the deploy on the chart itself.", "Sort by p95, worst first."]) {
    threads.push((await postThread(base, artifact.id, body)).id);
  }
  const other = await registerSession(base, token, "codex", "shots-codex");
  await publishAs(base, token, other.id, "Onboarding checklist", { "index.html": "<main><h2>First week</h2><ul><li>Laptop</li><li>Access</li></ul></main>" });
  await api(base, token, "/api/artifacts", { method: "POST", body: JSON.stringify({ title: "Permissions probe", files: { "index.html": { content: "<main><h2>Probe</h2></main>", encoding: "utf8" } } }) });
  return { base, token, aid: artifact.id, sid: s.id, threads };
}

const name = async (page: Page, who: string) => {
  await page.evaluate(n => fetch("/api/viewers/me", { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify({ display_name: n }) }), who);
};

export const SCENES: Scene[] = [
  { name: "gallery", path: () => "/", prepare: async page => { await name(page, "alex"); await page.reload(); } },
  { name: "view", path: s => `/a/${s.aid}` },
  { name: "comment", path: s => `/a/${s.aid}`, prepare: async page => { await page.getByRole("button", { name: /^Comment/ }).click(); } },
];
```

Add `postThread` to `web/e2e/fixtures.ts`. It posts a thread as the shell does (multipart), and later tasks use it:

```ts
/** Creates a viewer thread on `aid` v1 anchored to `body > main > h2`, as the shell posts it (multipart); returns the thread. */
export async function postThread(base: string, aid: string, body: string, selector = "body > main > h2") {
  const form = new FormData();
  form.set("anchor", JSON.stringify({ kind: "element", selector, quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" }));
  form.set("body", body);
  form.set("version", "1");
  const res = await fetch(`${base}/api/artifacts/${aid}/threads`, { method: "POST", body: form, headers: { origin: base } });
  if (!res.ok) throw new Error(`${res.status} ${await res.text()}`);
  return (await res.json()).thread as { id: string };
}
```

`web/e2e/shots.spec.ts`:

```ts
// Screenshots of every scene in light and dark, at 1440×900 and 390×844, for
// the task named by CLAX_SHOTS (task-NN). Skipped unless it is set. Fails if
// a phone-width page scrolls sideways.
import { expect, test } from "@playwright/test";
import { mkdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { startDaemon } from "./fixtures";
import { SCENES, seed, type Seeded } from "./scenes";

const TASK = process.env.CLAX_SHOTS ?? "";
const ONLY = process.env.CLAX_SCENES?.split(",") ?? null;
const SIZES = { desktop: { width: 1440, height: 900 }, phone: { width: 390, height: 844 } } as const;
const out = fileURLToPath(new URL(`../../.superpowers/sdd/2026-09-30-redesign/build-shots/${TASK}/`, import.meta.url));

test.skip(!/^task-\d\d$/.test(TASK), "set CLAX_SHOTS=task-NN to take the screenshots");

let d: Awaited<ReturnType<typeof startDaemon>>;
let s: Seeded;
test.beforeAll(async () => { test.setTimeout(240_000); d = await startDaemon(); s = await seed(d.base, d.token); mkdirSync(out, { recursive: true }); });
test.afterAll(async () => { await d?.stop(); });

for (const scene of SCENES) {
  for (const theme of ["light", "dark"] as const) {
    for (const [size, viewport] of Object.entries(SIZES)) {
      test(`${scene.name} ${theme} ${size}`, async ({ page }) => {
        test.skip(!!ONLY && !ONLY.includes(scene.name));
        await page.setViewportSize(viewport);
        await page.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" });
        await page.goto(`${d.base}${scene.path(s)}`);
        await scene.prepare?.(page, s);
        await page.evaluate(() => document.fonts.ready);
        await page.waitForTimeout(300);
        await page.screenshot({ path: `${out}${theme}-${size}-${scene.name}.png` });
        expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(viewport.width);
      });
    }
  }
}
```

Run: `cd web && CLAX_SHOTS=task-02 npx playwright test e2e/shots.spec.ts`
Expected: 12 screenshots in `.superpowers/sdd/2026-09-30-redesign/build-shots/task-02/`, and every test passes.

- [ ] **Step 6: Look at the screenshots**

Open all twelve. Write in the task report what you saw:
- buttons, titles and group heads are in Plex Sans Condensed, sentence case, with no brackets and no tracked capitals;
- comments and meta are in Plex Mono;
- light and dark both read, with no white flashes or unreadable pairs;
- the phone width has no sideways scroll;
- nothing else moved.

Run the voice gate from Global Constraints: no output.


Run: `cd web && npm run perf; echo "exit=$?"`
Expected: `exit=0`. Stop rule: if any of the five measures in `web/perf/budget.json` is over budget (link to first paint `firstPaint`, link to comment ready `commentReady`, frame paint `framePaint`, ready latency `readyLatency`, cold ready latency `coldLatency`, in either frame mode), stop and report all five numbers against their budgets. Lazy-load first; never raise a budget.

- [ ] **Step 7: Gates and staging**

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add web/shell/public/_clax/fonts/ibm-plex-sans-condensed-latin-600.woff2 web/shell/public/_clax/fonts/OFL.txt web/scripts/measure-fallback.mjs \
  web/shell/src/echo-theme.test.ts web/shell/src/theme.css web/shell/src/topbar-style.test.ts web/scripts/bundle-size.mjs web/scripts/bundle-size.test.ts \
  web/perf/bundle-budget.json web/e2e/shots.spec.ts web/e2e/scenes.ts web/e2e/pages/sample-report.html web/e2e/fixtures.ts
git status --short   # staged; the controller commits ("Set Clax in Echo's two voices: Plex Sans Condensed for structure, Plex Mono for words, with a font budget")
```

Do not stage the screenshots. They are evidence for the report, not source.

---

### Task 3: The Echo mark, the theme switch and the keyboard layer

This task adds:
- the Echo symbol, as a favicon, as a component and as markup the skeleton can carry;
- a theme switch that follows the system until the viewer flips it, with no flash before first paint (decided: Q2);
- the shell's keys, with a `?` sheet that loads only when asked for. Keys act only while focus is in the shell (decided: Q6).

Each later task that adds a key also adds its row to the sheet.

**Files:**
- Create: `web/shell/public/_clax/mark.svg`, `web/shell/src/view/mark.ts`, `web/shell/src/ui/Mark.svelte`, `web/shell/src/view/theme-model.ts`, `web/shell/src/view/theme-model.test.ts`, `web/shell/src/ui/ThemeSwitch.svelte`, `web/shell/src/view/keys.ts`, `web/shell/src/view/keys.test.ts`, `web/shell/src/ui/KeysSheet.svelte`, `web/shell/src/echo-chrome.test.ts`
- Modify: `web/shell/index.html`, `web/shell/artifact.html`, `web/shell/src/view/artifact-controller.ts`, `web/shell/src/view/artifact-controller.test.ts`, `web/shell/src/ui/StageIsland.svelte`, `web/shell/src/ui/ThreadCard.svelte`, `web/shell/src/ui/Sidebar.svelte`, `web/shell/src/ui/SidebarIsland.svelte`, `web/shell/src/ui/TopbarIsland.svelte`, `web/shell/src/ui/Gallery.svelte`, `web/shell/src/theme.css`, `web/e2e/scenes.ts`

**Interfaces:**
- `view/mark.ts`: `export const MARK_SVG: string` (30×24, `class="mk"`, `aria-hidden="true"`).
- `ui/Mark.svelte`: `{ size?: "bar" | "hero"; apart?: boolean; playful?: boolean }`. Only the gallery's mark is playful. The top bar's mark stays a plain link to the gallery, so a click there navigates (decided: Q9). With `playful`, it is a button whose click makes the halves meet, and a second click parts them (`aria-pressed`). Without it, the mark is decoration.
- `view/theme-model.ts`: `type Scheme = "light" | "dark"`, `type Choice = Scheme | null`, `THEME_KEY = "clax.theme"`, `readChoice(): Choice`, `systemScheme(): Scheme`, `shownScheme(choice: Choice, system: Scheme): Scheme`, `flip(choice: Choice, system: Scheme): Choice`, `applyChoice(c: Choice, root?: HTMLElement): void`.
- `view/keys.ts`: `type KeyAction = "help" | "comment" | "threads" | "next" | "prev" | "reply" | "send" | "resolve" | "versions" | "tick" | "sendTicked" | "people"`, `keyAction(e: KeyLike): KeyAction | null`, `type KeyRow = { keys: string[]; what: string; action: KeyAction | "escape" }`, `export const KEY_ROWS: KeyRow[]`.
- `ArtifactController`: `ViewState.sheet: "keys" | null` (initially `null`) and `ViewState.replyFocus: number` (initially `0`). New methods: `shortcut(a: KeyAction): void`, `closeSheet(): void`, and `private order(s?: ViewState): Thread[]`, which returns the sidebar's order (`open`, then `detached`).
- `ThreadCard` gains `focusReply?: number`. When the card is selected and the number grows, its reply field takes focus.

- [ ] **Step 1: Pure models, tests first**

`web/shell/src/view/theme-model.test.ts`:

```ts
import { afterEach, describe, expect, it } from "vitest";
import { THEME_KEY, applyChoice, flip, readChoice, shownScheme } from "./theme-model";

afterEach(() => { localStorage.clear(); delete document.documentElement.dataset.theme; });

describe("theme-model", () => {
  it("follows the system until flipped, and flipping back to the system's scheme follows it again", () => {
    expect(shownScheme(null, "dark")).toBe("dark");
    expect(flip(null, "dark")).toBe("light");
    expect(flip("light", "dark")).toBeNull();
    expect(flip(null, "light")).toBe("dark");
    expect(flip("dark", "light")).toBeNull();
  });
  it("stores and applies a choice, and clears both for null", () => {
    applyChoice("dark");
    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(localStorage.getItem(THEME_KEY)).toBe("dark");
    expect(readChoice()).toBe("dark");
    applyChoice(null);
    expect(document.documentElement.dataset.theme).toBeUndefined();
    expect(localStorage.getItem(THEME_KEY)).toBeNull();
    localStorage.setItem(THEME_KEY, "sepia");
    expect(readChoice()).toBeNull();
  });
});
```

`web/shell/src/view/keys.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { KEY_ROWS, keyAction } from "./keys";

const k = (key: string, over: Partial<KeyboardEvent> = {}, target: Element = document.body) =>
  ({ key, metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, isComposing: false, repeat: false, target, ...over }) as unknown as KeyboardEvent;

describe("keys", () => {
  it("maps the shell's keys", () => {
    expect(["?", "c", "C", "t", "j", "k", "Enter", "s", "r"].map(x => keyAction(k(x)))).toEqual(["help", "comment", "comment", "threads", "next", "prev", "reply", "send", "resolve"]);
    expect(keyAction(k("S", { shiftKey: true }))).toBe("sendTicked");
  });
  it("never acts while typing, composing, repeating, or with a modifier", () => {
    const input = document.createElement("input");
    const area = document.createElement("textarea");
    const edit = document.createElement("div");
    edit.contentEditable = "true";
    for (const t of [input, area, edit]) expect(keyAction(k("c", {}, t))).toBeNull();
    expect(keyAction(k("c", { metaKey: true }))).toBeNull();
    expect(keyAction(k("c", { ctrlKey: true }))).toBeNull();
    expect(keyAction(k("c", { altKey: true }))).toBeNull();
    expect(keyAction(k("c", { isComposing: true }))).toBeNull();
    expect(keyAction(k("j", { repeat: true }))).toBeNull();
  });
  it("acts on Enter only outside buttons and links, which Enter already presses", () => {
    expect(keyAction(k("Enter", {}, document.createElement("button")))).toBeNull();
    expect(keyAction(k("Enter", {}, document.createElement("a")))).toBeNull();
  });
  it("lists every row the sheet shows, in order", () => {
    expect(KEY_ROWS.map(r => r.keys.join("+"))).toEqual(["C", "Esc", "T", "J+K", "↵", "S", "R"]);
  });
});
```

Run: `cd web && npx vitest run shell/src/view/theme-model.test.ts shell/src/view/keys.test.ts`
Expected: FAIL (modules not found).

`web/shell/src/view/theme-model.ts`:

```ts
// The theme (spec §8): follow the system, plus a switch that flips light and
// dark. A flip that lands on the system's own scheme clears the choice, so
// the shell follows the system again. The choice is a per-browser
// convenience in localStorage; every access may throw (private windows).
export type Scheme = "light" | "dark";
export type Choice = Scheme | null;
export const THEME_KEY = "clax.theme";

export function readChoice(): Choice {
  try {
    const v = localStorage.getItem(THEME_KEY);
    return v === "light" || v === "dark" ? v : null;
  } catch { return null; }
}

export function systemScheme(): Scheme {
  return typeof matchMedia === "function" && matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

export const shownScheme = (choice: Choice, system: Scheme): Scheme => choice ?? system;

/** The choice after one press of the switch. */
export function flip(choice: Choice, system: Scheme): Choice {
  const next: Scheme = shownScheme(choice, system) === "dark" ? "light" : "dark";
  return next === system ? null : next;
}

/** Applies `c` to the document and remembers it (null forgets). */
export function applyChoice(c: Choice, root: HTMLElement = document.documentElement): void {
  if (c) root.dataset.theme = c;
  else delete root.dataset.theme;
  try {
    if (c) localStorage.setItem(THEME_KEY, c);
    else localStorage.removeItem(THEME_KEY);
  } catch { /* storage unavailable: the choice lasts for this page */ }
}
```

`web/shell/src/view/keys.ts`:

```ts
// The shell's keyboard layer (spec §8, "Keys"). A key acts only when focus is
// in the shell, outside a text field, with no modifier but Shift, and not
// while an input method composes. Keys pressed inside the artifact's frame
// belong to the page and never reach here.
export type KeyAction = "help" | "comment" | "threads" | "next" | "prev" | "reply" | "send" | "resolve" | "versions" | "tick" | "sendTicked" | "people";
export type KeyLike = Pick<KeyboardEvent, "key" | "metaKey" | "ctrlKey" | "altKey" | "shiftKey" | "isComposing" | "repeat" | "target">;
export type KeyRow = { keys: string[]; what: string; action: KeyAction | "escape" };

const MAP: Record<string, KeyAction> = {
  "?": "help", c: "comment", C: "comment", t: "threads", j: "next", k: "prev", Enter: "reply", s: "send", S: "sendTicked", r: "resolve",
};

/** The sheet's rows, in order. Tasks that add a key add its row and its MAP entry. */
export const KEY_ROWS: KeyRow[] = [
  { keys: ["C"], what: "Comment mode: click an element or drag an area", action: "comment" },
  { keys: ["Esc"], what: "Leave comment mode, close a menu", action: "escape" },
  { keys: ["T"], what: "Show or hide threads", action: "threads" },
  { keys: ["J", "K"], what: "Next and previous thread; the page scrolls to its pin", action: "next" },
  { keys: ["↵"], what: "Reply to the selected thread", action: "reply" },
  { keys: ["S"], what: "Send the selected thread to an agent", action: "send" },
  { keys: ["R"], what: "Resolve the selected thread", action: "resolve" },
];

function typing(t: EventTarget | null): boolean {
  if (!(t instanceof Element)) return false;
  const el = t as HTMLElement;
  return el.localName === "input" || el.localName === "textarea" || el.localName === "select" || el.isContentEditable || el.contentEditable === "true";
}

export function keyAction(e: KeyLike): KeyAction | null {
  if (e.metaKey || e.ctrlKey || e.altKey || e.isComposing || e.repeat || typing(e.target)) return null;
  if (e.key === "Enter" && e.target instanceof Element && e.target.closest("button, a, summary, [role=button]")) return null;
  return MAP[e.key] ?? null;
}
```

Run: `cd web && npx vitest run shell/src/view/theme-model.test.ts shell/src/view/keys.test.ts`
Expected: PASS.

- [ ] **Step 2: The mark, the theme script and the favicon**

`web/shell/public/_clax/mark.svg`: copy `.superpowers/sdd/2026-09-30-redesign/marks/1-echo.svg` unchanged. It recolours itself for dark tabs.

`web/shell/src/view/mark.ts`:

```ts
/** The Echo symbol (spec §8, "Look"): people's arc on the left in red-orange,
 * agents' arc on the right in green, the page between them. Decoration: the
 * element around it carries the name. */
export const MARK_SVG = `<svg class="mk" viewBox="0 0 30 24" aria-hidden="true" focusable="false"><path class="l" d="M2 2.5a9.5 9.5 0 0 1 0 19" fill="none" stroke-width="4.2"/><path class="r" d="M28 2.5a9.5 9.5 0 0 0 0 19" fill="none" stroke-width="4.2"/><circle cx="15" cy="12" r="2.6"/></svg>`;
```

`web/shell/src/ui/Mark.svelte`:

```svelte
<script lang="ts">
  // The Echo mark. `playful`: a click makes its halves meet, another parts
  // them (an easter egg; no other effect). `apart`: the empty gallery's mark.
  import { MARK_SVG } from "../view/mark";

  let { size = "bar", apart = false, playful = false }: { size?: "bar" | "hero"; apart?: boolean; playful?: boolean } = $props();
  let meet = $state(false);
</script>

{#if playful}
  <button type="button" class={["mark", size, apart && "apart"]} aria-label="Clax" aria-pressed={meet} onclick={() => { meet = !meet; }}>{@html MARK_SVG}</button>
{:else}
  <span class={["mark", size, apart && "apart"]} role="img" aria-label="Clax">{@html MARK_SVG}</span>
{/if}
```

`{@html}` renders a constant from this module, never data.

In `web/shell/index.html` and `web/shell/artifact.html`, add these as the first children of `<head>` after the `<meta name="viewport">` line:

```html
<script id="clax-theme">try{var t=localStorage.getItem("clax.theme");if(t==="light"||t==="dark")document.documentElement.dataset.theme=t}catch(e){}</script>
<link rel="icon" type="image/svg+xml" href="/_clax/mark.svg">
```

The theme script runs before the inlined CSS applies, so a dark choice never flashes light. In `artifact.html`, it comes before `clax-early`, which must still precede `<!--clax:boot-->` and `<body>`. `bundle-size.mjs` checks that order.

- [ ] **Step 3: Styles**

Append to `web/shell/src/theme.css`:

```css
/* The Echo mark (spec §8). Its halves meet when a playful mark is pressed. */
.mark { display: inline-grid; place-items: center; flex: none; color: var(--fg); }
button.mark { background: none; border: 0; padding: 4px; min-height: 0; width: auto; }
.mk { display: block; width: 30px; height: 24px; overflow: visible; }
.mark.hero .mk { width: 120px; height: 96px; }
.mk path { transition: transform .3s cubic-bezier(.3,1.4,.5,1); }
.mk .l { stroke: var(--you); } .mk .r { stroke: var(--agent); } .mk circle { fill: var(--fg); }
.mark[aria-pressed="true"] .l { transform: translateX(3.5px); } .mark[aria-pressed="true"] .r { transform: translateX(-3.5px); }
.mark.apart .l { transform: translateX(-4px); } .mark.apart .r { transform: translateX(4px); }
/* The keys sheet. */
.keys-backdrop { position: fixed; inset: 0; z-index: 40; background: rgba(26,13,9,.55); display: grid; place-items: center; padding: var(--gutter); }
.keys-panel { background: var(--raised); border: 1px solid var(--border-strong); box-shadow: 0 16px 40px var(--shadow); width: min(520px, 100%); max-height: calc(100dvh - 32px); overflow: auto; padding: 18px 20px 20px; }
.keys-panel h2 { margin: 0 0 4px; font-size: 22px; }
.keys-panel .sub { margin: 0 0 14px; color: var(--muted); font-size: 12px; }
.keys-panel dl { display: grid; grid-template-columns: auto 1fr; gap: 8px 16px; margin: 0; font-size: 13px; align-items: baseline; }
.keys-panel dt { display: flex; gap: 4px; justify-content: flex-end; }
.keys-panel kbd { font: 600 12px/20px var(--mono); min-width: 24px; text-align: center; border: 1px solid var(--border-strong); border-bottom-width: 2px; padding: 0 6px; background: var(--card); }
.keys-panel dd { margin: 0; }
.keys-panel .foot { margin-top: 16px; display: flex; justify-content: flex-end; }
@media (prefers-reduced-motion: reduce) { .mk path { transition: none; } }
```

- [ ] **Step 4: The components and the controller, tests first**

`web/shell/src/echo-chrome.test.ts`:

```ts
import { describe, expect, it, vi } from "vitest";
import { flush, mount } from "./test/svelte";
import KeysSheet from "./ui/KeysSheet.svelte";
import Mark from "./ui/Mark.svelte";
import { KEY_ROWS } from "./view/keys";
import ThemeSwitch from "./ui/ThemeSwitch.svelte";

describe("Echo chrome", () => {
  it("a playful mark's halves meet on one click and part on the next", () => {
    const m = mount(Mark, { playful: true });
    const b = m.root.querySelector("button.mark") as HTMLButtonElement;
    expect(b.getAttribute("aria-pressed")).toBe("false");
    flush(() => b.click());
    expect(b.getAttribute("aria-pressed")).toBe("true");
    flush(() => b.click());
    expect(b.getAttribute("aria-pressed")).toBe("false");
    m.unmount();
  });

  it("the switch flips the shown scheme and names what it will do", () => {
    vi.stubGlobal("matchMedia", (q: string) => ({ matches: q.includes("dark") ? false : true, addEventListener() {}, removeEventListener() {} }));
    const m = mount(ThemeSwitch, {});
    const b = m.root.querySelector("button") as HTMLButtonElement;
    expect(b.getAttribute("aria-label")).toBe("Switch to dark");
    flush(() => b.click());
    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(b.getAttribute("aria-label")).toBe("Switch to light");
    flush(() => b.click());
    expect(document.documentElement.dataset.theme).toBeUndefined();
    m.unmount();
    vi.unstubAllGlobals();
  });

  it("the keys sheet is a labelled dialog of the rows, closed by its button and by Escape", () => {
    const onClose = vi.fn();
    const m = mount(KeysSheet, { onClose });
    const d = m.root.querySelector("[role=dialog]")!;
    expect(d.getAttribute("aria-label")).toBe("Keyboard shortcuts");
    expect(d.querySelectorAll("dt")).toHaveLength(KEY_ROWS.length);
    flush(() => d.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })));
    flush(() => (d.querySelector(".foot button") as HTMLButtonElement).click());
    expect(onClose).toHaveBeenCalledTimes(2);
    m.unmount();
  });
});
```

`web/shell/src/ui/ThemeSwitch.svelte`:

```svelte
<script lang="ts">
  import { applyChoice, flip, readChoice, shownScheme, systemScheme } from "../view/theme-model";

  let choice = $state(readChoice());
  const shown = $derived(shownScheme(choice, systemScheme()));
  const press = () => { choice = flip(choice, systemScheme()); applyChoice(choice); };
</script>

<button type="button" class="icon theme-switch" aria-label={shown === "dark" ? "Switch to light" : "Switch to dark"} title="Light or dark" onclick={press}>
  <svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="8" cy="8" r="6.25" fill="none" stroke="currentColor" stroke-width="1.5"/><path d="M8 1.75a6.25 6.25 0 0 1 0 12.5z" fill="currentColor"/></svg>
</button>
```

`web/shell/src/ui/KeysSheet.svelte`:

```svelte
<script lang="ts">
  // The `?` sheet (spec §8, "Keys"), loaded on first use. Focus moves to its
  // Close button; Escape or Close returns it to where it was.
  import { KEY_ROWS } from "../view/keys";

  let { onClose }: { onClose(): void } = $props();
  const back = document.activeElement as HTMLElement | null;
  const close = () => { onClose(); back?.focus?.(); };
  const focus = (el: HTMLElement) => { el.focus(); };
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="keys-backdrop" onclick={e => { if (e.target === e.currentTarget) close(); }}>
  <!-- Escape closes the dialog. -->
  <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
  <div class="keys-panel" role="dialog" aria-modal="true" aria-label="Keyboard shortcuts" tabindex="-1"
    onkeydown={e => { if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); close(); } }}>
    <h2>Keyboard</h2>
    <p class="sub">Press ? to open this. Esc closes it. Keys work while the page does not have focus.</p>
    <dl>
      {#each KEY_ROWS as r (r.keys.join("+"))}
        <dt>{#each r.keys as key (key)}<kbd>{key}</kbd>{/each}</dt><dd>{r.what}</dd>
      {/each}
    </dl>
    <div class="foot"><button type="button" {@attach focus} onclick={close}>Close</button></div>
  </div>
</div>
```

In `view/artifact-controller.test.ts`, give the harness a seed, so this test and later ones (Tasks 18 and 23) can supply threads, artifact fields, versions, attention and extra routes. Replace `started()` with:

```ts
type Seed = { threads?: Thread[]; artifact?: Record<string, unknown>; versions?: unknown[]; attention?: unknown; routes?: (url: string, init?: RequestInit) => unknown };
const thread = (id: string, over: Partial<Thread> = {}): Thread => ({
  id, artifact_id: ID, version_n: 1, status: "open", sent_to_agent: false, has_clip: false, clip_url: null, created_at: "2026-09-30T10:00:00.000Z",
  resolved_at: null, resolved_by: null, feedback_state: null, comments: [{ id: `${id}c`, thread_id: id, author_kind: "viewer", author_name: "alex", via_harness: null, body: "x", created_at: "2026-09-30T10:00:00.000Z" }],
  anchor: { kind: "element", selector: "h2", quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" }, ...over,
});

async function started(seed: Seed = {}) {
  vi.stubGlobal("EventSource", FakeES);
  vi.stubGlobal("fetch", vi.fn(async (url: string, init?: RequestInit) => {
    const own = seed.routes?.(url, init);
    if (own !== undefined) return new Response(JSON.stringify(own));
    return new Response(JSON.stringify(
      url.includes("/threads") ? { threads: seed.threads ?? [], next_cursor: null }
      : url.startsWith("/api/viewers") ? { viewer: { public_id: "u_1", display_name: null, created_at: "x" } }
      : url === "/api/token" ? { token: "tk" }
      : { ...loaded, artifact: { ...loaded.artifact, ...seed.artifact }, versions: seed.versions ?? loaded.versions, ...(seed.attention ? { attention: seed.attention } : {}) }));
  }));
  // …the rest of the body as before.
}
```

Import `type Thread` from `../threads`. Existing calls (`started()`) keep their behaviour. Then add:

```ts
  it("acts on shell keys: C, T, J and K, ?, and Escape closes the sheet before leaving comment mode", async () => {
    const { ctl } = await started({ threads: [thread("t1"), thread("t2")] });
    await vi.waitFor(() => expect(ctl.state.get().threads.length).toBe(2));
    const key = (k: string, init: KeyboardEventInit = {}) => dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, ...init }));
    key("c");
    expect(ctl.state.get().commenting).toBe(true);
    const panel = ctl.state.get().panel;
    key("t");
    expect(ctl.state.get().panel).toBe(!panel);
    key("j");
    expect(ctl.state.get().selected).toBe(ctl.state.get().threads[0].id);
    key("j");
    expect(ctl.state.get().selected).toBe(ctl.state.get().threads[1].id);
    key("k");
    expect(ctl.state.get().selected).toBe(ctl.state.get().threads[0].id);
    key("?", { shiftKey: true });
    expect(ctl.state.get().sheet).toBe("keys");
    key("Escape");
    expect(ctl.state.get()).toMatchObject({ sheet: null, commenting: true });
    key("Escape");
    expect(ctl.state.get().commenting).toBe(false);
    ctl.dispose();
  });
```

Run: `cd web && npx vitest run shell/src/echo-chrome.test.ts shell/src/view/artifact-controller.test.ts`
Expected: FAIL.

In `view/artifact-controller.ts`:
- `ViewState` gains `/** The sheet over the view: the keys (spec §8), or none. */ sheet: "keys" | null;` and `/** Bumped to move focus to the selected thread's reply field. */ replyFocus: number;`. The initial values are `null` and `0`.
- Add, near `toggleComment`:

```ts
  /** The sidebar's order: open threads, then detached ones (J, K, ranges). */
  private order(s: ViewState = this.s): Thread[] {
    const sec = sidebarSections(s.threads, s.resolved, s.file, f => this.holds(f, s));
    return [...sec.open, ...sec.detached];
  }

  closeSheet(): void { this.set({ sheet: null }); }

  /** A shell key (spec §8, "Keys"); `keyAction` decided it applies. */
  shortcut(a: KeyAction): void {
    const s = this.s;
    if (!viewReady(s) || s.deleted) return;
    const sel = s.threads.find(t => t.id === s.selected) ?? null;
    switch (a) {
      case "help": this.set({ sheet: "keys" }); return;
      case "comment": this.toggleComment(); return;
      case "threads": this.togglePanel(); return;
      case "next": case "prev": {
        const list = this.order(s);
        if (!list.length) return;
        const i = sel ? list.findIndex(t => t.id === sel.id) : -1;
        const j = a === "next" ? (i + 1) % list.length : (i <= 0 ? list.length - 1 : i - 1);
        this.set({ panel: true });
        this.selectThread(list[j]);
        return;
      }
      case "reply": if (sel) this.set(x => ({ panel: true, replyFocus: x.replyFocus + 1 })); return;
      case "send": if (sel && sel.status === "open" && !sel.sent_to_agent) this.sendThread(sel); return;
      case "resolve": if (sel && sel.status === "open") this.resolveThread(sel); return;
      default: return; // added with their features (versions, tick, sendTicked, people)
    }
  }
```

- In `listen()`'s `onKey`, before the `else if (e.key === "Escape" …)` branch, add a branch for keydown that is not forwarded:

```ts
      } else if (e.type === "keydown" && e.key !== "Escape") {
        const a = keyAction(e);
        if (a) { e.preventDefault(); this.shortcut(a); }
```

- Change the Escape branch to close the sheet first: `if (this.s.sheet) this.set({ sheet: null }); else this.set({ commenting: false });`.
- Import `keyAction` and `type KeyAction` from `./keys`, and `sidebarSections` from `./sidebar-model`.

`ui/StageIsland.svelte`, at the end of the `{#if viewReady(s)}` block:

```svelte
  {#if s.sheet === "keys"}
    {#await import("./KeysSheet.svelte") then { default: KeysSheet }}<KeysSheet onClose={() => ctl.closeSheet()} />{/await}
  {/if}
```

`ui/ThreadCard.svelte`: add `focusReply?: number` to `Props`. Add `let replyInput: HTMLInputElement | undefined = $state();` and `bind:this={replyInput}` on the reply `<input>`, and:

```ts
  $effect(() => { if ((focusReply ?? 0) > 0 && selected === t.id) replyInput?.focus(); });
```

`ui/Sidebar.svelte` takes `focusReply?: number` and passes it to every `ThreadCard`. `ui/SidebarIsland.svelte` passes `focusReply={s.replyFocus}`.

`ui/TopbarIsland.svelte`: add `<ThemeSwitch />` as the island's last control. Task 4 places it for good. `ui/Gallery.svelte`: add `<ThemeSwitch />` as the header's last child, and replace `<h1>Clax</h1>` with `<Mark playful /><h1>Clax</h1>`.

Run: `cd web && npx vitest run && npm run lint && npm run typecheck`
Expected: PASS.

- [ ] **Step 5: Budget, scenes, screenshots**

Run: `cd web && npm run build && node scripts/bundle-size.mjs; echo "exit=$?"`
Expected: `exit=0`. `KeysSheet` is its own chunk: `grep -l "Keyboard shortcuts" dist/_clax/shell/*.js` names a file outside `manifest["artifact.html"]`'s closure.

Append to `SCENES` in `web/e2e/scenes.ts`:

```ts
  { name: "keys", path: s => `/a/${s.aid}`, prepare: async page => { await page.locator("body").press("Shift+?"); await page.getByRole("dialog", { name: "Keyboard shortcuts" }).waitFor(); } },
```

Run: `cd web && CLAX_SHOTS=task-03 npx playwright test e2e/shots.spec.ts`
Expected: PASS.

Look at the screenshots and report:
- the gallery bar shows the mark;
- the keys sheet is centred, readable in both themes, and fits 390px;
- the switch is in both bars.

Then do these by hand in a headed Chromium against the same scratch daemon (`npx playwright open`), and report each:
- the tab shows the Echo favicon;
- clicking the gallery's mark makes the halves meet, and a second click parts them;
- the switch flips the theme, survives a reload, and flipping back to the system's scheme clears `localStorage["clax.theme"]`;
- with the system in dark and no choice stored, reloading shows no light flash.


Run: `cd web && npm run perf; echo "exit=$?"`
Expected: `exit=0`. Stop rule: if any of the five measures in `web/perf/budget.json` is over budget (link to first paint `firstPaint`, link to comment ready `commentReady`, frame paint `framePaint`, ready latency `readyLatency`, cold ready latency `coldLatency`, in either frame mode), stop and report all five numbers against their budgets. Lazy-load first; never raise a budget.

- [ ] **Step 6: Gates and staging**

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add web/shell/public/_clax/mark.svg web/shell/src/view/mark.ts web/shell/src/ui/Mark.svelte web/shell/src/view/theme-model.ts web/shell/src/view/theme-model.test.ts \
  web/shell/src/ui/ThemeSwitch.svelte web/shell/src/view/keys.ts web/shell/src/view/keys.test.ts web/shell/src/ui/KeysSheet.svelte web/shell/src/echo-chrome.test.ts \
  web/shell/index.html web/shell/artifact.html web/shell/src/view/artifact-controller.ts web/shell/src/view/artifact-controller.test.ts web/shell/src/ui/StageIsland.svelte \
  web/shell/src/ui/ThreadCard.svelte web/shell/src/ui/Sidebar.svelte web/shell/src/ui/SidebarIsland.svelte web/shell/src/ui/TopbarIsland.svelte web/shell/src/ui/Gallery.svelte \
  web/shell/src/theme.css web/e2e/scenes.ts
git status --short   # staged; the controller commits ("Add the Echo mark, a light and dark switch that follows the system, and the shell's keys with a ? sheet")
```

---

### Task 4: The top bar and comment mode in Echo

This task builds the artifact view's 60px top bar from the mockup. The daemon serves a new skeleton:
- the mark links to the gallery;
- the title sits over the "published by" line;
- Comment turns red-orange, with its C keycap and a 3px red-orange rule under the bar;
- Threads shows its count;
- open raw and copy link move into a ⋯ menu;
- the theme switch sits at the end.

At phone width a Page | Threads switch sits at the foot. The roster and summary slot is laid out empty here, and Task 16 fills it. The version `<select>` stays, restyled, until Task 18 replaces it with the version menu.

**Files:**
- Create: `web/shell/src/ui/MoreMenu.svelte`, `web/shell/src/ui/PhoneTabs.svelte`, `web/shell/src/topbar.test.ts`, `web/e2e/echo.spec.ts`
- Modify: `web/shell/src/view/skeleton.ts`, `web/shell/src/view/skeleton.test.ts`, `web/shell/artifact.html`, `web/shell/src/artifact.ts`, `web/shell/src/view/gallery-model.ts`, `web/shell/src/view/gallery-model.test.ts`, `web/shell/src/ui/Gallery.svelte`, `web/shell/src/gallery.test.ts`, `web/shell/src/ui/TopbarIsland.svelte`, `web/shell/src/topbar-style.test.ts`, `web/shell/src/theme.css`, `web/e2e/viewer.spec.ts` (selectors only)

**Interfaces:**
- `SKELETON_HTML` becomes `<header class="topbar"><a href="/" class="home" aria-label="Gallery">${MARK_SVG}</a><div class="ttl"><h1>Clax</h1><span class="by"></span></div><div class="island"></div></header><div class="viewer"><div class="stage"><!--clax:frame--><div class="island"></div></div><div class="island"></div></div>`. `Skeleton` gains `topbar: HTMLElement` and `by: HTMLElement`, and `title` is found with `.topbar h1`. The daemon's `<h1>Clax</h1>` marker (`boot.rs` `TITLE_MARK`) is unchanged.
- `gallery-model.ts`: `publisherText(a)` reads `published by <harness>` when `a.owner_harness` is set, else `published from the command line` (previously null for the command line). It keys on `owner_harness`, which `with_owner` sets and the bootstrap keeps; `owner_session_id` is stripped from the bootstrap (and, from Task 15, from every token-less artifact view). The gallery's own check moves to `a.owner_harness` too.
- `pageFollows` also sets `sk.by` and toggles `commenting` on `sk.topbar`.
- `MoreMenu.svelte`: `{ rawHref: string | null; canCopy: boolean; onCopy(): void }`. `PhoneTabs.svelte`: `{ panel: boolean; open: number; onPage(): void; onThreads(): void }`.

- [ ] **Step 1: Tests first**

`web/shell/src/topbar.test.ts`:

```ts
import { describe, expect, it, vi } from "vitest";
import { flush, mount } from "./test/svelte";
import MoreMenu from "./ui/MoreMenu.svelte";
import PhoneTabs from "./ui/PhoneTabs.svelte";

describe("Echo top bar parts", () => {
  it("the more menu holds open raw and copy link, and closes on Escape with focus back", () => {
    const onCopy = vi.fn();
    const m = mount(MoreMenu, { rawHref: "/c/x/v/1/", canCopy: true, onCopy });
    const b = m.root.querySelector("button.icon") as HTMLButtonElement;
    expect(b.getAttribute("aria-label")).toBe("Open raw or copy link");
    expect(b.getAttribute("aria-expanded")).toBe("false");
    flush(() => b.click());
    const menu = m.root.querySelector("[role=menu]")!;
    expect(menu.querySelector("a")!.getAttribute("href")).toBe("/c/x/v/1/");
    expect(menu.querySelector("a")!.getAttribute("target")).toBe("_blank");
    flush(() => (menu.querySelector("button") as HTMLButtonElement).click());
    expect(onCopy).toHaveBeenCalled();
    flush(() => b.click());
    flush(() => m.root.querySelector("[role=menu]")!.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })));
    expect(m.root.querySelector("[role=menu]")).toBeNull();
    expect(document.activeElement).toBe(b);
    m.unmount();
  });

  it("the more menu shows open raw as plain text for a deleted artifact", () => {
    const m = mount(MoreMenu, { rawHref: null, canCopy: false, onCopy: vi.fn() });
    flush(() => (m.root.querySelector("button.icon") as HTMLButtonElement).click());
    expect(m.root.querySelector("[role=menu] a")).toBeNull();
    expect(m.root.querySelector("[role=menu]")!.textContent).toContain("Open raw");
    m.unmount();
  });

  it("phone tabs switch between the page and the threads", () => {
    const onPage = vi.fn();
    const onThreads = vi.fn();
    const m = mount(PhoneTabs, { panel: false, open: 3, onPage, onThreads });
    const [page, threads] = Array.from(m.root.querySelectorAll("button"));
    expect(page.getAttribute("aria-pressed")).toBe("true");
    expect(threads.textContent).toContain("3");
    flush(() => threads.click());
    expect(onThreads).toHaveBeenCalled();
    m.unmount();
  });
});
```

In `view/skeleton.test.ts`, add `expect(a.by.parentElement?.classList.contains("ttl")).toBe(true);`, `expect(a.topbar.querySelector("a.home svg.mk")).not.toBeNull();` and `expect(a.title.localName).toBe("h1");` to the first test.

In `topbar-style.test.ts`, replace the test's assertions about bracketed actions with these:
- a rule matching `.topbar.commenting` declares `box-shadow: inset 0 -3px 0 var(--you)`;
- the pressed Comment button matches a rule declaring `background: var(--you)`;
- no rule matching an element in the island declares `text-transform`.

Run: `cd web && npx vitest run shell/src/topbar.test.ts shell/src/view/skeleton.test.ts shell/src/topbar-style.test.ts`
Expected: FAIL.

- [ ] **Step 2: The skeleton and the page parts**

`view/skeleton.ts`:

```ts
import { MARK_SVG } from "./mark";

/** The artifact view's static layout (spec §8, "Top bar"). `.island`
 * elements are `display: contents` mount points for the top bar's controls,
 * the stage's overlays and the sidebar; `<!--clax:frame-->` marks where the
 * daemon may put the content frame; `<h1>Clax</h1>` is where it writes the
 * title. The daemon may send this markup in the page, and `skeleton` adopts it. */
export const SKELETON_HTML = `<header class="topbar"><a href="/" class="home" aria-label="Gallery">${MARK_SVG}</a><div class="ttl"><h1>Clax</h1><span class="by"></span></div><div class="island"></div></header><div class="viewer"><div class="stage"><!--clax:frame--><div class="island"></div></div><div class="island"></div></div>`;

export type Skeleton = { page: HTMLElement; topbar: HTMLElement; title: HTMLElement; by: HTMLElement; viewer: HTMLElement; stage: HTMLElement; topbarIsland: HTMLElement; stageIsland: HTMLElement; sidebarIsland: HTMLElement };
```

In `skeleton()`, return `topbar: q(".topbar")`, `title: q(".topbar h1")` and `by: q(".topbar .by")`.

`web/shell/artifact.html`: replace the `<div id="app">…</div>` line with the new markup, `<div id="app"><div class="page">` + `SKELETON_HTML` + `</div></div>`, with `${MARK_SVG}` written out literally. `skeleton.test.ts` checks that the two agree.

`artifact.ts` `pageFollows`: after the title line, add:

```ts
  setText(sk.by, !s.error && s.data ? publisherText(s.data.artifact) : "");
  if (sk.topbar.classList.contains("commenting") !== s.commenting) sk.topbar.classList.toggle("commenting", s.commenting);
```

Import `publisherText` from `./view/gallery-model`. Change it to:

```ts
/** Who published the artifact: its owner session's harness, else the command line. */
export function publisherText(a: Artifact): string {
  return a.owner_harness ? `published by ${a.owner_harness}` : "published from the command line";
}
```

In `Gallery.svelte`, the `{#if by}` test becomes `{#if a.owner_harness}`. Update the tests that pinned the old wording:
- `view/gallery-model.test.ts`: `{ owner_session_id: "s", owner_harness: "codex" }` now reads `published by codex`, `{ owner_harness: null }` reads `published from the command line`, and an artifact with neither reads the same;
- `gallery.test.ts` (the two `.publisher` assertions): `published by claude-code`.

- [ ] **Step 3: The components and the island**

`web/shell/src/ui/MoreMenu.svelte`:

```svelte
<script lang="ts">
  import { tick } from "svelte";

  let { rawHref, canCopy, onCopy }: { rawHref: string | null; canCopy: boolean; onCopy(): void } = $props();
  let open = $state(false);
  let button: HTMLButtonElement | undefined = $state();
  let menu: HTMLDivElement | undefined = $state();
  async function toggle() {
    open = !open;
    if (open) { await tick(); menu?.querySelector<HTMLElement>("a, button")?.focus(); }
  }
  function close() { open = false; button?.focus(); }
  function outside(e: PointerEvent) {
    if (open && !menu?.contains(e.target as Node) && !button?.contains(e.target as Node)) open = false;
  }
</script>

<svelte:window onpointerdown={outside} />

<div class="more hide-sm">
  <button type="button" class="icon" bind:this={button} aria-label="Open raw or copy link" aria-haspopup="menu" aria-expanded={open} onclick={toggle}>
    <svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="3" cy="8" r="1.4" fill="currentColor"/><circle cx="8" cy="8" r="1.4" fill="currentColor"/><circle cx="13" cy="8" r="1.4" fill="currentColor"/></svg>
  </button>
  {#if open}
    <!-- Escape closes the menu. -->
    <!-- svelte-ignore a11y_interactive_supports_focus -->
    <div class="more-menu" role="menu" bind:this={menu} onkeydown={e => { if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); close(); } }}>
      {#if rawHref}
        <a role="menuitem" href={rawHref} target="_blank" rel="noopener" onclick={() => { open = false; }}>Open raw</a>
      {:else}
        <span class="muted" role="menuitem" aria-disabled="true">Open raw</span>
      {/if}
      {#if canCopy}<button type="button" role="menuitem" class="ghost" onclick={() => { onCopy(); close(); }}>Copy link</button>{/if}
    </div>
  {/if}
</div>
```

`web/shell/src/ui/PhoneTabs.svelte`:

```svelte
<script lang="ts">
  let { panel, open, onPage, onThreads }: { panel: boolean; open: number; onPage(): void; onThreads(): void } = $props();
</script>

<nav class="phone-tabs" aria-label="Page or threads">
  <button type="button" aria-pressed={!panel} onclick={onPage}>Page</button>
  <button type="button" aria-pressed={panel} onclick={onThreads}>Threads <span class="cnt">{open}</span></button>
</nav>
```

`ui/TopbarIsland.svelte`, the body:

```svelte
{#if viewReady(s)}
  {@const shown = ctl.shown(s)}
  {@const latest = ctl.latest(s)}
  <div class="who-slot"></div>
  <button class="comment" aria-pressed={s.commenting} disabled={s.deleted} onclick={() => ctl.toggleComment()}>Comment <span class="kc" aria-hidden="true">C</span></button>
  <button class="threads hide-sm" aria-pressed={s.panel} onclick={() => ctl.togglePanel()}>Threads <span class="cnt">{ctl.openCount(s)}</span></button>
  {#if !s.narrow}<ViewerName setNotice={ctl.setNotice} onViewer={v => ctl.setMe(v)} />{/if}
  <select class="version hide-sm" value={shown} disabled={s.deleted} aria-label="Version" onchange={e => ctl.chooseVersion(Number(e.currentTarget.value))}>
    {#each s.data.versions as v (v.n)}
      <option value={v.n}>v{v.n}{v.n === latest ? ` of ${latest}` : ""}{v.label ? ` · ${v.label}` : ""}</option>
    {/each}
  </select>
  <MoreMenu rawHref={s.deleted ? null : ctl.rawHref(s)} canCopy={!!navigator.clipboard && !s.deleted} onCopy={() => ctl.copyLink()} />
  <span class="hide-sm"><ThemeSwitch /></span>
  <PhoneTabs panel={s.panel} open={ctl.openCount(s)} onPage={() => { if (s.panel) ctl.togglePanel(); }} onThreads={() => { if (!s.panel) ctl.togglePanel(); }} />
{/if}
```

The keycap carries `aria-hidden`, so the button's accessible name stays "Comment" and existing e2e locators `getByRole("button", { name: "Comment" })` keep working. In `web/e2e/viewer.spec.ts` and elsewhere, change only locators that named `open raw` or `copy link` as top bar items: they now open the ⋯ menu first (`getByRole("button", { name: "Open raw or copy link" })`), then `getByRole("menuitem", { name: "Copy link" })`.

- [ ] **Step 4: Styles**

The gallery's header still uses `class="topbar"`, and the rules below are for the artifact view only. So that the gallery is not left unstyled until Task 6, change `Gallery.svelte`'s `<header class="topbar">` to `<header class="gbar">` now, and add the gallery bar's first rules (Task 6 replaces them with its full set):

```css
.gbar { height: 60px; display: flex; align-items: center; gap: 12px; padding: 0 var(--gutter); background: var(--card); border-bottom: 1px solid var(--border); }
.gbar h1 { margin: 0; font-size: 22px; }
```

In `web/shell/src/theme.css`, delete the old top bar rules: every rule whose selector starts with `.topbar` (`.topbar`, `.topbar h1`, the `:first-child` rules, `.topbar select`, `.topbar button`, the island action rules, the pressed-button rules) and the `max-width: 480px` `.topbar` lines. Add:

```css
/* The top bar (spec §8): 60px; the title over its by-line in the condensed
   face; Comment red-orange when on, with a 3px rule under the bar. */
.topbar { position: relative; display: flex; align-items: center; gap: 12px; height: 60px; flex: none; padding: 0 var(--gutter); background: var(--card); border-bottom: 1px solid var(--border); }
.topbar.commenting { box-shadow: inset 0 -3px 0 var(--you); }
.topbar > .home { display: inline-grid; place-items: center; min-width: 32px; min-height: 32px; }
.ttl { min-width: 0; flex: 1; display: flex; flex-direction: column; gap: 2px; }
.ttl h1 { margin: 0; font-size: 20px; line-height: 1.1; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.ttl .by { font-size: 11.5px; color: var(--muted); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.who-slot:empty { display: none; }
.topbar button.comment[aria-pressed="true"] { background: var(--you); border-color: var(--you); color: var(--on-you); }
.topbar button.threads[aria-pressed="true"] { background: var(--comment-hl); border-color: var(--accent); }
.cnt { font: 600 11px/18px var(--mono); min-width: 18px; height: 18px; border-radius: 9px; background: var(--you); color: var(--on-you); text-align: center; padding: 0 4px; }
.topbar select.version { font: 600 18px/1 var(--grot); height: 36px; max-width: 40vw; }
.more { position: relative; }
.more-menu { position: absolute; right: 0; top: calc(100% + 8px); z-index: 20; min-width: 180px; background: var(--raised); border: 1px solid var(--border-strong); box-shadow: 0 14px 40px var(--shadow); padding: 6px 0; display: flex; flex-direction: column; }
.more-menu > a, .more-menu > button, .more-menu > span { display: block; width: 100%; padding: 8px 14px; text-align: left; font: 600 14px/1.2 var(--grot); justify-content: flex-start; min-height: 0; border: 0; }
.more-menu > a:hover, .more-menu > button:hover { background: var(--bg); }
.phone-tabs { display: none; }
@media (max-width: 700px) {
  .topbar { gap: 8px; padding: 0 10px; height: 56px; }
  .topbar .hide-sm, .ttl .by, .topbar .kc { display: none; }
  .ttl h1 { font-size: 17px; }
  .phone-tabs { display: flex; position: fixed; left: 0; right: 0; bottom: 0; height: 52px; z-index: 12; background: var(--card); border-top: 1px solid var(--border-strong); }
  .phone-tabs button { flex: 1; border: 0; background: none; font-size: 16px; color: var(--muted); min-height: 52px; }
  .phone-tabs button[aria-pressed="true"] { color: var(--fg); box-shadow: inset 0 3px 0 var(--fg); }
  .viewer { margin-bottom: 52px; }
}
```

The port's `@media (max-width: 700px) { .sidebar { … } }` rule stays as it is. At that width the sidebar covers the stage, above the tabs (`bottom: 52px` is added to its `inset` there).

- [ ] **Step 5: Browser tests**

`web/e2e/echo.spec.ts`:

```ts
import { test, expect } from "@playwright/test";
import { openArtifact, publishAs, registerSession, startDaemon } from "./fixtures";

let d: Awaited<ReturnType<typeof startDaemon>>;
test.beforeAll(async () => { test.setTimeout(180_000); d = await startDaemon(); });
test.afterAll(async () => { await d?.stop(); });

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: the top bar reads Echo, and comment mode shows the red-orange rule`, async ({ page }) => {
    const s = await registerSession(d.base, d.token, "claude", `echo-${mode}`);
    const { artifact } = await publishAs(d.base, d.token, s.id, `Echo ${mode}`, { "index.html": "<main><h2>Goals</h2></main>" });
    await openArtifact(page, d.base, artifact.id, 1, mode);
    await expect(page.locator(".topbar h1")).toHaveText(`Echo ${mode}`);
    await expect(page.locator(".topbar .by")).toHaveText("published by claude");
    const h1Font = await page.locator(".topbar h1").evaluate(e => getComputedStyle(e).fontFamily);
    expect(h1Font).toContain("IBM Plex Sans Condensed");
    const comment = page.getByRole("button", { name: "Comment", exact: true });
    await comment.click();
    await expect(comment).toHaveAttribute("aria-pressed", "true");
    await expect(page.locator(".topbar")).toHaveClass(/commenting/);
    expect(await page.locator(".topbar").evaluate(e => getComputedStyle(e).boxShadow)).toMatch(/inset 0px -3px 0px/);
    await page.keyboard.press("Escape");
    await expect(page.locator(".topbar")).not.toHaveClass(/commenting/);
  });
}

test("at phone width the bar keeps the mark, title and Comment, and tabs switch to threads", async ({ page }) => {
  const s = await registerSession(d.base, d.token, "claude", "echo-phone");
  const { artifact } = await publishAs(d.base, d.token, s.id, "A long title that has to fit a phone without pushing anything sideways", { "index.html": "<main><h2>Goals</h2></main>" });
  await page.setViewportSize({ width: 390, height: 844 });
  await openArtifact(page, d.base, artifact.id, 1, "subdomain");
  await expect(page.locator(".topbar a.home")).toBeVisible();
  await expect(page.getByRole("button", { name: "Comment", exact: true })).toBeVisible();
  await expect(page.locator(".topbar select.version")).toBeHidden();
  await page.getByRole("navigation", { name: "Page or threads" }).getByRole("button", { name: /Threads/ }).click();
  await expect(page.locator("aside.sidebar")).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
});
```

Run: `cd web && npx vitest run && npm run lint && npm run typecheck && npm run build && node scripts/bundle-size.mjs && npx playwright test e2e/echo.spec.ts e2e/viewer.spec.ts e2e/boot.spec.ts; echo "exit=$?"`
Expected: `exit=0`. `boot.spec.ts` proves the daemon still adopts the served skeleton and frame.

Also run the Rust side that embeds `artifact.html`: `cargo test -p clax-server boot`. Expected: PASS, since the markers are unchanged.

- [ ] **Step 6: Time to usable, screenshots, and a look**

Run: `cd web && npm run perf; echo "exit=$?"`
Expected: `exit=0`. Stop rule: if any of the five measures in `web/perf/budget.json` is over budget (link to first paint `firstPaint`, link to comment ready `commentReady`, frame paint `framePaint`, ready latency `readyLatency`, cold ready latency `coldLatency`, in either frame mode), stop and report all five numbers against their budgets. Lazy-load first; never raise a budget. Do not move work earlier or later to chase the numbers.

Run: `cd web && CLAX_SHOTS=task-04 npx playwright test e2e/shots.spec.ts`
Expected: PASS.

Report what the `view` and `comment` shots show in both themes and both sizes, against `concept-3-echo/shots/*-view.png` and `*-comment.png`:
- the bar height;
- the mark;
- the title over its by-line;
- Comment and its keycap, red-orange when on, with the 3px rule;
- the Threads count;
- the ⋯ menu;
- the switch;
- at phone width, only the mark, the title and Comment, with the tabs at the foot.

- [ ] **Step 7: Gates and staging**

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add web/shell/src/ui/MoreMenu.svelte web/shell/src/ui/PhoneTabs.svelte web/shell/src/topbar.test.ts web/e2e/echo.spec.ts web/shell/src/view/skeleton.ts \
  web/shell/src/view/skeleton.test.ts web/shell/artifact.html web/shell/src/artifact.ts web/shell/src/view/gallery-model.ts web/shell/src/ui/Gallery.svelte \
  web/shell/src/view/gallery-model.test.ts web/shell/src/gallery.test.ts \
  web/shell/src/ui/TopbarIsland.svelte web/shell/src/topbar-style.test.ts web/shell/src/theme.css web/e2e/viewer.spec.ts
git add -u web/e2e
git status --short   # staged; the controller commits ("Lay out the artifact top bar in Echo: the mark, the title over its by-line, a red-orange Comment with its rule, and a phone tab bar")
```

---

### Task 5: Thread cards and the sidebar in Echo: mirrored messages, the history line, the outdated tag

Echo thread cards, built from data the shell already has:
- people's words carry a red-orange rule on the left, and an agent's a green rule on the right;
- one line of version-tagged history;
- an `outdated` tag when the element changed in a later version but still exists;
- `Send to <agent>` named after the publishing agent;
- group heads with half-disc swatches, with Detached and Resolved collapsed into a tail;
- Echo pins.

Later tasks add events to the history line: working (Task 16), addressed (Task 18), sends (Task 23).

**Files:**
- Create: `web/shell/src/view/history-model.ts`, `web/shell/src/view/history-model.test.ts`
- Modify: `web/shell/src/ui/ThreadCard.svelte`, `web/shell/src/ui/Sidebar.svelte`, `web/shell/src/ui/SidebarIsland.svelte`, `web/shell/src/ui/Pins.svelte`, `web/shell/src/view/sidebar-model.ts`, `web/shell/src/sidebar.test.ts`, `web/shell/src/artifact.test.ts`, `web/e2e/comments.spec.ts`, `web/e2e/comment-loop.spec.ts`, `web/shell/src/theme.css`, `web/e2e/scenes.ts`, and the e2e specs whose locators name `Send to agent`, or click a card in Resolved or Detached (`grep -rln "Send to agent\|section-resolved\|section-detached" web/e2e`)

**Interfaces:**
- `view/history-model.ts` (no `svelte` import):
  - `type HistoryEvent = { v: number | null; who: string; agent: boolean; verb: string }`;
  - `versionAt(versions: Version[], iso: string): number`: the version current at `iso`, the newest with `created_at <= iso`, else 1;
  - `historyOf(t: Thread, versions: Version[], names: (by: string) => string): HistoryEvent[]`;
  - `isOutdated(t: Thread, r: AnchorResult | undefined, shown: number): boolean`;
  - `agentName(harness: string | null | undefined): string`, which answers the harness (`claude`, `codex`, `pi`), or `agent`.
- `sidebar-model.ts`: `authorLabel(c)` answers the agent's name (`agentName(c.via_harness)`) for agent comments, else the author's name.
- `ThreadCard` gains the props `history: HistoryEvent[]`, `outdated: boolean`, `agent: string` and `when: string`.
- `Sidebar` gains `versions: Version[]`, `shown: number` and `agent: string`, and passes them on.

- [ ] **Step 1: The history model, test first**

`web/shell/src/view/history-model.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import type { Version } from "../api";
import type { Comment, Thread } from "../threads";
import { historyOf, isOutdated, versionAt } from "./history-model";

const V = (n: number, at: string): Version => ({ artifact_id: "a", n, label: null, created_at: at, files: {} });
const vs = [V(1, "2026-09-30T10:00:00.000Z"), V(2, "2026-09-30T11:00:00.000Z"), V(3, "2026-09-30T12:00:00.000Z")];
const C = (id: string, kind: "viewer" | "agent", name: string, at: string): Comment =>
  ({ id, thread_id: "t", author_kind: kind, author_name: name, via_harness: kind === "agent" ? "claude" : null, body: "x", created_at: at });
const T = (over: Partial<Thread>): Thread => ({
  id: "t", artifact_id: "a", version_n: 1, status: "open", sent_to_agent: true, has_clip: false, clip_url: null, created_at: "2026-09-30T10:30:00.000Z",
  resolved_at: null, resolved_by: null, feedback_state: null, comments: [],
  anchor: { kind: "element", selector: "h2", quote: null, prefix: null, suffix: null, html_hash: "h1", rect: null, custom_name: null, file: "index.html" }, ...over,
});
const names = (by: string) => (by === "viewer:u_1" ? "alex" : by.startsWith("agent:") ? by.slice(6) : "someone");

describe("history-model", () => {
  it("tags an event with the version current when it happened", () => {
    expect(versionAt(vs, "2026-09-30T09:00:00.000Z")).toBe(1);
    expect(versionAt(vs, "2026-09-30T11:30:00.000Z")).toBe(2);
    expect(versionAt(vs, "2026-09-30T13:00:00.000Z")).toBe(3);
  });

  it("reads comments, replies, an agent's reply without a tag, and the resolve", () => {
    const t = T({
      comments: [C("1", "viewer", "alex", "2026-09-30T10:30:00.000Z"), C("2", "viewer", "Mia", "2026-09-30T11:10:00.000Z"), C("3", "agent", "Agent", "2026-09-30T11:20:00.000Z")],
      status: "resolved", resolved_by: "viewer:u_1", resolved_at: "2026-09-30T12:10:00.000Z",
    });
    expect(historyOf(t, vs, names)).toEqual([
      { v: 1, who: "alex", agent: false, verb: "commented" },
      { v: 2, who: "Mia", agent: false, verb: "replied" },
      { v: null, who: "claude", agent: true, verb: "replied" },
      { v: 3, who: "alex", agent: false, verb: "resolved" },
    ]);
  });

  it("calls a thread outdated when a later version changed its element but still has it", () => {
    const found = (method: "exact" | "selector" | "quote" | "custom") => ({ id: "t", found: true, method, rect: null });
    expect(isOutdated(T({}), found("selector"), 2)).toBe(true);
    expect(isOutdated(T({}), found("quote"), 2)).toBe(true);
    expect(isOutdated(T({}), found("exact"), 2)).toBe(false);
    expect(isOutdated(T({}), found("selector"), 1)).toBe(false);
    expect(isOutdated(T({}), { id: "t", found: false, method: null, rect: null }, 2)).toBe(false);
    expect(isOutdated(T({ anchor: { ...T({}).anchor, html_hash: null } }), found("selector"), 2)).toBe(false);
    expect(isOutdated(T({}), found("custom"), 2)).toBe(false);
  });
});
```

Run: `cd web && npx vitest run shell/src/view/history-model.test.ts`
Expected: FAIL (module not found).

`web/shell/src/view/history-model.ts`:

```ts
// A thread's history as version-tagged events (spec §8, "Thread sidebar"):
// "v3 alex commented · v4 Mia replied · claude replied · v5 alex resolved".
// A person's event carries the version current when it happened; an agent's
// carries one only when it came with a version (Task 18 adds those).
import type { AnchorResult } from "../../../bridge/src/protocol";
import type { Version } from "../api";
import type { Thread } from "../threads";

export type HistoryEvent = { v: number | null; who: string; agent: boolean; verb: string };

export const agentName = (h: string | null | undefined): string => h || "agent";

export function versionAt(versions: Version[], iso: string): number {
  let n = 1;
  for (const v of versions) if (v.created_at <= iso && v.n > n) n = v.n;
  return n;
}

/** `names` turns a `resolved_by` value (`viewer:<public_id>`, `agent:<harness>`) into a name. */
export function historyOf(t: Thread, versions: Version[], names: (by: string) => string): HistoryEvent[] {
  const out: HistoryEvent[] = [];
  t.comments.forEach((c, i) => {
    if (c.author_kind === "agent") out.push({ v: null, who: agentName(c.via_harness), agent: true, verb: "replied" });
    else out.push({ v: versionAt(versions, c.created_at), who: c.author_name, agent: false, verb: i === 0 ? "commented" : "replied" });
  });
  if (t.status === "resolved" && t.resolved_by && t.resolved_at) {
    const agent = t.resolved_by.startsWith("agent:");
    out.push({ v: agent ? null : versionAt(versions, t.resolved_at), who: names(t.resolved_by), agent, verb: "resolved" });
  }
  return out;
}

/** The element changed in a later version but is still there: found by
 * selector or quote while the stored hash no longer matches (spec §8). A
 * page-anchored (custom) thread is never outdated. */
export function isOutdated(t: Thread, r: AnchorResult | undefined, shown: number): boolean {
  return !!r?.found && shown > t.version_n && !!t.anchor.html_hash && (r.method === "selector" || r.method === "quote");
}
```

Run: `cd web && npx vitest run shell/src/view/history-model.test.ts`
Expected: PASS.

- [ ] **Step 2: The card, the sidebar and the pins**

`ui/ThreadCard.svelte`: add to `Props` `history: HistoryEvent[]`, `outdated: boolean`, `agent: string` and `when: string`. Replace the markup from `<header>` to the end of the actions with:

```svelte
  <header>
    <button type="button" class="card-head" aria-pressed={selected === t.id} onclick={e => { e.stopPropagation(); onSelect(t); }}
      >{#if n !== undefined}<span class="thread-num">{n}</span>{/if}<span class="anchor-label">{anchorLabel(t.anchor)}</span
      >{#if outdated}<span class="vt out">outdated</span>{/if}{#if t.anchor.file !== file}<span class="file-label muted small">on {t.anchor.file}</span>{/if}<span class="muted small">{when}</span
    ></button>
  </header>
  {#if t.clip_url}<img class="thumb" src={t.clip_url} alt="Screenshot of the commented region" loading="lazy" />{/if}
  {#each t.comments as c (c.id)}
    <div class={["msg", c.author_kind === "agent" ? "agent" : "you"]}>
      <b class="author">{authorLabel(c)}{#if c.via_page}<span class="via-page muted small">{" · via the page"}</span>{/if}</b>
      <p class="body">{c.body}</p>
    </div>
  {/each}
  {#if label}<p class="st waiting">{label}</p>{/if}
  {#if history.length}
    <p class="hist" aria-label="History">
      {#each history as e, i (i)}<span class={["ev", e.agent && "agent"]}>{#if e.v !== null}<span class="vt">v{e.v}</span>{/if}<b>{e.who}</b> {e.verb}</span>{/each}
    </p>
  {/if}
  {#if t.status === "open"}
    <!-- Only stops a click on these controls from also selecting the card. -->
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <div class="actions" onclick={e => e.stopPropagation()}>
      <button onclick={() => onResolve(t)}>Resolve</button>
      {#if !t.sent_to_agent}<button class="primary" onclick={() => onSend(t)}>Send to {agent}</button>{/if}
    </div>
  {/if}
```

The `Resolved by …` paragraph goes, because the history line now records the resolve. The reply form stays, with the placeholder `Reply, or @name someone`.

`ui/Sidebar.svelte`:
- Add `versions: Version[]`, `shown: number` and `agent: string` to `Props`.
- Compute `const names = (by: string) => by.startsWith("agent:") ? agentName(by.slice(6)) : resolvedByLabel(by, p.me);`.
- Pass these to every `ThreadCard`: `history={historyOf(t, p.versions, names)}`, `outdated={isOutdated(t, p.resolved[t.id], p.shown)}`, `agent={p.agent}` and `when={relativeTime(t.created_at, clock.now)}`.
- Replace the `section` snippet and its three uses with:

```svelte
{#snippet cards(list: Thread[])}
  {#each list as t (t.id)}
    <ThreadCard {t} n={s.numbers.get(t.id)} now={clock.now} me={p.me} selected={p.selected} file={s.file} focusReply={p.focusReply}
      history={historyOf(t, p.versions, names)} outdated={isOutdated(t, p.resolved[t.id], p.shown)} agent={p.agent} when={relativeTime(t.created_at, clock.now)}
      onSelect={p.onSelect} onSend={p.onSend} onResolve={p.onResolve} onReply={p.onReply} onHover={p.onHover} />
  {/each}
{/snippet}

<aside class="sidebar" aria-label="Comment threads">
  {@render p.header?.()}
  <section class="section-open">
    <h2 class="gh you"><span class="sw" aria-hidden="true"></span><span class="t">Open</span> <span class="c">{s.open.length}</span></h2>
    {#if s.open.length === 0}<p class="muted small empty-open">Nothing open. Press C and click anything to comment on it.</p>{:else}{@render cards(s.open)}{/if}
  </section>
  <div class="tail">
    <details class="section-detached">
      <summary class="gh oth"><span class="sw" aria-hidden="true"></span><span class="t">Detached</span> <span class="c">{s.detached.length}</span></summary>
      {@render cards(s.detached)}
    </details>
    <details class="section-resolved">
      <summary class="gh set"><span class="sw" aria-hidden="true"></span><span class="t">Resolved</span> <span class="c">{s.resolved.length}</span></summary>
      {@render cards(s.resolved)}
    </details>
  </div>
</aside>
```

`ui/SidebarIsland.svelte` passes `versions={s.data.versions}`, `shown={ctl.shown(s)}` and `agent={agentName(s.data.artifact.owner_harness)}`.

`view/sidebar-model.ts` `authorLabel`: `return c.author_kind === "agent" ? agentName(c.via_harness) : c.author_name;`

`ui/Pins.svelte`: no markup change. The Echo pin styles below apply. Tasks 16 and 18 add state classes.

In `sidebar.test.ts`:
- every `mount(Sidebar, …)` gains `versions: [], shown: 1, agent: "claude"`;
- assertions of `Agent · via claude` become `claude`;
- `Send to agent` becomes `Send to claude`;
- `Resolved by …` assertions now read the history line (`.hist`).

Add a test: a thread with a second viewer comment shows `.hist` reading `v1alex commented` then `v1Mia replied`, and an open thread on v1, shown at v2 with a `selector` result and an `html_hash`, has `.vt.out` with the text `outdated`.

- [ ] **Step 3: Styles**

In `web/shell/src/theme.css`, replace the rules for `.sidebar`, `.sidebar h2`, `.thread-card` and everything under it, `.comment` and `.comment.*`, `.waiting`, `.thread-num, .thread-pin`, `.thread-pin` and `.thread-pin:not(:disabled):hover` with:

```css
/* The sidebar (spec §8, "Thread sidebar"). */
.sidebar { width: 392px; flex: none; overflow: auto; border-left: 1px solid var(--border); background: var(--bg); padding: 12px 14px 18px; display: flex; flex-direction: column; gap: 10px; }
.gh { display: flex; align-items: center; gap: 8px; margin: 6px 2px 8px; font: 600 19px/1.1 var(--grot); list-style: none; cursor: default; }
summary.gh { cursor: pointer; font-size: 16px; color: var(--muted); }
summary.gh::-webkit-details-marker { display: none; }
.gh .t { flex: 1; } .gh .c { font: 400 12px var(--mono); color: var(--muted); }
.gh .sw { width: 7px; height: 16px; flex: none; }
.gh.you .sw { border-radius: 0 8px 8px 0; background: var(--you); }
.gh.ag .sw { border-radius: 8px 0 0 8px; background: var(--agent); } .gh.ag .t { color: var(--agent-ink); }
.gh.oth .sw { border-radius: 0 8px 8px 0; box-shadow: inset 0 0 0 1.5px var(--you); }
.gh.set .sw { border-radius: 50%; width: 10px; height: 10px; background: var(--border-strong); }
.tail { margin-top: 6px; border-top: 1px solid var(--border); padding-top: 8px; }
.thread-card { background: var(--card); border: 1px solid var(--border); padding: 11px 12px 10px; margin-bottom: 10px; cursor: pointer; transition: background-color var(--t); }
.thread-card:hover { border-color: var(--border-strong); }
.thread-card.selected { box-shadow: inset 3px 0 0 var(--accent); background: var(--comment-hl); }
.thread-card header { margin-bottom: 8px; min-width: 0; }
.thread-card .card-head { display: flex; gap: 8px; align-items: center; width: 100%; min-width: 0; min-height: 0; background: none; border: 0; padding: 0; color: var(--muted); text-align: left; font: 400 12px var(--mono); }
.thread-card .anchor-label { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--fg); }
.thread-num, .thread-pin { display: inline-grid; place-items: center; width: 22px; height: 22px; min-height: 0; border-radius: 50%; background: var(--you); color: var(--on-you); font: 600 12px/1 var(--mono); flex: none; padding: 0; }
.vt { font: 600 11px/15px var(--grot); padding: 0 4px; border: 1px solid var(--border-strong); color: var(--fg); background: var(--bg); white-space: nowrap; }
.vt.out { color: var(--muted); }
.msg { display: grid; gap: 3px; }
.msg + .msg { margin-top: 10px; }
.msg .author { font: 600 14px/20px var(--grot); }
.msg .body { margin: 0; font-size: 13.5px; line-height: 1.55; white-space: pre-wrap; overflow-wrap: anywhere; }
.msg.you { border-left: 3px solid var(--you); padding: 1px 0 1px 10px; }
.msg.agent { border-right: 3px solid var(--agent); padding: 1px 10px 1px 0; margin-left: 26px; text-align: right; }
.msg.agent .author { color: var(--agent-ink); }
.msg.agent .body { text-align: left; }
.st { display: flex; align-items: center; gap: 8px; margin: 10px 0 0; padding-top: 8px; border-top: 1px dashed var(--border); font-size: 12px; color: var(--muted); }
.hist { display: flex; flex-wrap: wrap; gap: 4px 10px; margin: 9px 0 0; padding-top: 8px; border-top: 1px dashed var(--border); font-size: 11.5px; color: var(--muted); line-height: 1.6; }
.hist .ev { display: inline-flex; align-items: center; gap: 4px; white-space: nowrap; }
.hist .ev b { font-weight: 600; color: var(--fg); }
.hist .ev.agent .vt { border-color: var(--agent); color: var(--agent-ink); }
.thread-pin { position: absolute; pointer-events: auto; border: 2px solid var(--pin-ring); box-shadow: 0 1px 3px rgba(47,11,4,.45); }
.thread-pin:not(:disabled):hover { border-color: var(--focus); }
@media (max-width: 700px) { .sidebar { position: absolute; inset: 0 0 52px; width: auto; z-index: 10; border-left: 0; } }
```

Delete the port's older `@media (max-width: 700px) { .sidebar { … } }` line, which the last line above replaces.

- [ ] **Step 4: Tests, e2e locators, screenshots**

Update every test that pins the old card. Each is listed with its replacement:
- `web/shell/src/sidebar.test.ts:25`: `.comment.agent .author` reading `Agent · via claude` becomes `.msg.agent .author` reading `claude`; the `Send to agent` filter (about line 28) becomes `Send to claude`.
- `web/shell/src/artifact.test.ts:846` and `:1009`: `buttonNamed(…, "Send to agent")` becomes `buttonNamed(…, "Send to claude")`, or the test fixture's owner harness when it is not `claude`.
- `web/e2e/comments.spec.ts:44` and `web/e2e/comment-loop.spec.ts:55`: `toContainText("Agent · via claude")` becomes `toContainText("claude")` on `.msg.agent .author`.
- `web/e2e/comments.spec.ts:48` (`Resolved by Viewer`) and `:116`: assert the history instead, `card.locator(".hist")` containing `Viewer resolved` (the name the viewer has there).
- `web/e2e/comment-loop.spec.ts:60` (`Resolved by Agent · via claude`): `done.locator(".hist")` containing `claude resolved`.
- `web/e2e/comments.spec.ts:188-192`, the Tab-order test: Resolve now comes before Send. The order becomes header, `// Resolve`, `// Send to the agent`, `// Reply input`, `// Reply button`, next card. The comment no longer says `Send to agent`, so Task 25's gate stays quiet.
- Any other locator naming `Send to agent` becomes `/^Send to /`: `grep -rn "Send to agent" web/shell/src web/e2e` prints nothing afterwards.
- A step that clicks a card under Resolved or Detached first clicks that group's `summary`.

Count assertions (`toHaveCount`) need no change, because closed `<details>` content stays in the DOM.

Run: `cd web && npx vitest run && npm run lint && npm run typecheck && npm run build && node scripts/bundle-size.mjs && npx playwright test; echo "exit=$?"`
Expected: `exit=0`.

Append to `SCENES` in `web/e2e/scenes.ts`, with the panel open and the first thread selected:

```ts
  { name: "threads", path: s => `/a/${s.aid}`, prepare: async page => {
    if (!(await page.locator("aside.sidebar").isVisible())) await page.getByRole("button", { name: /Threads/ }).first().click();
    await page.locator(".thread-card").first().click();
  } },
```

Run: `cd web && CLAX_SHOTS=task-05 npx playwright test e2e/shots.spec.ts`
Expected: PASS.

Report what the `threads` shots show against `concept-3-echo/shots/*-view.png`:
- the red-orange rule on people's messages;
- the history line with its `v1` tags;
- `Send to claude`;
- the group head with its swatch;
- Detached and Resolved collapsed into the tail;
- the empty Open message, when it shows;
- at phone width, the sidebar above the tabs.


Run: `cd web && npm run perf; echo "exit=$?"`
Expected: `exit=0`. Stop rule: if any of the five measures in `web/perf/budget.json` is over budget (link to first paint `firstPaint`, link to comment ready `commentReady`, frame paint `framePaint`, ready latency `readyLatency`, cold ready latency `coldLatency`, in either frame mode), stop and report all five numbers against their budgets. Lazy-load first; never raise a budget.

- [ ] **Step 5: Gates and staging**

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add web/shell/src/view/history-model.ts web/shell/src/view/history-model.test.ts web/shell/src/ui/ThreadCard.svelte web/shell/src/ui/Sidebar.svelte \
  web/shell/src/ui/SidebarIsland.svelte web/shell/src/ui/Pins.svelte web/shell/src/view/sidebar-model.ts web/shell/src/sidebar.test.ts web/shell/src/theme.css web/e2e/scenes.ts
git add -u web/e2e web/shell/src
git status --short   # staged; the controller commits ("Show threads in Echo: mirrored messages, a version-tagged history line, and an outdated tag")
```

---

### Task 6: The gallery in Echo: numerals, pinned first, the haiku footer, the empty state, rally of 10

The gallery in Echo:
- a 60px bar with the playful mark, `Clax`, `local artifacts · seen as <name>`, search, and the switch;
- cards that lead with the version numeral, pinned first and then the most recent;
- a footer haiku, loaded after first paint;
- the empty gallery's mark with its halves apart;
- the "rally of 10" chip.

Grouping by "needs your eyes" and the markers wait for attention (Task 19). The roster waits for participants (Task 16).

**Files:**
- Create: `web/shell/src/view/haiku.json`, `web/shell/src/view/haiku.ts`, `web/shell/src/view/haiku.test.ts`, `web/shell/src/ui/HaikuLine.svelte`, `web/shell/src/ui/GalleryCard.svelte`
- Modify: `web/shell/src/ui/Gallery.svelte`, `web/shell/src/view/gallery-model.ts`, `web/shell/src/view/gallery-model.test.ts`, `web/shell/src/gallery.test.ts`, `web/shell/src/theme.css`, `crates/clax-cli/src/commands/haiku.rs`, `web/e2e/shots.spec.ts` (the empty-gallery run)

**Interfaces:**
- `view/haiku.ts`: `pickHaiku(list: string[], seed?: string): string`. Without a seed the pick is random. With one, it is stable: an FNV-1a hash of the seed, modulo the length.
- `view/gallery-model.ts`: `orderArtifacts(list: Artifact[]): Artifact[]` puts pinned first, then sorts by `updated_at`, newest first. `rally(a: Artifact): boolean` is true when `a.current_version === 10` (decided: Q9).
- `GalleryCard.svelte`: `{ a: Artifact; token: string | null; onPin(): void; onDelete(): void; markers?: Snippet; footer?: Snippet }`.

- [ ] **Step 1: Haiku, with a parity test across Rust and the shell**

First replace the ninth haiku in `crates/clax-cli/src/commands/haiku.rs` `HAIKU`, which speaks of a turn. Change `"Stop hook, end of turn\none more comment slipped in late\nthe work carries on"` to `"Stop hook at the end\none more comment slipped in late\nthe work carries on"` (still five, seven, five).

`web/shell/src/view/haiku.json`: the ten haiku from `HAIKU`, in order, as a JSON array of strings, each with its `\n` line breaks.

Append to the tests in `crates/clax-cli/src/commands/haiku.rs`:

```rust
    #[test]
    fn the_shell_shows_the_same_haiku() {
        let shell: Vec<String> = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../web/shell/src/view/haiku.json"
        )))
        .expect("haiku.json is a JSON array of strings");
        assert_eq!(shell, HAIKU.to_vec(), "web/shell/src/view/haiku.json must match HAIKU");
    }
```

`web/shell/src/view/haiku.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import list from "./haiku.json";
import { pickHaiku } from "./haiku";

describe("haiku", () => {
  it("are ten, three lines each, and a seed picks the same one every time", () => {
    expect(list).toHaveLength(10);
    for (const h of list) expect(h.split("\n")).toHaveLength(3);
    expect(pickHaiku(list, "01J9ABC")).toBe(pickHaiku(list, "01J9ABC"));
    expect(list).toContain(pickHaiku(list));
  });
});
```

`web/shell/src/view/haiku.ts`:

```ts
// Clax's haiku in the shell (spec §8, "Look"): the gallery footer picks one
// at random each visit; the working line picks by the record's key, so it
// holds still while the record lives. The list is haiku.json, loaded lazily.
export function pickHaiku(list: string[], seed?: string): string {
  if (seed === undefined) return list[Math.floor(Math.random() * list.length)];
  let h = 0x811c9dc5;
  for (let i = 0; i < seed.length; i++) { h ^= seed.charCodeAt(i); h = Math.imul(h, 0x01000193) >>> 0; }
  return list[h % list.length];
}
```

`web/shell/src/ui/HaikuLine.svelte`:

```svelte
<script lang="ts">
  // One haiku, loaded after first paint (the list is its own chunk). Never in
  // comment mode, never animated: the callers decide where it shows.
  import { pickHaiku } from "../view/haiku";

  let { seed, footer = false }: { seed?: string; footer?: boolean } = $props();
  let text = $state<string | null>(null);
  $effect(() => { void import("../view/haiku.json").then(m => { text = pickHaiku(m.default, seed); }); });
</script>

{#if text}
  {#if footer}
    <footer class="gfoot"><pre>{text}</pre><span>clax haiku · a new one each visit</span></footer>
  {:else}
    <span class="hk">{text.replaceAll("\n", " / ")}</span>
  {/if}
{/if}
```

If `npm run typecheck` rejects the JSON import, add `"resolveJsonModule": true` to `compilerOptions` in `web/tsconfig.json`, and stage that file too.

Run: `cargo test -p clax-cli haiku && cd web && npx vitest run shell/src/view/haiku.test.ts`
Expected: PASS.

- [ ] **Step 2: Order and the card, tests first**

Add to `web/shell/src/view/gallery-model.test.ts`:

```ts
  it("puts pinned first, then the most recent", () => {
    const A = (id: string, pinned: boolean, at: string) => ({ id, title: id, description: null, icon: null, pinned, current_version: 1, updated_at: at });
    expect(orderArtifacts([A("old", false, "2026-09-01"), A("pin", true, "2026-08-01"), A("new", false, "2026-09-30")]).map(a => a.id)).toEqual(["pin", "new", "old"]);
  });
  it("marks the tenth version, and only it", () => {
    expect([9, 10, 11, 20].map(n => rally({ current_version: n } as never))).toEqual([false, true, false, false]);
  });
```

In `gallery.test.ts`:
- the test that reads card text expects `.card .v` to read `v3`, and the card to be no longer `p` description;
- the two `.publisher` assertions become `.card .by` containing `claude-code`;
- a new test stubs `/api/artifacts` with `[]` and expects `.empty-gallery .mark.apart` and the text `When an agent publishes a page, it lands here.`;
- a new test expects the pinned artifact's card first even when listed second.

Run: `cd web && npx vitest run shell/src/view/gallery-model.test.ts shell/src/gallery.test.ts`
Expected: FAIL.

`view/gallery-model.ts`:

```ts
/** Pinned first, then the most recently updated. */
export function orderArtifacts(list: Artifact[]): Artifact[] {
  return [...list].sort((a, b) => Number(b.pinned) - Number(a.pinned) || b.updated_at.localeCompare(a.updated_at));
}

/** The "rally of 10" easter egg: the artifact is at its tenth version. */
export const rally = (a: Artifact): boolean => a.current_version === 10;
```

`web/shell/src/ui/GalleryCard.svelte`:

```svelte
<script lang="ts">
  import type { Snippet } from "svelte";
  import type { Artifact } from "../api";
  import { relativeTime } from "../format";
  import { agentName } from "../view/history-model";
  import { rally } from "../view/gallery-model";

  let { a, token, onPin, onDelete, markers, footer }: { a: Artifact; token: string | null; onPin(): void; onDelete(): void; markers?: Snippet; footer?: Snippet } = $props();
  const by = $derived(a.owner_harness ? agentName(a.owner_harness) : "command line");
</script>

<div class="card-wrap">
  <a class="card" href={`/a/${a.id}`} title={a.description ?? undefined}>
    <span class="cb2">
      <span class="v g">v{a.current_version}</span>
      <h3>{a.title}{#if a.pinned}<span class="pin" title="Pinned"> ★</span>{/if}</h3>
      <span class="by">{#if a.owner_live}<span class="live-dot" role="img" aria-label="session is live" title="Session is live"></span>{/if}{by} · {relativeTime(a.updated_at)}</span>
    </span>
    {#if markers || rally(a)}
      <span class="mks">{@render markers?.()}{#if rally(a)}<span class="chip rally">rally of 10</span>{/if}</span>
    {/if}
    <span class="ft">{@render footer?.()}</span>
  </a>
  {#if token}
    <div class="card-tools">
      <button type="button" class="ghost" title={a.pinned ? "Unpin" : "Pin"} aria-label={a.pinned ? `Unpin ${a.title}` : `Pin ${a.title}`} onclick={onPin}>{a.pinned ? "★" : "☆"}</button>
      <button type="button" class="ghost" title="Delete" onclick={onDelete}>Delete</button>
    </div>
  {/if}
</div>
```

`ui/Gallery.svelte`:
- `shown` becomes `artifacts && orderArtifacts(filterArtifacts(artifacts, query))`.
- Add `let me = $state<string | null>(null);`, and in `onMount`, after `refresh`, add `void getViewer().then(v => { me = v.display_name; }, () => {});` with `getViewer` imported from `../threads`.
- Replace the header and main with:

```svelte
<header class="gbar">
  <Mark playful /><h1>Clax</h1><span class="sub hide-sm">local artifacts{#if me} · seen as {me}{/if}</span>
  <input type="search" class="search" placeholder="Search artifacts" aria-label="Search artifacts" bind:value={query} />
  <ThemeSwitch />
</header>
<main class="gal">
  {#if error}<p class="empty">Could not load artifacts: {error}</p>{/if}
  {#if artifacts && artifacts.length === 0}
    <div class="empty-gallery"><Mark size="hero" apart /><p>When an agent publishes a page, it lands here.</p><p class="muted">Or publish one yourself with <code>clax publish index.html</code>.</p></div>
  {/if}
  {#if artifacts && artifacts.length > 0 && shown && shown.length === 0}<p class="empty">No artifacts match your search.</p>{/if}
  {#if shown && shown.length > 0}
    <section class="grp rest">
      <div class="cards">
        {#each shown as a (a.id)}
          <GalleryCard {a} {token} onPin={() => token && act(() => patchArtifact(a.id, { pinned: !a.pinned }, token!))}
            onDelete={() => { if (token && confirm(`Delete "${a.title}"? This removes every version.`)) void act(() => deleteArtifact(a.id, token!)); }} />
        {/each}
      </div>
    </section>
  {/if}
  {#if artifacts}
    {#await import("./HaikuLine.svelte") then { default: HaikuLine }}<HaikuLine footer />{/await}
  {/if}
</main>
```

- [ ] **Step 3: Styles**

In `web/shell/src/theme.css`, replace `.wrap`, `.grid`, `.card` and its children, `.empty` and `.empty code`, `.search`, `.card-wrap`, `.card-tools` and its children, and Task 4's two `.gbar` rules, with:

```css
/* The gallery (spec §8, "Gallery"). */
.gbar { height: 60px; display: flex; align-items: center; gap: 12px; padding: 0 var(--gutter); background: var(--card); border-bottom: 1px solid var(--border); }
.gbar h1 { margin: 0; font-size: 22px; }
.gbar .sub { font-size: 11.5px; color: var(--muted); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; min-width: 0; }
.search { margin-left: auto; width: 260px; min-width: 0; max-width: 50%; }
.gal { max-width: 1360px; margin: 0 auto; padding: 28px 32px 40px; }
.grp { margin-bottom: 34px; min-width: 0; }
.grp h2 { display: flex; align-items: center; gap: 10px; margin: 0 0 4px; font-size: 26px; line-height: 1.1; padding-bottom: 10px; border-bottom: 2px solid var(--fg); }
.grp h2 small { font: 400 12px var(--mono); color: var(--muted); margin-left: auto; text-align: right; }
.grp h2 .sw { width: 12px; height: 24px; flex: none; }
.grp .rule { font-size: 11.5px; color: var(--muted); margin: 6px 0 14px; line-height: 1.45; }
.cards { display: grid; grid-template-columns: repeat(auto-fill, minmax(230px, 1fr)); gap: 18px; }
.card-wrap { position: relative; }
.card { background: var(--card); border: 1px solid var(--border); display: flex; flex-direction: column; height: 100%; transition: border-color var(--t); }
.card:hover { border-color: var(--border-strong); }
.card .cb2 { display: grid; grid-template-columns: auto 1fr; gap: 2px 12px; padding: 11px 14px 10px; align-items: baseline; }
.card .v { font-size: 40px; line-height: .9; grid-row: span 2; }
.card h3 { margin: 0; font-size: 17px; line-height: 1.15; overflow-wrap: anywhere; padding-right: 64px; }
.card .by { font-size: 11.5px; color: var(--muted); }
.card .live-dot { display: inline-block; width: 7px; height: 7px; border-radius: 50%; background: var(--agent); margin-right: 5px; vertical-align: 1px; }
.card .mks { display: flex; flex-wrap: wrap; gap: 6px; padding: 0 12px 10px; }
.card .ft { display: flex; align-items: center; gap: 8px; border-top: 1px solid var(--border); padding: 8px 12px; margin-top: auto; min-height: 37px; flex-wrap: wrap; }
.card .ft:empty { border-top-color: transparent; }
.chip { display: inline-flex; align-items: center; gap: 6px; font: 600 13px/1 var(--grot); padding: 4px 8px; white-space: nowrap; }
.chip.rally { font: 400 11px var(--mono); color: var(--muted); padding: 0; }
.card-tools { position: absolute; right: 6px; top: 8px; display: flex; gap: 2px; }
.card-tools button { min-height: 28px; padding: 2px 8px; color: var(--muted); }
.card-tools button:last-child:hover, .card-tools button:last-child:focus-visible { color: var(--danger); }
.empty { text-align: center; color: var(--muted); padding: 64px 0; }
.empty-gallery { display: grid; place-items: center; gap: 10px; padding: 80px 0 40px; text-align: center; }
.empty-gallery p { margin: 0; font: 600 20px var(--grot); }
.empty-gallery p.muted { font: 400 13px var(--mono); }
code { background: var(--bg); border: 1px solid var(--border); padding: 2px 6px; color: var(--fg); }
.gfoot { margin-top: 4px; color: var(--muted); font-size: 12px; display: flex; gap: 24px; border-top: 1px solid var(--border); padding-top: 14px; }
.gfoot pre { margin: 0; font: inherit; line-height: 1.7; color: var(--fg); }
.hk { color: var(--muted); font-size: 11.5px; line-height: 1.5; }
@media (max-width: 700px) {
  .gbar .hide-sm { display: none; }
  .gal { padding: 18px 16px 32px; }
  .grp h2 { font-size: 22px; }
  .cards { grid-template-columns: 1fr; gap: 12px; }
  .card .v { font-size: 30px; } .card h3 { font-size: 15px; }
  .gfoot { flex-direction: column; gap: 8px; }
}
```

Search keeps working over title and description. Pin and Delete keep their labels.

- [ ] **Step 4: Run, budget, screenshots**

Run: `cd web && npx vitest run && npm run lint && npm run typecheck && npm run build && node scripts/bundle-size.mjs; echo "exit=$?"`
Expected: `exit=0`. `haiku.json` and `HaikuLine` are outside `manifest["index.html"]`'s closure: `grep -l "seventy-four eighty" dist/_clax/shell/*.js` names a chunk the entry does not import statically.

Add a scene for the empty gallery. Seeding happens once per run, so it is a separate run against a fresh daemon. Add `test.describe` in `shots.spec.ts` for `CLAX_SHOTS_EMPTY=1`, which skips `seed` and photographs `/` as `gallery-empty`. Run both:

```bash
cd web && CLAX_SHOTS=task-06 npx playwright test e2e/shots.spec.ts && CLAX_SHOTS=task-06 CLAX_SHOTS_EMPTY=1 npx playwright test e2e/shots.spec.ts
```

Expected: PASS.

Report, against `concept-3-echo/shots/*-gallery.png`:
- the numerals lead each card;
- pinned is first;
- the footer haiku is three lines in mono;
- the empty state shows the mark large with its halves apart;
- the bar holds the mark, the sub-line, search and the switch;
- the phone layout is one column with no sideways scroll.

In a headed browser, click the gallery's mark and see the halves meet, then part.


Run: `cd web && npm run perf; echo "exit=$?"`
Expected: `exit=0`. Stop rule: if any of the five measures in `web/perf/budget.json` is over budget (link to first paint `firstPaint`, link to comment ready `commentReady`, frame paint `framePaint`, ready latency `readyLatency`, cold ready latency `coldLatency`, in either frame mode), stop and report all five numbers against their budgets. Lazy-load first; never raise a budget.

- [ ] **Step 5: Gates and staging**

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add web/shell/src/view/haiku.json web/shell/src/view/haiku.ts web/shell/src/view/haiku.test.ts web/shell/src/ui/HaikuLine.svelte web/shell/src/ui/GalleryCard.svelte \
  web/shell/src/ui/Gallery.svelte web/shell/src/view/gallery-model.ts web/shell/src/view/gallery-model.test.ts web/shell/src/gallery.test.ts web/shell/src/theme.css \
  crates/clax-cli/src/commands/haiku.rs web/e2e/shots.spec.ts
git status --short   # staged; the controller commits ("Show the gallery in Echo: version numerals, pinned first, a haiku footer, and the mark apart when empty")
```

---

### Task 7: The working registry in clax-core

**Files:**
- Create: `crates/clax-core/src/working.rs`
- Modify: `crates/clax-core/src/lib.rs`, `crates/clax-core/src/events.rs`

**Interfaces:**
- Produces: `clax_core::working::{WORKING_TTL_SECS: i64 = 120, MAX_MESSAGE_CHARS: usize = 140, MAX_WORKING_THREADS: usize = 20, Clock, SystemClock, ManualClock, Actor, WorkingView, SessionWorking, SetWorking, Changed, Working, clean_message}`.
- `Working` methods: `new(clock: Arc<dyn Clock>) -> Working`, `now() -> DateTime<Utc>`, `skew(secs: i64)`, `set(&Actor, aid, SetWorking) -> (SessionWorking, Changed)`, `mark(&Actor, aid, &[String]) -> Changed`, `renew(sid) -> usize`, `clear(sid, aid, Option<&[String]>) -> Changed`, `thread_done(sid, aid, tid) -> Changed`, `thread_gone(aid, tid) -> Changed`, `end_session(sid) -> Changed`, `artifact_gone(aid) -> Changed`, `sweep() -> Changed`, `for_artifact(aid) -> Vec<WorkingView>`, `for_session(sid) -> Vec<SessionWorking>`, `threads_of(sid, aid) -> Vec<String>`, `all() -> BTreeMap<String, Vec<WorkingView>>`.
- Produces: `Event::Working { artifact_id: String, working: Vec<WorkingView> }`, SSE name `working`.

- [ ] **Step 1: Write the failing tests**

Create `crates/clax-core/src/working.rs` with only this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (Arc<ManualClock>, Working) {
        let clock = Arc::new(ManualClock::at("2026-09-30T10:00:00Z"));
        (clock.clone(), Working::new(clock))
    }
    fn claude(sid: &str) -> Actor {
        Actor { session_id: sid.into(), harness: "claude".into() }
    }
    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn mark_creates_then_adds_threads_and_keeps_the_key() {
        let (_c, w) = fixture();
        assert_eq!(w.mark(&claude("s1"), "a1", &ids(&["t1"])).0, ["a1".to_string()].into());
        let key = w.for_artifact("a1")[0].key.clone();
        w.mark(&claude("s1"), "a1", &ids(&["t2", "t1"]));
        let v = w.for_artifact("a1");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].thread_ids, ids(&["t1", "t2"]));
        assert_eq!(v[0].key, key);
        assert_eq!(v[0].harness, "claude");
        assert_eq!(v[0].message, None);
    }

    #[test]
    fn set_replaces_given_fields_and_keeps_started_at() {
        let (c, w) = fixture();
        w.mark(&claude("s1"), "a1", &ids(&["t1"]));
        let started = w.for_artifact("a1")[0].started_at.clone();
        c.advance(30);
        let (s, changed) = w.set(&claude("s1"), "a1", SetWorking { thread_ids: None, message: Some("Tightening spacing".into()) });
        assert!(!changed.is_empty());
        assert_eq!(s.view.message.as_deref(), Some("Tightening spacing"));
        assert_eq!(s.view.thread_ids, ids(&["t1"]));
        assert_eq!(s.view.started_at, started);
        assert_eq!(s.view.last_heartbeat, "2026-09-30T10:00:30.000Z");
        assert_eq!(s.expires_at, "2026-09-30T10:02:30.000Z");
        let (s, _) = w.set(&claude("s1"), "a1", SetWorking { thread_ids: Some(vec![]), message: None });
        assert!(s.view.thread_ids.is_empty());
        assert_eq!(s.view.message.as_deref(), Some("Tightening spacing"));
    }

    #[test]
    fn records_lapse_120_s_after_the_last_renewal() {
        let (c, w) = fixture();
        w.mark(&claude("s1"), "a1", &[]);
        c.advance(119);
        assert_eq!(w.for_artifact("a1").len(), 1);
        assert_eq!(w.renew("s1"), 1);
        c.advance(119);
        assert_eq!(w.for_artifact("a1").len(), 1, "renewed at 119 s");
        c.advance(1);
        assert!(w.for_artifact("a1").is_empty(), "hidden at 120 s, before any sweep");
        assert_eq!(w.sweep().0, ["a1".to_string()].into());
        assert!(w.sweep().is_empty(), "swept once");
        assert_eq!(w.renew("s1"), 0, "a lapsed record is not revived");
    }

    #[test]
    fn replying_to_the_last_named_thread_clears_the_record() {
        let (_c, w) = fixture();
        w.mark(&claude("s1"), "a1", &ids(&["t1", "t2"]));
        w.thread_done("s1", "a1", "t1");
        assert_eq!(w.for_artifact("a1")[0].thread_ids, ids(&["t2"]));
        assert!(!w.thread_done("s1", "a1", "t2").is_empty());
        assert!(w.for_artifact("a1").is_empty());
    }

    #[test]
    fn a_record_that_never_named_threads_survives_a_reply() {
        let (_c, w) = fixture();
        w.set(&claude("s1"), "a1", SetWorking { thread_ids: None, message: Some("Refactoring".into()) });
        assert!(w.thread_done("s1", "a1", "t9").is_empty());
        assert_eq!(w.for_artifact("a1").len(), 1);
    }

    #[test]
    fn thread_gone_touches_every_session_on_the_artifact() {
        let (_c, w) = fixture();
        w.mark(&claude("s1"), "a1", &ids(&["t1"]));
        w.mark(&Actor { session_id: "s2".into(), harness: "codex".into() }, "a1", &ids(&["t1", "t2"]));
        w.thread_gone("a1", "t1");
        let v = w.for_artifact("a1");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].harness, "codex");
        assert_eq!(v[0].thread_ids, ids(&["t2"]));
    }

    #[test]
    fn end_session_clear_and_artifact_gone() {
        let (_c, w) = fixture();
        w.mark(&claude("s1"), "a1", &[]);
        w.mark(&claude("s1"), "a2", &[]);
        w.mark(&claude("s2"), "a2", &[]);
        assert_eq!(w.end_session("s1").0, ["a1".to_string(), "a2".to_string()].into());
        assert_eq!(w.for_artifact("a2").len(), 1);
        assert!(!w.clear("s2", "a2", None).is_empty());
        w.mark(&claude("s3"), "a3", &[]);
        assert!(!w.artifact_gone("a3").is_empty());
        assert!(w.all().is_empty());
    }

    #[test]
    fn clear_with_threads_removes_only_those() {
        let (_c, w) = fixture();
        w.mark(&claude("s1"), "a1", &ids(&["t1", "t2"]));
        w.clear("s1", "a1", Some(&ids(&["t1"])));
        assert_eq!(w.threads_of("s1", "a1"), ids(&["t2"]));
    }

    #[test]
    fn views_are_newest_first_and_carry_no_session_id() {
        let (c, w) = fixture();
        w.mark(&claude("s1"), "a1", &[]);
        c.advance(1);
        w.mark(&Actor { session_id: "s2".into(), harness: "pi".into() }, "a1", &[]);
        let v = w.for_artifact("a1");
        assert_eq!(v[0].harness, "pi");
        let json = serde_json::to_string(&v).unwrap();
        assert!(!json.contains("s1") && !json.contains("s2") && !json.contains("session"), "{json}");
        assert_ne!(v[0].key, v[1].key);
        assert_eq!(w.for_session("s2")[0].session_id, "s2");
        assert_eq!(w.for_session("s2")[0].artifact_id, "a1");
    }

    #[test]
    fn skew_moves_the_registry_clock() {
        let (_c, w) = fixture();
        w.mark(&claude("s1"), "a1", &[]);
        w.skew(121);
        assert!(w.for_artifact("a1").is_empty());
    }

    #[test]
    fn messages_are_one_line_and_bounded() {
        assert_eq!(clean_message("  a\n\tb\u{2028}c\u{7}d  "), (Some("a b cd".into()), false));
        assert_eq!(clean_message(" \n "), (None, false));
        let long = "x".repeat(141);
        let (m, cut) = clean_message(&long);
        assert!(cut);
        assert_eq!(m.as_deref().unwrap().chars().count(), 140);
        assert!(m.unwrap().ends_with('…'));
        assert_eq!(clean_message(&"é".repeat(140)), (Some("é".repeat(140)), false));
    }
}
```

Add `pub mod working;` to `crates/clax-core/src/lib.rs`.

Run: `cargo test -p clax-core working`
Expected: FAIL to compile (`Working`, `ManualClock`, ... not found).

- [ ] **Step 2: Implement the registry**

Put this above the test module in `working.rs`:

```rust
//! The working signal (spec §10 "Working"): which harness session is acting
//! on which artifact, and on which of its threads, now. Records live in
//! memory only. Each lapses [`WORKING_TTL_SECS`] after its last renewal, so a
//! daemon restart starts with none and never shows stale work.

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

/// Seconds after its last renewal that a record lapses.
pub const WORKING_TTL_SECS: i64 = 120;
/// Longest message, in characters, after [`clean_message`].
pub const MAX_MESSAGE_CHARS: usize = 140;
/// Most threads one record names.
pub const MAX_WORKING_THREADS: usize = 20;

/// Where the registry reads the time.
pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

/// The system clock.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// A clock that moves only when told to.
pub struct ManualClock(Mutex<DateTime<Utc>>);

impl ManualClock {
    /// A clock stopped at `rfc3339`.
    ///
    /// # Panics
    /// When `rfc3339` does not parse.
    pub fn at(rfc3339: &str) -> ManualClock {
        ManualClock(Mutex::new(
            DateTime::parse_from_rfc3339(rfc3339).expect("an RFC 3339 time").with_timezone(&Utc),
        ))
    }
    pub fn advance(&self, secs: i64) {
        *self.0.lock().unwrap() += Duration::seconds(secs);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> DateTime<Utc> {
        *self.0.lock().unwrap()
    }
}

/// The session a record belongs to, as the daemon knows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Actor {
    pub session_id: String,
    pub harness: String,
}

/// A record as anyone may read it: never names the session.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WorkingView {
    /// A ULID minted when the record was created.
    pub key: String,
    pub harness: String,
    pub message: Option<String>,
    pub thread_ids: Vec<String>,
    pub started_at: String,
    pub last_heartbeat: String,
}

/// A record as its session's token holder reads it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SessionWorking {
    #[serde(flatten)]
    pub view: WorkingView,
    pub session_id: String,
    pub artifact_id: String,
    pub expires_at: String,
}

/// An explicit update: each `Some` field replaces the stored one.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SetWorking {
    pub thread_ids: Option<Vec<String>>,
    pub message: Option<String>,
}

/// The artifacts whose working list changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changed(pub BTreeSet<String>);

impl Changed {
    pub fn merge(&mut self, other: Changed) {
        self.0.extend(other.0);
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    fn one(aid: &str) -> Changed {
        Changed([aid.to_string()].into())
    }
}

struct Record {
    key: String,
    harness: String,
    message: Option<String>,
    threads: Vec<String>,
    /// The record has named a thread at some point: losing its last one clears it.
    had_threads: bool,
    started_at: DateTime<Utc>,
    heartbeat: DateTime<Utc>,
}

fn stamp(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Millis, true)
}

impl Record {
    fn view(&self) -> WorkingView {
        WorkingView {
            key: self.key.clone(),
            harness: self.harness.clone(),
            message: self.message.clone(),
            thread_ids: self.threads.clone(),
            started_at: stamp(self.started_at),
            last_heartbeat: stamp(self.heartbeat),
        }
    }
    fn add(&mut self, threads: &[String]) {
        for t in threads {
            if !self.threads.contains(t) && self.threads.len() < MAX_WORKING_THREADS {
                self.threads.push(t.clone());
            }
        }
        self.had_threads |= !self.threads.is_empty();
    }
}

/// `raw` as one line: whitespace runs (line and paragraph separators included)
/// become one space, other control characters are dropped, the ends are
/// trimmed. Empty is `None`. Past [`MAX_MESSAGE_CHARS`] it is cut to one
/// character less plus `…`; the flag says so.
pub fn clean_message(raw: &str) -> (Option<String>, bool) {
    let kept: String = raw.chars().filter(|c| c.is_whitespace() || !c.is_control()).collect();
    let one = kept.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.is_empty() {
        return (None, false);
    }
    if one.chars().count() <= MAX_MESSAGE_CHARS {
        return (Some(one), false);
    }
    let mut cut: String = one.chars().take(MAX_MESSAGE_CHARS - 1).collect();
    cut.push('…');
    (Some(cut), true)
}

type Key = (String, String);

/// The registry. Every method takes the lock once; none blocks on I/O.
pub struct Working {
    clock: Arc<dyn Clock>,
    skew: Mutex<Duration>,
    records: Mutex<BTreeMap<Key, Record>>,
}

impl Working {
    pub fn new(clock: Arc<dyn Clock>) -> Working {
        Working { clock, skew: Mutex::new(Duration::zero()), records: Mutex::new(BTreeMap::new()) }
    }

    /// The registry's time: its clock plus any [`Working::skew`].
    pub fn now(&self) -> DateTime<Utc> {
        self.clock.now() + *self.skew.lock().unwrap()
    }

    /// Moves the registry's time forward by `secs` (the debug build's test route).
    pub fn skew(&self, secs: i64) {
        *self.skew.lock().unwrap() += Duration::seconds(secs);
    }

    fn live(&self, r: &Record, now: DateTime<Utc>) -> bool {
        now - r.heartbeat < Duration::seconds(WORKING_TTL_SECS)
    }

    fn upsert<'a>(map: &'a mut BTreeMap<Key, Record>, who: &Actor, aid: &str, now: DateTime<Utc>) -> &'a mut Record {
        let r = map.entry((who.session_id.clone(), aid.to_string())).or_insert_with(|| Record {
            key: crate::new_ulid(),
            harness: who.harness.clone(),
            message: None,
            threads: Vec::new(),
            had_threads: false,
            started_at: now,
            heartbeat: now,
        });
        r.heartbeat = now;
        r
    }

    fn session_view(sid: &str, aid: &str, r: &Record) -> SessionWorking {
        SessionWorking {
            view: r.view(),
            session_id: sid.to_string(),
            artifact_id: aid.to_string(),
            expires_at: stamp(r.heartbeat + Duration::seconds(WORKING_TTL_SECS)),
        }
    }

    /// Drops lapsed records from `map` first, so a lapsed record is never updated in place.
    fn prune(&self, map: &mut BTreeMap<Key, Record>, now: DateTime<Utc>) -> Changed {
        let mut changed = Changed::default();
        map.retain(|(_, aid), r| {
            let keep = self.live(r, now);
            if !keep {
                changed.0.insert(aid.clone());
            }
            keep
        });
        changed
    }

    /// Creates or updates the record; `Some` fields replace (an empty thread
    /// list clears the threads without clearing the record). Always renews.
    pub fn set(&self, who: &Actor, aid: &str, s: SetWorking) -> (SessionWorking, Changed) {
        let now = self.now();
        let mut map = self.records.lock().unwrap();
        let mut changed = self.prune(&mut map, now);
        let r = Self::upsert(&mut map, who, aid, now);
        if let Some(t) = s.thread_ids {
            r.threads.clear();
            r.had_threads = false;
            r.add(&t);
        }
        if let Some(m) = s.message {
            r.message = Some(m);
        }
        changed.merge(Changed::one(aid));
        (Self::session_view(&who.session_id, aid, r), changed)
    }

    /// Creates or renews the record and adds `threads` (feedback reached the session).
    pub fn mark(&self, who: &Actor, aid: &str, threads: &[String]) -> Changed {
        let now = self.now();
        let mut map = self.records.lock().unwrap();
        let mut changed = self.prune(&mut map, now);
        let before = map.get(&(who.session_id.clone(), aid.to_string())).map(|r| r.threads.clone());
        let r = Self::upsert(&mut map, who, aid, now);
        r.add(threads);
        if before.as_ref() != Some(&r.threads) {
            changed.merge(Changed::one(aid));
        }
        changed
    }

    /// Renews every live record of session `sid`; how many.
    pub fn renew(&self, sid: &str) -> usize {
        let now = self.now();
        let mut map = self.records.lock().unwrap();
        let mut n = 0;
        for ((s, _), r) in map.iter_mut() {
            if s == sid && self.live(r, now) {
                r.heartbeat = now;
                n += 1;
            }
        }
        n
    }

    /// Removes `threads` from the record, or the record when `None`. A record
    /// that named threads and has none left is removed.
    pub fn clear(&self, sid: &str, aid: &str, threads: Option<&[String]>) -> Changed {
        let mut map = self.records.lock().unwrap();
        let key = (sid.to_string(), aid.to_string());
        let Some(r) = map.get_mut(&key) else { return Changed::default() };
        match threads {
            None => {
                map.remove(&key);
            }
            Some(ts) => {
                let n = r.threads.len();
                r.threads.retain(|t| !ts.contains(t));
                if r.threads.len() == n {
                    return Changed::default();
                }
                if r.had_threads && r.threads.is_empty() {
                    map.remove(&key);
                }
            }
        }
        Changed::one(aid)
    }

    /// The session replied to or resolved `tid`: renews the session, then
    /// takes `tid` out of its record on `aid`.
    pub fn thread_done(&self, sid: &str, aid: &str, tid: &str) -> Changed {
        self.renew(sid);
        self.clear(sid, aid, Some(&[tid.to_string()]))
    }

    /// `tid` was resolved by a viewer or deleted: out of every record on `aid`.
    pub fn thread_gone(&self, aid: &str, tid: &str) -> Changed {
        let sessions: Vec<String> = {
            let map = self.records.lock().unwrap();
            map.keys().filter(|(_, a)| a == aid).map(|(s, _)| s.clone()).collect()
        };
        let mut changed = Changed::default();
        for s in sessions {
            changed.merge(self.clear(&s, aid, Some(&[tid.to_string()])));
        }
        changed
    }

    /// Every record of `sid` (turn end or session end).
    pub fn end_session(&self, sid: &str) -> Changed {
        let mut changed = Changed::default();
        self.records.lock().unwrap().retain(|(s, aid), _| {
            let keep = s != sid;
            if !keep {
                changed.0.insert(aid.clone());
            }
            keep
        });
        changed
    }

    /// Every record on `aid`.
    pub fn artifact_gone(&self, aid: &str) -> Changed {
        let mut map = self.records.lock().unwrap();
        let n = map.len();
        map.retain(|(_, a), _| a != aid);
        if map.len() == n { Changed::default() } else { Changed::one(aid) }
    }

    /// Removes lapsed records; the artifacts whose list changed.
    pub fn sweep(&self) -> Changed {
        let now = self.now();
        self.prune(&mut self.records.lock().unwrap(), now)
    }

    /// The live records on `aid`, newest `started_at` first.
    pub fn for_artifact(&self, aid: &str) -> Vec<WorkingView> {
        let now = self.now();
        let map = self.records.lock().unwrap();
        let mut v: Vec<_> = map
            .iter()
            .filter(|((_, a), r)| a == aid && self.live(r, now))
            .map(|(_, r)| (r.started_at, r.view()))
            .collect();
        v.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.key.cmp(&a.1.key)));
        v.into_iter().map(|(_, x)| x).collect()
    }

    /// The live records of `sid`, by artifact ID.
    pub fn for_session(&self, sid: &str) -> Vec<SessionWorking> {
        let now = self.now();
        let map = self.records.lock().unwrap();
        map.iter()
            .filter(|((s, _), r)| s == sid && self.live(r, now))
            .map(|((s, a), r)| Self::session_view(s, a, r))
            .collect()
    }

    /// The threads the live record (`sid`, `aid`) names.
    pub fn threads_of(&self, sid: &str, aid: &str) -> Vec<String> {
        let now = self.now();
        let map = self.records.lock().unwrap();
        map.get(&(sid.to_string(), aid.to_string()))
            .filter(|r| self.live(r, now))
            .map(|r| r.threads.clone())
            .unwrap_or_default()
    }

    /// Every artifact with live records, and its list ([`Working::for_artifact`] order).
    pub fn all(&self) -> BTreeMap<String, Vec<WorkingView>> {
        let aids: BTreeSet<String> = self.records.lock().unwrap().keys().map(|(_, a)| a.clone()).collect();
        aids.into_iter()
            .map(|a| (a.clone(), self.for_artifact(&a)))
            .filter(|(_, v)| !v.is_empty())
            .collect()
    }
}
```

Run: `cargo test -p clax-core working`
Expected: PASS (11 tests).

- [ ] **Step 3: The `working` event**

In `crates/clax-core/src/events.rs`, add the variant after `FeedbackState`:

```rust
    /// The artifact's working list changed; `working` is the whole list
    /// (spec §10 "Working"), never naming a session.
    Working {
        artifact_id: String,
        working: Vec<crate::working::WorkingView>,
    },
```

Add `| Event::Working { artifact_id, .. }` to `artifact_id()` and `Event::Working { .. } => "working",` to `name()`. Add to the `names_match_the_serialised_type` list:

```rust
            Event::Working {
                artifact_id: "a".into(),
                working: vec![],
            },
```

Run: `cargo test -p clax-core`
Expected: PASS.

- [ ] **Step 4: Gates and staging**

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add crates/clax-core/src/working.rs crates/clax-core/src/lib.rs crates/clax-core/src/events.rs
git status --short   # staged; the controller commits ("Add the in-memory working registry and the working event")
```

---

### Task 8: Working routes, events and the sweeper

**Files:**
- Create: `crates/clax-server/src/working.rs`, `crates/clax-server/src/routes/working.rs`, `crates/clax-server/tests/api_working.rs`
- Modify: `crates/clax-server/src/lib.rs`, `crates/clax-server/src/state.rs`, `crates/clax-server/src/feedback.rs` (`FeedbackCtx` gains `working`), `crates/clax-server/src/routes/mod.rs`, `crates/clax-server/src/routes/events.rs`, `crates/clax-server/src/routes/artifacts.rs` (`with_owner`, `list`, `get`), `crates/clax-server/src/boot.rs` (`assemble`), `crates/clax-server/src/daemon.rs`, `crates/clax-server/src/testing.rs`

**Interfaces:**
- Consumes: Task 7's `Working`.
- Produces: `AppState.working: Arc<Working>`, `FeedbackCtx.working: Arc<Working>`, `TestServer.working: Arc<Working>`.
- Produces: `clax_server::working::{announce(events: &EventBus, w: &Working, changed: &Changed), sweep_and_announce(w: &Working, events: &EventBus), SWEEP_INTERVAL: Duration = 5 s}`.
- Produces the routes in the spec §6 amendment. Error codes: 401 `unauthorized`; 404 `not_found` (unknown session or artifact); 400 `unknown_session` (ended session); 400 `invalid_args` (`thread_ids` not ULIDs or more than 20); 400 `unknown_thread` (not a thread of the artifact); 400 `thread_not_open`; 400 `invalid_json`.
- Produces (debug builds only): `POST /api/_test/working/skew` (token) with `{secs}`, answering `{now}` after skewing and sweeping. Playwright uses it in Task 16.
- `GET /api/events?types=working[,…]`.

- [ ] **Step 1: Write the failing route tests**

`crates/clax-server/tests/api_working.rs`:

```rust
mod common;
use clax_core::working::{ManualClock, Working};
use common::TestServer;
use serde_json::{Value, json};
use std::sync::Arc;

async fn server() -> (TestServer, Arc<ManualClock>) {
    let clock = Arc::new(ManualClock::at("2026-09-30T10:00:00Z"));
    let c = clock.clone();
    let ts = TestServer::spawn_with(move |s| s.working = Arc::new(Working::new(c))).await;
    (ts, clock)
}

/// A claude session owning a one-version artifact with one sent thread.
async fn setup(ts: &TestServer) -> (String, String, String) {
    let s = ts.register_session("claude", "w1").await;
    let sid = s["id"].as_str().unwrap().to_string();
    let a = ts.publish_as(&sid, "T", "<main><h2>Quarterly goals</h2></main>").await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let t = ts.thread(&aid, 1, "@agent two columns").await;
    (sid, aid, t["id"].as_str().unwrap().to_string())
}

async fn put(ts: &TestServer, sid: &str, aid: &str, body: Value) -> reqwest::Response {
    ts.authed(ts.client.put(format!("{}/api/sessions/{sid}/working/{aid}", ts.base)))
        .json(&body)
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn setting_needs_the_token_and_reading_does_not() {
    let (ts, _) = server().await;
    let (sid, aid, tid) = setup(&ts).await;
    let url = format!("{}/api/sessions/{sid}/working/{aid}", ts.base);
    assert_eq!(ts.client.put(&url).json(&json!({})).send().await.unwrap().status(), 401);
    let res = put(&ts, &sid, &aid, json!({"thread_ids": [tid], "message": " Two\ncolumns "})).await;
    assert_eq!(res.status(), 200);
    let v: Value = res.json().await.unwrap();
    assert_eq!(v["working"]["message"], "Two columns");
    assert_eq!(v["working"]["session_id"], sid.as_str());
    assert_eq!(v["message_truncated"], false);
    let public: Value = ts.get(&format!("/api/artifacts/{aid}/working")).await.json().await.unwrap();
    let w = &public["working"][0];
    assert_eq!(w["harness"], "claude");
    assert_eq!(w["thread_ids"], json!([tid]));
    assert!(!public.to_string().contains(&sid), "{public}");
    assert!(w.get("session_id").is_none());
    let mine = ts.get_authed(&format!("/api/sessions/{sid}/working")).await;
    assert_eq!(mine.status(), 200);
    assert_eq!(ts.get(&format!("/api/sessions/{sid}/working")).await.status(), 401);
}

#[tokio::test]
async fn artifact_views_carry_the_working_list_without_session_ids() {
    let (ts, _) = server().await;
    let (sid, aid, _) = setup(&ts).await;
    put(&ts, &sid, &aid, json!({"message": "Refactoring"})).await;
    let one: Value = ts.get(&format!("/api/artifacts/{aid}")).await.json().await.unwrap();
    assert_eq!(one["artifact"]["working"][0]["message"], "Refactoring");
    let all: Value = ts.get("/api/artifacts").await.json().await.unwrap();
    assert_eq!(all["artifacts"][0]["working"][0]["harness"], "claude");
    assert!(all["artifacts"][0]["working"][0].get("session_id").is_none());
}

#[tokio::test]
async fn bad_requests_name_their_code() {
    let (ts, _) = server().await;
    let (sid, aid, tid) = setup(&ts).await;
    let code = |r: Value| r["error"]["code"].as_str().unwrap().to_string();
    let r = put(&ts, &sid, &aid, json!({"thread_ids": ["nope"]})).await;
    assert_eq!(r.status(), 400);
    assert_eq!(code(r.json().await.unwrap()), "invalid_args");
    let many: Vec<String> = (0..21).map(|_| clax_core::new_ulid()).collect();
    assert_eq!(code(put(&ts, &sid, &aid, json!({"thread_ids": many})).await.json().await.unwrap()), "invalid_args");
    let r = put(&ts, &sid, &aid, json!({"thread_ids": [clax_core::new_ulid()]})).await;
    assert_eq!(code(r.json().await.unwrap()), "unknown_thread");
    ts.client
        .post(format!("{}/api/artifacts/{aid}/threads/{tid}/resolve", ts.base))
        .send()
        .await
        .unwrap();
    let r = put(&ts, &sid, &aid, json!({"thread_ids": [tid]})).await;
    assert_eq!(code(r.json().await.unwrap()), "thread_not_open");
    let r = put(&ts, &sid, "7q3k9mzx2b4t", json!({})).await;
    assert_eq!(r.status(), 404);
    let r = put(&ts, &clax_core::new_ulid(), &aid, json!({})).await;
    assert_eq!(r.status(), 404);
    ts.authed(ts.client.patch(format!("{}/api/sessions/{sid}", ts.base)))
        .json(&json!({"ended": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(code(put(&ts, &sid, &aid, json!({})).await.json().await.unwrap()), "unknown_session");
}

#[tokio::test]
async fn a_long_message_is_cut_and_flagged() {
    let (ts, _) = server().await;
    let (sid, aid, _) = setup(&ts).await;
    let v: Value = put(&ts, &sid, &aid, json!({"message": "m".repeat(300)})).await.json().await.unwrap();
    assert_eq!(v["message_truncated"], true);
    assert_eq!(v["working"]["message"].as_str().unwrap().chars().count(), 140);
}

#[tokio::test]
async fn delete_clears_one_record_or_its_threads() {
    let (ts, _) = server().await;
    let (sid, aid, tid) = setup(&ts).await;
    put(&ts, &sid, &aid, json!({"thread_ids": [tid], "message": "x"})).await;
    let url = format!("{}/api/sessions/{sid}/working/{aid}", ts.base);
    let v: Value = ts.authed(ts.client.delete(format!("{url}?thread_ids={tid}"))).send().await.unwrap().json().await.unwrap();
    assert_eq!(v["cleared"], true);
    assert_eq!(v["working"], Value::Null, "its last thread went");
    put(&ts, &sid, &aid, json!({"message": "y"})).await;
    let v: Value = ts.authed(ts.client.delete(&url)).send().await.unwrap().json().await.unwrap();
    assert_eq!(v["cleared"], true);
    let v: Value = ts.authed(ts.client.delete(&url)).send().await.unwrap().json().await.unwrap();
    assert_eq!(v["cleared"], false);
}

#[tokio::test]
async fn renew_and_end_act_on_every_record_of_the_session() {
    let (ts, clock) = server().await;
    let (sid, aid, _) = setup(&ts).await;
    put(&ts, &sid, &aid, json!({})).await;
    clock.advance(100);
    let v: Value = ts.post_json(&format!("/api/sessions/{sid}/working/renew"), json!({})).await.json().await.unwrap();
    assert_eq!(v["renewed"], 1);
    clock.advance(100);
    let w: Value = ts.get(&format!("/api/artifacts/{aid}/working")).await.json().await.unwrap();
    assert_eq!(w["working"].as_array().unwrap().len(), 1, "renewed at 100 s, so alive at 200 s");
    let v: Value = ts.post_json(&format!("/api/sessions/{sid}/working/end"), json!({})).await.json().await.unwrap();
    assert_eq!(v["cleared"], 1);
    let w: Value = ts.get(&format!("/api/artifacts/{aid}/working")).await.json().await.unwrap();
    assert_eq!(w["working"], json!([]));
}

#[tokio::test]
async fn expiry_is_hidden_at_once_and_announced_by_the_sweep() {
    let (ts, clock) = server().await;
    let (sid, aid, _) = setup(&ts).await;
    let mut ev = ts.events(&format!("?artifact={aid}&types=working")).await;
    put(&ts, &sid, &aid, json!({"message": "x"})).await;
    let set = ev.next_named("working").await;
    assert_eq!(set["working"][0]["message"], "x");
    clock.advance(120);
    let w: Value = ts.get(&format!("/api/artifacts/{aid}/working")).await.json().await.unwrap();
    assert_eq!(w["working"], json!([]));
    clax_server::working::sweep_and_announce(&ts.working, &ts.events);
    let gone = ev.next_named("working").await;
    assert_eq!(gone, json!({"type": "working", "artifact_id": aid, "working": []}));
}

#[tokio::test]
async fn a_types_filter_drops_other_events() {
    let (ts, _) = server().await;
    let (sid, aid, _) = setup(&ts).await;
    let mut ev = ts.events("?types=working").await;
    ts.thread(&aid, 1, "plain note").await;
    put(&ts, &sid, &aid, json!({})).await;
    let (name, data) = ev.next().await;
    assert_eq!(name, "working");
    assert_eq!(data["artifact_id"], aid.as_str());
}
```

Run: `cargo test -p clax-server --test api_working`
Expected: FAIL to compile (`working` is not a field of `AppState`).

- [ ] **Step 2: State, announce, sweeper**

In `state.rs` add `pub working: Arc<clax_core::working::Working>,` with the doc comment `/// Working records (spec §10 "Working"), in memory.`. In `feedback.rs`, add `pub working: Arc<clax_core::working::Working>,` to `FeedbackCtx` and `working: self.working.clone(),` to `feedback_ctx()`. In `daemon.rs` and `testing.rs`, initialise `working: Arc::new(clax_core::working::Working::new(Arc::new(clax_core::working::SystemClock))),`, and give `TestServer` a `pub working: Arc<clax_core::working::Working>` field copied from the state after `f(&mut state)`.

`crates/clax-server/src/working.rs`:

```rust
//! Fan-out of working-record changes (`working` events), the sweeper, and
//! the automatic marks made when feedback reaches a session.

use clax_core::working::{Changed, Working};
use clax_core::{Event, EventBus};
use std::time::Duration;

/// How often lapsed records are removed and announced.
pub const SWEEP_INTERVAL: Duration = Duration::from_secs(5);

/// Sends each changed artifact's whole list.
pub fn announce(events: &EventBus, w: &Working, changed: &Changed) {
    for aid in &changed.0 {
        events.publish(Event::Working { artifact_id: aid.clone(), working: w.for_artifact(aid) });
    }
}

/// Removes lapsed records and announces the artifacts they were on.
pub fn sweep_and_announce(w: &Working, events: &EventBus) {
    let changed = w.sweep();
    announce(events, w, &changed);
}
```

Add `pub mod working;` to `lib.rs`. In `daemon.rs::serve`, after the reaper task, spawn the sweeper and abort it with the reaper:

```rust
    let (sweep_working, sweep_events) = (state_working.clone(), state_events.clone());
    let sweeper = tokio::spawn(async move {
        let mut every = tokio::time::interval(crate::working::SWEEP_INTERVAL);
        loop {
            every.tick().await;
            crate::working::sweep_and_announce(&sweep_working, &sweep_events);
        }
    });
```

Here `state_working` and `state_events` are clones of `state.working` and `state.events`, taken before `state` moves into `build_router_with_shutdown`. Add `sweeper.abort();` next to `reaper.abort();`. In the reaper's blocking closure, after `ctx.waiters.forget(id);`, add `crate::working::announce(&ctx.events, &ctx.working, &ctx.working.end_session(id));`.

- [ ] **Step 3: The routes**

`crates/clax-server/src/routes/working.rs`:

```rust
//! Working routes (spec §6, §10 "Working").

use super::artifacts::{body, parse_id, path};
use crate::auth::RequireToken;
use crate::error::ApiError;
use crate::state::AppState;
use crate::working::announce;
use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use clax_core::model::Session;
use clax_core::working::{Actor, MAX_WORKING_THREADS, SetWorking, clean_message};
use clax_core::{ArtifactId, CoreError, Store};
use serde::Deserialize;
use serde_json::{Value, json};

/// The live session `sid`: 404 when unknown, 400 `unknown_session` when ended.
fn live(st: &Store, sid: &str) -> clax_core::Result<Session> {
    match st.get_session(sid)? {
        None => Err(CoreError::NotFound),
        Some(s) if s.ended_at.is_some() => Err(CoreError::invalid("unknown_session", "the session has ended")),
        Some(s) => Ok(s),
    }
}

/// Checks `ids` (at most [`MAX_WORKING_THREADS`] ULIDs) are open threads of `aid`.
pub(crate) fn check_threads(st: &Store, aid: &ArtifactId, ids: &[String]) -> clax_core::Result<()> {
    if ids.len() > MAX_WORKING_THREADS {
        return Err(CoreError::invalid("invalid_args", format!("at most {MAX_WORKING_THREADS} thread_ids")));
    }
    for tid in ids {
        if !clax_core::is_ulid(tid) {
            return Err(CoreError::invalid("invalid_args", format!("'{tid}' is not a thread ID")));
        }
        match st.get_thread(tid)? {
            Some(t) if t.artifact_id == aid.as_str() => {
                if t.status != "open" {
                    return Err(CoreError::invalid("thread_not_open", format!("thread {tid} is resolved")));
                }
            }
            _ => return Err(CoreError::invalid("unknown_thread", format!("{tid} is not a thread of {aid}"))),
        }
    }
    Ok(())
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct SetBody {
    #[serde(default)]
    thread_ids: Option<Vec<String>>,
    #[serde(default)]
    message: Option<String>,
}

/// `PUT /api/sessions/<sid>/working/<aid>` (W): creates or updates the
/// record; given fields replace. `{working, message_truncated}`.
pub async fn put(
    State(s): State<AppState>,
    _t: RequireToken,
    p: Result<Path<(String, String)>, PathRejection>,
    req: Result<Json<SetBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let (sid, aid) = path(p)?;
    let id = parse_id(&aid)?;
    let b = body(req)?;
    let (message, truncated) = match b.message.as_deref() {
        Some(m) => clean_message(m),
        None => (None, false),
    };
    let (w, events) = (s.working.clone(), s.events.clone());
    let view = s
        .store_call(move |st| {
            let sess = live(st, &sid)?;
            st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            if let Some(t) = &b.thread_ids {
                check_threads(st, &id, t)?;
            }
            let who = Actor { session_id: sess.id, harness: sess.harness };
            let (view, changed) = w.set(&who, id.as_str(), SetWorking { thread_ids: b.thread_ids, message });
            announce(&events, &w, &changed);
            Ok(view)
        })
        .await?;
    Ok(Json(json!({"working": view, "message_truncated": truncated})))
}

#[derive(Deserialize)]
pub struct ClearQuery {
    thread_ids: Option<String>,
}

/// `DELETE /api/sessions/<sid>/working/<aid>` (W): the record, or with
/// `?thread_ids=a,b` only those threads. `{cleared, working}` (`working` is
/// what remains, or null).
pub async fn delete(
    State(s): State<AppState>,
    _t: RequireToken,
    p: Result<Path<(String, String)>, PathRejection>,
    q: Result<Query<ClearQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let (sid, aid) = path(p)?;
    let id = parse_id(&aid)?;
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let threads: Option<Vec<String>> =
        q.thread_ids.map(|t| t.split(',').filter(|x| !x.is_empty()).map(str::to_string).collect());
    let (w, events) = (s.working.clone(), s.events.clone());
    let (cleared, left) = s
        .store_call(move |st| {
            live(st, &sid)?;
            let changed = w.clear(&sid, id.as_str(), threads.as_deref());
            announce(&events, &w, &changed);
            let left = w.for_session(&sid).into_iter().find(|r| r.artifact_id == id.as_str());
            Ok((!changed.is_empty(), left))
        })
        .await?;
    Ok(Json(json!({"cleared": cleared, "working": left})))
}

/// `GET /api/sessions/<sid>/working` (token): `{working}` with session fields.
pub async fn for_session(
    State(s): State<AppState>,
    _t: RequireToken,
    p: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let sid = path(p)?;
    let w = s.working.clone();
    let list = s
        .store_call(move |st| {
            st.get_session(&sid)?.ok_or(CoreError::NotFound)?;
            Ok(w.for_session(&sid))
        })
        .await?;
    Ok(Json(json!({"working": list})))
}

/// `GET /api/artifacts/<aid>/working` (no token): `{working}`, never naming a session.
pub async fn for_artifact(
    State(s): State<AppState>,
    p: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&path(p)?)?;
    let w = s.working.clone();
    let list = s
        .store_call(move |st| {
            st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            Ok(w.for_artifact(id.as_str()))
        })
        .await?;
    Ok(Json(json!({"working": list})))
}

/// `POST /api/sessions/<sid>/working/renew` (W): `{renewed}`.
pub async fn renew(
    State(s): State<AppState>,
    _t: RequireToken,
    p: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let sid = path(p)?;
    let w = s.working.clone();
    let n = s.store_call(move |st| { live(st, &sid)?; Ok(w.renew(&sid)) }).await?;
    Ok(Json(json!({"renewed": n})))
}

/// `POST /api/sessions/<sid>/working/end` (W; the turn ended): `{cleared}`.
pub async fn end(
    State(s): State<AppState>,
    _t: RequireToken,
    p: Result<Path<String>, PathRejection>,
) -> Result<Json<Value>, ApiError> {
    let sid = path(p)?;
    let (w, events) = (s.working.clone(), s.events.clone());
    let n = s
        .store_call(move |st| {
            live(st, &sid)?;
            let changed = w.end_session(&sid);
            announce(&events, &w, &changed);
            Ok(changed.0.len())
        })
        .await?;
    Ok(Json(json!({"cleared": n})))
}

#[cfg(debug_assertions)]
#[derive(Deserialize)]
pub struct SkewBody {
    secs: i64,
}

/// Debug builds only: moves the working clock forward and sweeps, so browser
/// tests can observe expiry without waiting. `{now}`.
#[cfg(debug_assertions)]
pub async fn skew(
    State(s): State<AppState>,
    _t: RequireToken,
    req: Result<Json<SkewBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let b = body(req)?;
    s.working.skew(b.secs);
    crate::working::sweep_and_announce(&s.working, &s.events);
    Ok(Json(json!({"now": s.working.now().to_rfc3339()})))
}
```

In `routes/mod.rs`, add `pub mod working;` and these routes to `api_fast`:

```rust
        .route("/api/artifacts/{aid}/working", get(working::for_artifact))
        .route("/api/sessions/{id}/working", get(working::for_session))
        .route("/api/sessions/{id}/working/renew", post(working::renew))
        .route("/api/sessions/{id}/working/end", post(working::end))
        .route(
            "/api/sessions/{id}/working/{aid}",
            axum::routing::put(working::put).delete(working::delete),
        )
```

and, after the `#[cfg(feature = "test-routes")]` block:

```rust
    #[cfg(debug_assertions)]
    let api_fast = api_fast.route("/api/_test/working/skew", post(working::skew));
```

- [ ] **Step 4: Artifact views carry `working`; the `types` filter**

In `routes/artifacts.rs`, give `with_owner` a third parameter `working: &[clax_core::working::WorkingView]` and set `v["working"] = json!(working);`. `list` passes `s.working.all()` lookups (`all.get(&a.id).map(Vec::as_slice).unwrap_or(&[])`, with `all` computed once before the loop and moved into the closure). `get` passes `s.working.for_artifact(id.as_str())`. The port made `with_owner` `pub(crate)` for the bootstrap block. In `crates/clax-server/src/boot.rs` `assemble`, pass `s.working.for_artifact(<the artifact ID>)` as well, so the embedded artifact carries its `working` list. Add `crates/clax-server/src/boot.rs` to this task's files and its `git add`. Expected JSON in `crates/clax-server/tests/shell_boot.rs` gains `"working": []`.

In `routes/events.rs`, add `types: Option<String>` to `EventsQuery` (doc: ``When set, only events whose name is in this comma list are sent; `ready` and `resync` always are.``). Parse it once into `Option<BTreeSet<String>>`, and in the `filter_map` after the artifact check add:

```rust
        if let Some(t) = &types
            && !t.contains(ev.name())
        {
            return None;
        }
```

Update the doc comment's event list to include `working`.

Run: `cargo test -p clax-server --test api_working`
Expected: PASS (8 tests). Then `cargo test -p clax-server` must pass too. Existing tests comparing exact artifact JSON gain `"working": []`: add it to their expected values, and change nothing else.

- [ ] **Step 5: Gates and staging**

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add crates/clax-server/src/working.rs crates/clax-server/src/routes/working.rs crates/clax-server/tests/api_working.rs \
  crates/clax-server/src/lib.rs crates/clax-server/src/state.rs crates/clax-server/src/feedback.rs crates/clax-server/src/routes/mod.rs \
  crates/clax-server/src/routes/events.rs crates/clax-server/src/routes/artifacts.rs crates/clax-server/src/daemon.rs crates/clax-server/src/testing.rs \
  crates/clax-server/src/boot.rs
git add -u crates/clax-server/tests
git status --short   # staged; the controller commits ("Serve working records over REST and SSE, and sweep lapsed ones")
```

---

### Task 9: Automatic marks, renewals and clears in the daemon

**Files:**
- Modify: `crates/clax-server/src/working.rs`, `crates/clax-server/src/routes/feedback.rs`, `crates/clax-server/src/push.rs`, `crates/clax-server/src/routes/threads.rs`, `crates/clax-server/src/routes/artifacts.rs` (`publish`, `delete`), `crates/clax-server/src/routes/sessions.rs` (`patch`)
- Create: `crates/clax-server/tests/api_working_auto.rs`

**Interfaces:**
- Produces: `clax_server::working::{mark_items(ctx: &FeedbackCtx, st: &Store, session_id: &str, items: &[FeedbackItem]) -> clax_core::Result<()>, renew_for_tier(ctx: &FeedbackCtx, session_id: &str, tier: Tier)}`.
- The event table under "Design: the working record" is the contract of this task.

- [ ] **Step 1: Write the failing tests**

`crates/clax-server/tests/api_working_auto.rs`:

```rust
mod common;
use clax_core::working::{ManualClock, Working};
use common::TestServer;
use serde_json::{Value, json};
use std::sync::Arc;

async fn server() -> (TestServer, Arc<ManualClock>) {
    let clock = Arc::new(ManualClock::at("2026-09-30T10:00:00Z"));
    let c = clock.clone();
    (TestServer::spawn_with(move |s| s.working = Arc::new(Working::new(c))).await, clock)
}

async fn setup(ts: &TestServer, harness: &str) -> (String, String, String) {
    let s = ts.register_session(harness, &format!("{harness}-auto")).await;
    let sid = s["id"].as_str().unwrap().to_string();
    let a = ts.publish_as(&sid, "T", "<main><h2>Quarterly goals</h2></main>").await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let t = ts.thread(&aid, 1, "@agent two columns").await;
    (sid, aid, t["id"].as_str().unwrap().to_string())
}

async fn working(ts: &TestServer, aid: &str) -> Vec<Value> {
    let v: Value = ts.get(&format!("/api/artifacts/{aid}/working")).await.json().await.unwrap();
    v["working"].as_array().unwrap().clone()
}

async fn take(ts: &TestServer, sid: &str, tier: &str) -> Value {
    ts.get_authed(&format!("/api/sessions/{sid}/feedback?tier={tier}")).await.json().await.unwrap()
}

#[tokio::test]
async fn each_delivering_tier_marks_the_session_working_on_the_thread() {
    for tier in ["piggyback", "stop_hook", "prompt_hook", "wait", "inject"] {
        let (ts, _) = server().await;
        let (sid, aid, tid) = setup(&ts, if tier == "inject" { "pi" } else { "claude" }).await;
        assert!(working(&ts, &aid).await.is_empty(), "{tier}");
        let got = take(&ts, &sid, tier).await;
        assert_eq!(got["feedback"].as_array().unwrap().len(), 1, "{tier}");
        let w = working(&ts, &aid).await;
        assert_eq!(w.len(), 1, "{tier}");
        assert_eq!(w[0]["thread_ids"], json!([tid]), "{tier}");
    }
}

#[tokio::test]
async fn an_empty_hook_or_tool_take_renews_but_a_heartbeat_and_a_wait_poll_do_not() {
    let (ts, clock) = server().await;
    let (sid, aid, _) = setup(&ts, "claude").await;
    take(&ts, &sid, "piggyback").await;
    clock.advance(100);
    ts.authed(ts.client.patch(format!("{}/api/sessions/{sid}", ts.base)))
        .json(&json!({"heartbeat": true})).send().await.unwrap();
    take(&ts, &sid, "wait").await;
    clock.advance(20);
    assert!(working(&ts, &aid).await.is_empty(), "neither the heartbeat nor the wait poll renewed");
    let (ts, clock) = server().await;
    let (sid, aid, _) = setup(&ts, "claude").await;
    take(&ts, &sid, "piggyback").await;
    clock.advance(100);
    take(&ts, &sid, "stop_hook").await;
    clock.advance(100);
    assert_eq!(working(&ts, &aid).await.len(), 1, "the empty stop_hook take renewed at 100 s");
}

#[tokio::test]
async fn the_agent_reply_to_the_last_named_thread_clears() {
    let (ts, _) = server().await;
    let (sid, aid, tid) = setup(&ts, "claude").await;
    take(&ts, &sid, "piggyback").await;
    let mut ev = ts.events(&format!("?artifact={aid}&types=working")).await;
    let res = ts.authed(ts.client.post(format!("{}/api/artifacts/{aid}/threads/{tid}/comments", ts.base)))
        .header("x-clax-session", &sid)
        .json(&json!({"body": "Done.", "author_kind": "agent"}))
        .send().await.unwrap();
    assert_eq!(res.status(), 201);
    assert_eq!(ev.next_named("working").await["working"], json!([]));
    assert!(working(&ts, &aid).await.is_empty());
}

#[tokio::test]
async fn an_agent_resolve_and_a_viewer_resolve_take_the_thread_out() {
    let (ts, _) = server().await;
    let (sid, aid, tid) = setup(&ts, "claude").await;
    take(&ts, &sid, "piggyback").await;
    ts.authed(ts.client.post(format!("{}/api/artifacts/{aid}/threads/{tid}/resolve", ts.base)))
        .header("x-clax-session", &sid).json(&json!({"as": "agent"})).send().await.unwrap();
    assert!(working(&ts, &aid).await.is_empty());
    let t2 = ts.thread(&aid, 1, "@agent again").await;
    take(&ts, &sid, "piggyback").await;
    assert_eq!(working(&ts, &aid).await.len(), 1);
    ts.client.post(format!("{}/api/artifacts/{aid}/threads/{}/resolve", ts.base, t2["id"].as_str().unwrap()))
        .send().await.unwrap();
    assert!(working(&ts, &aid).await.is_empty());
}

#[tokio::test]
async fn a_publish_by_the_session_clears_its_record_on_that_artifact() {
    let (ts, _) = server().await;
    let (sid, aid, _) = setup(&ts, "claude").await;
    take(&ts, &sid, "piggyback").await;
    let res = ts.authed(ts.client.post(format!("{}/api/artifacts/{aid}/versions", ts.base)))
        .header("x-clax-session", &sid)
        .json(&json!({"if_version": 1, "files": {"index.html": {"content": "<p>2</p>", "encoding": "utf8"}}}))
        .send().await.unwrap();
    assert_eq!(res.status(), 201);
    assert!(working(&ts, &aid).await.is_empty());
}

#[tokio::test]
async fn session_end_and_artifact_delete_clear() {
    let (ts, _) = server().await;
    let (sid, aid, _) = setup(&ts, "claude").await;
    take(&ts, &sid, "piggyback").await;
    ts.authed(ts.client.patch(format!("{}/api/sessions/{sid}", ts.base)))
        .json(&json!({"ended": true})).send().await.unwrap();
    assert!(working(&ts, &aid).await.is_empty());
    let (ts, _) = server().await;
    let (sid, aid, _) = setup(&ts, "claude").await;
    take(&ts, &sid, "piggyback").await;
    let mut ev = ts.events("?types=working").await;
    ts.authed(ts.client.delete(format!("{}/api/artifacts/{aid}", ts.base))).send().await.unwrap();
    assert_eq!(ev.next_named("working").await["working"], json!([]));
}

#[tokio::test]
async fn a_thread_delete_takes_it_out() {
    let (ts, _) = server().await;
    let (sid, aid, tid) = setup(&ts, "claude").await;
    take(&ts, &sid, "piggyback").await;
    let named = ts.viewer(Some("Alex")).await;
    let res = ts.client.delete(format!("{}/api/artifacts/{aid}/threads/{tid}", ts.base))
        .header("cookie", format!("clax_viewer={}", named.cookie))
        .send().await.unwrap();
    assert_eq!(res.status(), 200);
    assert!(working(&ts, &aid).await.is_empty());
}
```

Run: `cargo test -p clax-server --test api_working_auto`
Expected: FAIL (nothing marks yet).

- [ ] **Step 2: Mark and renew in the feedback route and in push**

Append to `crates/clax-server/src/working.rs`:

```rust
use crate::feedback::FeedbackCtx;
use clax_core::working::Actor;
use clax_core::{FeedbackItem, Store, Tier};
use std::collections::BTreeMap;

/// Tiers whose takes are hook runs or tool calls: each renews the session's records.
pub fn renew_for_tier(ctx: &FeedbackCtx, session_id: &str, tier: Tier) {
    if matches!(tier, Tier::Piggyback | Tier::StopHook | Tier::PromptHook) {
        ctx.working.renew(session_id);
    }
}

/// Feedback reached `session_id`: marks it working on each item's artifact
/// and thread, and announces the changes.
pub fn mark_items(ctx: &FeedbackCtx, st: &Store, session_id: &str, items: &[FeedbackItem]) -> clax_core::Result<()> {
    if items.is_empty() {
        return Ok(());
    }
    let Some(sess) = st.get_session(session_id)? else { return Ok(()) };
    let who = Actor { session_id: sess.id, harness: sess.harness };
    let mut by_artifact: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for i in items {
        by_artifact.entry(&i.artifact_id).or_default().push(i.thread_id.clone());
    }
    let mut changed = clax_core::working::Changed::default();
    for (aid, tids) in by_artifact {
        changed.merge(ctx.working.mark(&who, aid, &tids));
    }
    announce(&ctx.events, &ctx.working, &changed);
    Ok(())
}
```

In `routes/feedback.rs::poll`, inside the `store_call` closure right after `apply(&ctx, st, &touched);`:

```rust
                crate::working::renew_for_tier(&ctx, &t.session_id, t.tier);
                if t.tier != Tier::Queue {
                    crate::working::mark_items(&ctx, st, &t.session_id, &items)?;
                }
```

In `push.rs::dispatch`, clone `items` into the spawned task (`let marked = items.clone();` before the spawn), and in the blocking settle closure, inside `if error.is_none()` (add that branch next to the failure branch):

```rust
                if error.is_none()
                    && let Err(e) = crate::working::mark_items(&ctx, &store, &sid, &marked)
                {
                    tracing::warn!(session = %sid, error = %e, "marking a queued session working failed");
                }
```

- [ ] **Step 3: Clear on reply, resolve, delete, publish, session end**

`routes/threads.rs`:
- `comment`, agent branch, after `touched.merge(st.acknowledge(...))`: `let changed = ctx.working.thread_done(&sess.id, &aid, &tid); crate::working::announce(&ctx.events, &ctx.working, &changed);`
- `resolve`: in the agent branch, after the resolve succeeds, the same `thread_done` with the agent session's ID. In the viewer branch, `ctx.working.thread_gone(&aid, &tid)`, announced.
- `delete` (viewer or agent): after the delete succeeds, `thread_gone`, announced.

`routes/artifacts.rs`:
- `publish`: inside `if let Some(sid) = &session`, after `ensure_watch`, add `let changed = ctx.working.clear(sid, artifact.id.as_str(), None); crate::working::announce(&ctx.events, &ctx.working, &changed);`. Task 13 inserts the linking just before this clear.
- `delete`: after `st.delete_artifact(&id)?`, add `crate::working::announce(&events, &working, &working.artifact_gone(id.as_str()));`, with `working` cloned from `s.working` before the closure.

`routes/sessions.rs::patch`, `ended` branch, after `ctx.waiters.forget(&id);`: `crate::working::announce(&ctx.events, &ctx.working, &ctx.working.end_session(&id));`.

Run: `cargo test -p clax-server --test api_working_auto`
Expected: PASS (7 tests). Then `cargo test -p clax-server`: PASS.

- [ ] **Step 4: A Codex queue success marks**

Append to `crates/clax-server/tests/api_push.rs` a test that reuses that file's fake-`codex` helper (the script that exits 0) to set up a Codex session with `codex_home`, a published artifact and a sent thread. It waits for the push the way that file's existing success test does, then asserts `GET /api/artifacts/<aid>/working` lists one `codex` record naming the thread. A second test with the failing fake asserts the list stays empty. Name them `a_queue_that_exits_0_marks_the_session_working` and `a_failed_queue_marks_nothing`.

Run: `cargo test -p clax-server --test api_push`
Expected: PASS.

- [ ] **Step 5: Gates and staging**

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add crates/clax-server/src/working.rs crates/clax-server/src/routes/feedback.rs crates/clax-server/src/push.rs \
  crates/clax-server/src/routes/threads.rs crates/clax-server/src/routes/artifacts.rs crates/clax-server/src/routes/sessions.rs \
  crates/clax-server/tests/api_working_auto.rs crates/clax-server/tests/api_push.rs
git status --short   # staged; the controller commits ("Mark sessions working when feedback reaches them, and clear on reply, publish and end")
```

---

### Task 10: The `working` tool (MCP and Pi), and Pi's renewals and turn end

Two parts: the tool itself on every harness (A), then Pi's automatic renewal on tool calls and its turn end (B). They are one task because the Pi extension changes in both, and one commit keeps it coherent.

#### A. The `working` tool and the twenty-three tool lists

The MCP tool and Pi's `clax_working` land together. `scripts/test-plugins.sh` requires every fixture description in both `tools.rs` and `clax.ts`.

**Files:**
- Modify: `plugins/pi/test/fixtures/contract.json`, `crates/clax-mcp/src/tools.rs`, `crates/clax-mcp/src/client.rs`, `crates/clax-mcp/tests/comments.rs`, `crates/clax-mcp/tests/shim.rs` (count 22 → 23), `plugins/pi/src/clax.ts`, `plugins/pi/src/client.ts`, `plugins/pi/test/clax.test.ts`, `scripts/test-plugins.sh`, `docs/contract.md`, `README.md`, `plugins/claude-code/README.md`, `plugins/clax/README.md`, `plugins/pi/README.md`, `plugins/*/skills/clax/SKILL.md` (generated block and "Comment loop")

**Interfaces:**
- Produces: `clax_mcp::tools::WorkingArgs { url_or_id: String, thread_ids: Option<Vec<String>>, message: Option<String>, done: Option<bool> }` and `ClaxTools::working`.
- Produces: `DaemonClient::{set_working(id: &str, body: &Value) -> Result<Value>, clear_working(id: &str, threads: Option<&[String]>) -> Result<Value>, renew_working() -> Result<Value>, end_working() -> Result<Value>}` and the same four on Pi's `DaemonClient` (`setWorking`, `clearWorking`, `renewWorking`, `endWorking`).
- The description string, verbatim everywhere:

```text
Tell the person you are working on an artifact: its page's top bar shows `<harness> working on N` (or `<harness>: <message>`), its gallery card a chip, and `<harness> is working on it` on each thread in `thread_ids`. Comments sent to you mark you working automatically; call this for other work or to add a short `message` (at most 140 characters). It clears when you reply to those threads, publish the artifact, end your turn, or go 2 minutes without a tool call; `done: true` clears it now.
```

- Result: `{artifact_id, url, working: true, message, thread_ids, started_at, expires_in_s: 120, message_truncated}`, or `{artifact_id, url, working: false, cleared}`. Errors are the daemon's codes, passed through (`invalid_args`, `not_found`, `unknown_thread`, `thread_not_open`, `unknown_session`), plus the tool's own `invalid_id`, `invalid_args` and `no_session`.

- [ ] **Step 1: The fixture and the failing MCP test**

In `plugins/pi/test/fixtures/contract.json`, insert into `tools` after `wait_for_feedback`:

```json
    {"name": "working", "description": "Tell the person you are working on an artifact: its page's top bar shows `<harness> working on N` (or `<harness>: <message>`), its gallery card a chip, and `<harness> is working on it` on each thread in `thread_ids`. Comments sent to you mark you working automatically; call this for other work or to add a short `message` (at most 140 characters). It clears when you reply to those threads, publish the artifact, end your turn, or go 2 minutes without a tool call; `done: true` clears it now."},
```

Append to `crates/clax-mcp/tests/comments.rs` (add `WorkingArgs` to the `use clax_mcp::tools::{...}` list):

```rust
#[tokio::test]
async fn working_marks_the_artifact_and_done_clears_it() {
    let ts = TestServer::spawn().await;
    let (tools, _sid) = session_tools(&ts).await;
    let (pub_, _) = blocks(&tools.publish(Parameters(PublishArgs { html: Some("<h2>Goals</h2>".into()), title: Some("T".into()), ..Default::default() })).await.unwrap());
    let aid = pub_["artifact_id"].as_str().unwrap().to_string();
    let tid = ts.thread(&aid, 1, "@agent columns").await["id"].as_str().unwrap().to_string();
    let (v, _) = blocks(&tools.working(Parameters(WorkingArgs {
        url_or_id: aid.clone(),
        thread_ids: Some(vec![tid.clone()]),
        message: Some("Two columns".into()),
        done: None,
    })).await.unwrap());
    assert_eq!(v["working"], true);
    assert_eq!(v["message"], "Two columns");
    assert_eq!(v["thread_ids"], json!([tid]));
    assert_eq!(v["expires_in_s"], 120);
    assert_eq!(v["message_truncated"], false);
    let public: Value = ts.get(&format!("/api/artifacts/{aid}/working")).await.json().await.unwrap();
    assert_eq!(public["working"][0]["message"], "Two columns");
    let (v, _) = blocks(&tools.working(Parameters(WorkingArgs { url_or_id: aid.clone(), done: Some(true), ..Default::default() })).await.unwrap());
    assert_eq!(v, json!({"artifact_id": aid, "url": v["url"], "working": false, "cleared": true, "feedback": v["feedback"]}));
}

#[tokio::test]
async fn working_refuses_bad_threads_and_the_sessionless_endpoint() {
    let ts = TestServer::spawn().await;
    let (tools, _) = session_tools(&ts).await;
    let (pub_, _) = blocks(&tools.publish(Parameters(PublishArgs { html: Some("<p>".into()), title: Some("T".into()), ..Default::default() })).await.unwrap());
    let aid = pub_["artifact_id"].as_str().unwrap().to_string();
    let err = |r: CallToolResult| -> String {
        assert_eq!(r.is_error, Some(true));
        let v: Value = serde_json::from_str(&r.content[0].as_text().unwrap().text).unwrap();
        v["error"]["code"].as_str().unwrap().to_string()
    };
    let r = tools.working(Parameters(WorkingArgs { url_or_id: aid.clone(), thread_ids: Some(vec!["x".into()]), ..Default::default() })).await.unwrap();
    assert_eq!(err(r), "invalid_args");
    let r = tools.working(Parameters(WorkingArgs { url_or_id: aid.clone(), thread_ids: Some(vec![clax_core::new_ulid()]), ..Default::default() })).await.unwrap();
    assert_eq!(err(r), "unknown_thread");
    let sessionless = ClaxTools::new(
        DaemonClient::new(ts.base.clone(), ts.token.clone(), None),
        ts.base.clone(), None, ts.home.log_path(),
    );
    let r = sessionless.working(Parameters(WorkingArgs { url_or_id: aid, ..Default::default() })).await.unwrap();
    assert_eq!(err(r), "no_session");
}
```

Run: `cargo test -p clax-mcp --test comments working`
Expected: FAIL to compile (`WorkingArgs` not found).

- [ ] **Step 2: The MCP tool**

In `crates/clax-mcp/src/tools.rs`, change the header comment to `twenty-three tools`, and add:

```rust
#[derive(Clone, Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkingArgs {
    /// Artifact URL or ID.
    pub url_or_id: String,
    /// Threads of the artifact you are acting on (at most 20); replaces the ones named before.
    pub thread_ids: Option<Vec<String>>,
    /// What you are doing, in a few words (at most 140 characters).
    pub message: Option<String>,
    /// Clear it now (with `thread_ids`, only those threads).
    pub done: Option<bool>,
}
```

and in `impl ClaxTools`:

```rust
    async fn do_working(&self, a: WorkingArgs) -> Outcome {
        let id = artifact_id(&a.url_or_id)?;
        if let Some(t) = &a.thread_ids {
            if t.len() > clax_core::working::MAX_WORKING_THREADS {
                return Err(invalid("at most 20 thread_ids"));
            }
            for tid in t {
                check_thread_id(tid)?;
            }
        }
        self.require_session().await?;
        if a.done.unwrap_or(false) {
            let r = self.client.clear_working(&id, a.thread_ids.as_deref()).await.map_err(|e| self.fail(e))?;
            let still = !r["working"].is_null();
            return Ok(json!({"artifact_id": id, "url": self.artifact_url(&id), "working": still, "cleared": r["cleared"]}));
        }
        let mut body = json!({});
        if let Some(t) = &a.thread_ids {
            body["thread_ids"] = json!(t);
        }
        if let Some(m) = &a.message {
            body["message"] = json!(m);
        }
        let r = self.client.set_working(&id, &body).await.map_err(|e| self.fail(e))?;
        let w = &r["working"];
        Ok(json!({
            "artifact_id": id,
            "url": self.artifact_url(&id),
            "working": true,
            "message": w["message"],
            "thread_ids": w["thread_ids"],
            "started_at": w["started_at"],
            "expires_in_s": clax_core::working::WORKING_TTL_SECS,
            "message_truncated": r["message_truncated"],
        }))
    }
```

and in the `#[tool_router]` block after `wait_for_feedback`:

```rust
    #[tool(
        description = "Tell the person you are working on an artifact: its page's top bar shows `<harness> working on N` (or `<harness>: <message>`), its gallery card a chip, and `<harness> is working on it` on each thread in `thread_ids`. Comments sent to you mark you working automatically; call this for other work or to add a short `message` (at most 140 characters). It clears when you reply to those threads, publish the artifact, end your turn, or go 2 minutes without a tool call; `done: true` clears it now."
    )]
    pub async fn working(
        &self,
        Parameters(args): Parameters<WorkingArgs>,
    ) -> Result<CallToolResult, McpError> {
        self.finish(self.do_working(args).await).await
    }
```

In `crates/clax-mcp/src/client.rs`, after `unwatch`:

```rust
    /// `PUT /api/sessions/<sid>/working/<id>`: `{working, message_truncated}`.
    pub async fn set_working(&self, id: &str, body: &Value) -> Result<Value> {
        self.json(|c| c.request(reqwest::Method::PUT, &format!("{}/working/{id}", c.session_path())).json(body))
            .await
    }

    /// `DELETE /api/sessions/<sid>/working/<id>[?thread_ids=…]`: `{cleared, working}`.
    pub async fn clear_working(&self, id: &str, threads: Option<&[String]>) -> Result<Value> {
        let q: Vec<(&str, String)> = threads.map(|t| vec![("thread_ids", t.join(","))]).unwrap_or_default();
        self.json(|c| c.request(reqwest::Method::DELETE, &format!("{}/working/{id}", c.session_path())).query(&q))
            .await
    }

    /// `POST /api/sessions/<sid>/working/renew`: `{renewed}`.
    pub async fn renew_working(&self) -> Result<Value> {
        self.json(|c| c.request(reqwest::Method::POST, &format!("{}/working/renew", c.session_path())).json(&json!({})))
            .await
    }

    /// `POST /api/sessions/<sid>/working/end`: `{cleared}`.
    pub async fn end_working(&self) -> Result<Value> {
        self.json(|c| c.request(reqwest::Method::POST, &format!("{}/working/end", c.session_path())).json(&json!({})))
            .await
    }
```

In `crates/clax-mcp/tests/shim.rs`, change `.len(), 22)` to `.len(), 23)`.

Run: `cargo test -p clax-mcp`
Expected: PASS.

- [ ] **Step 3: Pi's `clax_working`**

In `plugins/pi/src/client.ts`, after `unwatch`:

```ts
  /** `PUT /api/sessions/<sid>/working/<id>`: `{working, message_truncated}`. */
  setWorking(id: string, body: { thread_ids?: string[]; message?: string }): Promise<any> {
    return this.json(() => `${this.sessionPath()}/working/${id}`, this.jsonBody("PUT", body));
  }

  /** `DELETE /api/sessions/<sid>/working/<id>`: `{cleared, working}`. */
  clearWorking(id: string, threadIds?: string[]): Promise<any> {
    const q = threadIds ? `?${new URLSearchParams({ thread_ids: threadIds.join(",") })}` : "";
    return this.json(() => `${this.sessionPath()}/working/${id}${q}`, { method: "DELETE" });
  }

  /** `POST /api/sessions/<sid>/working/renew`: `{renewed}`. Never starts a daemon. */
  renewWorking(): Promise<any> {
    return this.json(() => `${this.sessionPath()}/working/renew`, this.jsonBody("POST", {}), this.discoverFn);
  }

  /** `POST /api/sessions/<sid>/working/end`: `{cleared}`. Never starts a daemon. */
  endWorking(): Promise<any> {
    return this.json(() => `${this.sessionPath()}/working/end`, this.jsonBody("POST", {}), this.discoverFn);
  }
```

In `plugins/pi/src/clax.ts`, change the header comment to `twenty-three`, add the schema after `WaitArgs`:

```ts
const WorkingArgs = Type.Object({
  url_or_id: urlOrId,
  thread_ids: opt(Type.Array(Type.String(), { description: "Threads of the artifact you are acting on (at most 20); replaces the ones named before." })),
  message: opt(str("What you are doing, in a few words (at most 140 characters).")),
  done: opt(Type.Boolean({ description: "Clear it now (with `thread_ids`, only those threads)." })),
}, strict);
```

the method on `Tools`, after `watch`:

```ts
  async working(ctx: ExtensionContext, a: Static<typeof WorkingArgs>): Promise<Json> {
    const { id } = artifactRef(a.url_or_id);
    if (a.thread_ids) {
      if (a.thread_ids.length > 20) throw invalid("at most 20 thread_ids");
      for (const t of a.thread_ids) checkThreadId(t);
    }
    const c = this.clientFor(ctx);
    const url = this.artifactUrl(c, id);
    if (a.done) {
      const r = await this.call(() => c.clearWorking(id, a.thread_ids));
      return { artifact_id: id, url, working: r.working !== null, cleared: r.cleared };
    }
    const body: { thread_ids?: string[]; message?: string } = {};
    if (a.thread_ids) body.thread_ids = a.thread_ids;
    if (a.message !== undefined) body.message = a.message;
    const r = await this.call(() => c.setWorking(id, body));
    return {
      artifact_id: id, url, working: true, message: r.working.message, thread_ids: r.working.thread_ids,
      started_at: r.working.started_at, expires_in_s: 120, message_truncated: r.message_truncated,
    };
  }
```

and register it after `watch`:

```ts
    define("working", "Clax working",
      "Tell the person you are working on an artifact: its page's top bar shows `<harness> working on N` (or `<harness>: <message>`), its gallery card a chip, and `<harness> is working on it` on each thread in `thread_ids`. Comments sent to you mark you working automatically; call this for other work or to add a short `message` (at most 140 characters). It clears when you reply to those threads, publish the artifact, end your turn, or go 2 minutes without a tool call; `done: true` clears it now.",
      "Show the person you are working on a Clax artifact, or clear it",
      WorkingArgs, (ctx, a) => tools.working(ctx, a));
```

While in `plugins/pi/src/clax.ts`, correct the article in the existing one-line snippets: every `an Clax` becomes `a Clax` (`grep -n "an Clax" plugins/pi/src/clax.ts`). Do the same in `plugins/pi/src/daemon.ts:1` and in `docs/contract.md` (`an Clax extension` becomes `a Clax extension`). Update any test or fixture that pins those snippets (`grep -rn "an Clax" plugins docs`), so the grep prints nothing afterwards.

In `plugins/pi/test/clax.test.ts`, rename the test to `registers the twenty-three tools with one-line prompt snippets, and the clax command`, and add to `describe("comments", ...)`:

```ts
  it("clax_working marks the artifact and done clears it, as the MCP tool does", async () => {
    const { pi, ctx } = load(daemon.home, "pi-working");
    await pi.emit("session_start", { type: "session_start", reason: "startup" }, ctx);
    const pub = JSON.parse((await pi.callTool("clax_publish", { html: "<h2>Goals</h2>", title: "W" }, ctx)).content[0].text!);
    const set = await pi.callTool("clax_working", { url_or_id: pub.artifact_id, message: "Two columns" }, ctx);
    expect(set.isError).toBe(false);
    expect(JSON.parse(set.content[0].text!)).toMatchObject({ working: true, message: "Two columns", expires_in_s: 120, message_truncated: false });
    const seen = await (await fetch(`${daemon.base}/api/artifacts/${pub.artifact_id}/working`)).json();
    expect(seen.working[0]).toMatchObject({ harness: "pi", message: "Two columns" });
    expect(JSON.stringify(seen)).not.toContain("session_id");
    const done = await pi.callTool("clax_working", { url_or_id: pub.artifact_id, done: true }, ctx);
    expect(JSON.parse(done.content[0].text!)).toMatchObject({ working: false, cleared: true });
    const bad = await pi.callTool("clax_working", { url_or_id: pub.artifact_id, thread_ids: ["x"] }, ctx);
    expect(bad.isError).toBe(true);
    expect(JSON.parse(bad.content[0].text!).error.code).toBe("invalid_args");
  });
```

`daemon.base` is `TestDaemon.base` from `plugins/pi/test/daemon-fixture.ts`.

Run: `cd plugins/pi && npm run typecheck && npm test`
Expected: PASS.

- [ ] **Step 4: The tool lists, the skills and the plugin check**

- `scripts/test-plugins.sh`: change the comment to `twenty-three`, `len(tools) != 22` to `!= 23`, `not 22` to `not 23`, and the PASS text to `the twenty-three tool descriptions match in tools.rs and clax.ts`.
- `docs/contract.md`: `Twenty-two tools:` becomes `Twenty-three tools:`, and after `` `wait_for_feedback` `` insert `` `working` `` in that list. `the same twenty-two tools` becomes `the same twenty-three tools`.
- `README.md`, `plugins/claude-code/README.md`, `plugins/clax/README.md`: `twenty-two tools` becomes `twenty-three tools`, with `` `working` `` inserted after `` `wait_for_feedback` ``. `plugins/pi/README.md`: `Twenty-two tools:` becomes `Twenty-three tools:`, with `` `clax_working` `` inserted after `` `clax_wait_for_feedback` ``.
- In each of the three `SKILL.md` files, in `## Comment loop`, insert this before the paragraph that starts `When the person wants to iterate live`. The text is identical in all three files:

```markdown
Showing that you are working: when a comment reaches you, the person's page
already shows you as working on its thread, and it clears when you reply to
that thread, publish the artifact, or end your turn. Call `working` yourself
when you start on an artifact for another reason (the person asked in the
terminal, or you are acting on several threads at once) and when a short
`message` would help ("Rebuilding the chart", at most 140 characters). Reply
to each thread after publishing the change, so the version lists the thread
as addressed before your reply clears it. Call `working` with `done: true`
when you stop without replying or publishing.

- `working` (`url_or_id`; optional `thread_ids`, `message`, `done`):
  `working: true` with the stored `message`, `thread_ids` and `expires_in_s`,
  or `working: false` after `done`.
```

Then:

```bash
python3 scripts/sync-skill-tools.py
bash scripts/test-plugins.sh | tail -1
```

Expected: `plugin checks passed`.

- [ ] **Step 5: Gates and staging**

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add plugins/pi/test/fixtures/contract.json crates/clax-mcp/src/tools.rs crates/clax-mcp/src/client.rs crates/clax-mcp/tests/comments.rs \
  crates/clax-mcp/tests/shim.rs plugins/pi/src/clax.ts plugins/pi/src/client.ts plugins/pi/test/clax.test.ts scripts/test-plugins.sh \
  docs/contract.md README.md plugins/claude-code/README.md plugins/clax/README.md plugins/pi/README.md \
  plugins/claude-code/skills/clax/SKILL.md plugins/clax/skills/clax/SKILL.md plugins/pi/skills/clax/SKILL.md plugins/pi/src/daemon.ts
git status --short   # staged; this task continues below
```

#### B. Pi renews on every tool call and ends the turn on `agent_end`

**Files:**
- Modify: `plugins/pi/src/clax.ts`, `plugins/pi/test/clax.test.ts`

**Interfaces:**
- Consumes: `renewWorking`, `endWorking` (Task 10).
- Produces: `RENEW_EVERY_MS = 15_000` exported from `clax.ts` (the throttle), and handlers for `tool_call` and `agent_end` (Pi 0.73.1: `AgentEndEvent { type: "agent_end"; messages }`, `ToolCallEvent`).

- [ ] **Step 6: Failing tests**

Append to `describe("comments", ...)` in `plugins/pi/test/clax.test.ts`:

```ts
  it("tool calls renew working records at most every 15 s, and agent_end ends them", async () => {
    const { pi, ctx } = load(daemon.home, "pi-renew");
    await pi.emit("session_start", { type: "session_start", reason: "startup" }, ctx);
    const pub = JSON.parse((await pi.callTool("clax_publish", { html: "<p>r</p>", title: "R" }, ctx)).content[0].text!);
    await pi.callTool("clax_working", { url_or_id: pub.artifact_id, message: "m" }, ctx);
    const renews: number[] = [];
    const spy = vi.spyOn(DaemonClient.prototype, "renewWorking").mockImplementation(async function (this: DaemonClient) { renews.push(Date.now()); return { renewed: 1 }; });
    vi.useFakeTimers({ toFake: ["Date"] });
    try {
      await pi.emit("tool_call", { type: "tool_call", toolName: "bash", toolCallId: "1", input: {} }, ctx);
      await pi.emit("tool_call", { type: "tool_call", toolName: "edit", toolCallId: "2", input: {} }, ctx);
      expect(renews.length).toBe(1);
      vi.setSystemTime(Date.now() + RENEW_EVERY_MS);
      await pi.emit("tool_call", { type: "tool_call", toolName: "read", toolCallId: "3", input: {} }, ctx);
      expect(renews.length).toBe(2);
    } finally {
      vi.useRealTimers();
      spy.mockRestore();
    }
    await pi.emit("agent_end", { type: "agent_end", messages: [] }, ctx);
    const seen = await (await fetch(`${daemon.base}/api/artifacts/${pub.artifact_id}/working`)).json();
    expect(seen.working).toEqual([]);
  });
```

Add `RENEW_EVERY_MS` to the import from `../src/clax`, `DaemonClient` from `../src/client`, and `vi` from `vitest` where missing.

Run: `cd plugins/pi && npm test -- -t "renew working"`
Expected: FAIL.

- [ ] **Step 7: Implement**

In `claxExtension`, after the `session_shutdown` handler:

```ts
    // Working records (spec §10 "Working"): any tool call renews them, at most
    // every RENEW_EVERY_MS; the end of the agent loop ends the turn's records.
    // Neither starts a daemon, registers a session, or delays the tool.
    let lastRenew = 0;
    pi.on("tool_call", async () => {
      const c = tools.existingClient();
      if (!c?.session() || Date.now() - lastRenew < RENEW_EVERY_MS) return;
      lastRenew = Date.now();
      void c.renewWorking().catch(() => undefined);
    });
    pi.on("agent_end", async () => {
      const c = tools.existingClient();
      if (!c?.session()) return;
      lastRenew = 0;
      await c.endWorking().catch(() => undefined);
    });
```

and at module level, with the other constants:

```ts
/** The shortest gap between two renewals of the session's working records. */
export const RENEW_EVERY_MS = 15_000;
```

Run: `cd plugins/pi && npm run typecheck && npm test`
Expected: PASS.

- [ ] **Step 8: Gates and staging**

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add plugins/pi/src/clax.ts plugins/pi/test/clax.test.ts
git status --short   # staged; the controller commits ("Add the working tool to MCP and Pi, renew on Pi tool calls, and end the turn at agent_end")
```

---

### Task 11: Hooks end the turn and renew, and the comment-loop smoke shows it

Two parts: the hooks (A), then the smoke that shows working on delivery and clearing on reply (B).

#### A. The Stop hook ends the turn, and a `tool` hook renews

**Files:**
- Modify: `crates/clax-hooks/src/events.rs`, `crates/clax-cli/src/commands/hook.rs`, `crates/clax-hooks/tests/golden.rs`, `plugins/claude-code/hooks/hooks.json`, `plugins/clax/hooks/hooks.json`, `scripts/test-plugins.sh`, `scripts/quality_gates.sh`, `scripts/smoke-codex.sh`, `plugins/claude-code/README.md`, `plugins/clax/README.md`
- Create: `crates/clax-hooks/tests/fixtures/claude-post-tool-use.json`, `crates/clax-hooks/tests/fixtures/codex-post-tool-use.json`, `scripts/tool-hook.sh`, `plugins/claude-code/scripts/tool-hook.sh`, `plugins/clax/scripts/tool-hook.sh`, `scripts/test-tool-hook.sh`

**Interfaces:**
- Produces: `clax_hooks::events::tool(harness: &str, input: &HookInput, daemon: &dyn Daemon) -> anyhow::Result<HookOutput>` (always `HookOutput::none()`).
- Changes: `clax_hooks::events::stop` posts `/api/sessions/<sid>/working/end` when it allows the stop.
- Produces: `clax hook --agent <claude|codex> tool`, with a budget of 2 s for the whole run and 1 s per request.
- Produces: `scripts/tool-hook.sh <claude|codex>`, the throttled `PostToolUse` command (Step 4). The stamp is at `${CLAX_HOME:-$HOME/.clax}/run/tool-hook/<harness>-<session ID>`, and the script always exits 0.

- [ ] **Step 1: Failing unit tests**

In `crates/clax-hooks/src/events.rs` tests, make `FeedbackFake` record posts: add `posts: RefCell<Vec<String>>` to the struct and to `fake()`, and have `post` push `path.to_string()` before answering. Then add:

```rust
    #[test]
    fn stop_ends_the_turn_only_when_it_allows_the_stop() {
        let d = fake(Some("[clax] 1 comment sent to you:\nX"));
        stop("claude", &HookInput::parse(r#"{"session_id":"s1"}"#), &d).unwrap();
        assert!(d.posts.borrow().is_empty(), "a blocked stop continues the turn");
        let d = fake(None);
        assert_eq!(stop("claude", &HookInput::parse(r#"{"session_id":"s1","stop_hook_active":true}"#), &d).unwrap(), HookOutput::none());
        assert_eq!(*d.posts.borrow(), ["/api/sessions/S/working/end"]);
    }

    #[test]
    fn tool_renews_and_prints_nothing() {
        let d = fake(None);
        assert_eq!(tool("claude", &HookInput::parse(r#"{"session_id":"s1"}"#), &d).unwrap(), HookOutput::none());
        assert_eq!(*d.posts.borrow(), ["/api/sessions/S/working/renew"]);
        let d = fake(None);
        assert_eq!(tool("claude", &HookInput::parse(r#"{"session_id":"other"}"#), &d).unwrap(), HookOutput::none());
        assert!(d.posts.borrow().is_empty(), "no live session: nothing to renew");
    }
```

Run: `cargo test -p clax-hooks --lib`
Expected: FAIL to compile (`tool` not found).

- [ ] **Step 2: Implement**

In `stop`, replace the final `Ok(match ... )` with:

```rust
    match feedback_text(daemon, &sid, &format!("tier=stop_hook&resends={resends}"))? {
        Some(text) => Ok(HookOutput::block(&text)),
        None => {
            // The turn really ends: its working records end with it.
            daemon.post(&format!("/api/sessions/{sid}/working/end"), &json!({}))?;
            Ok(HookOutput::none())
        }
    }
```

and extend its doc comment: ``When it allows the stop, it ends the session's working records (the turn is over).`` Add:

```rust
/// `PostToolUse`: renews the session's working records. Prints nothing.
///
/// # Errors
/// When the input has no `session_id` or a daemon request fails.
pub fn tool(harness: &str, input: &HookInput, daemon: &dyn Daemon) -> anyhow::Result<HookOutput> {
    if let Some(sid) = live_session(harness, input, daemon)? {
        daemon.post(&format!("/api/sessions/{sid}/working/renew"), &json!({}))?;
    }
    Ok(HookOutput::none())
}
```

In `crates/clax-cli/src/commands/hook.rs`: add `const TOOL_DEADLINE: Duration = Duration::from_secs(2);` and `const TOOL_REQUEST_TIMEOUT: Duration = Duration::from_secs(1);` with doc comments. Add the variant `/// A tool call finished; renew the session's working records.\n    Tool,` to `Event`, `Event::Tool => (TOOL_DEADLINE, TOOL_REQUEST_TIMEOUT)` to `budget`, `Event::Tool => "tool"` to `name`, and `Event::Tool => events::tool(agent.harness(), &input, &client),` to `handle`.

Run: `cargo test -p clax-hooks --lib`
Expected: PASS.

- [ ] **Step 3: Golden tests against a real daemon**

`crates/clax-hooks/tests/fixtures/claude-post-tool-use.json`:

```json
{"session_id":"cc-hook-1","transcript_path":"/tmp/t.jsonl","cwd":"/tmp/project","permission_mode":"default","hook_event_name":"PostToolUse","tool_name":"Edit","tool_input":{"file_path":"/tmp/project/a.txt"},"tool_response":{"success":true},"tool_use_id":"toolu_1"}
```

`crates/clax-hooks/tests/fixtures/codex-post-tool-use.json`:

```json
{"session_id":"cx-hook-1","transcript_path":null,"cwd":"/tmp/project","hook_event_name":"PostToolUse","model":"gpt-5","tool_name":"shell","tool_input":{"command":["ls"]},"tool_response":"ok"}
```

Add `"tool"` to the event lists in `unusable_stdin_prints_nothing` and `no_daemon_prints_nothing_and_starts_none`. Append:

```rust
impl Daemon {
    fn working(&self, aid: &str) -> Vec<Value> {
        let v: Value = self.http().get(format!("{}/api/artifacts/{aid}/working", self.base())).send().unwrap().json().unwrap();
        v["working"].as_array().unwrap().clone()
    }
    fn skew(&self, secs: i64) {
        let res = self.http().post(format!("{}/api/_test/working/skew", self.base()))
            .bearer_auth(self.token()).json(&serde_json::json!({"secs": secs})).send().unwrap();
        assert_eq!(res.status(), 200);
    }
}

fn working_through_a_turn(agent: &str, hsid: &str) {
    let d = Daemon::start();
    let (_sid, aid) = d.session_with_artifact(agent, hsid);
    d.sent_thread(&aid, "tighten the spacing");
    let mut stop_in: Value = serde_json::from_slice(&fixture(&format!("{agent}-stop.json"))).unwrap();
    stop_in["session_id"] = hsid.into();
    let r = hook(&d.home(), agent, "stop", stop_in.to_string().as_bytes());
    assert_eq!(one_line_json(&r.stdout)["decision"], "block");
    assert_eq!(d.working(&aid).len(), 1, "the blocked stop handed the comment over and marked the session");
    d.skew(100);
    let mut tool_in: Value = serde_json::from_slice(&fixture(&format!("{agent}-post-tool-use.json"))).unwrap();
    tool_in["session_id"] = hsid.into();
    let r = hook(&d.home(), agent, "tool", tool_in.to_string().as_bytes());
    assert_eq!((r.code, r.stdout.as_str()), (Some(0), ""));
    d.skew(100);
    assert_eq!(d.working(&aid).len(), 1, "the tool hook renewed at 100 s");
    stop_in["stop_hook_active"] = true.into();
    let r = hook(&d.home(), agent, "stop", stop_in.to_string().as_bytes());
    assert_eq!(r.stdout, "");
    assert!(d.working(&aid).is_empty(), "the turn ended");
}

#[test]
fn claude_working_through_a_turn() {
    working_through_a_turn("claude", "cc-work-1");
}

#[test]
fn codex_working_through_a_turn() {
    working_through_a_turn("codex", "cx-work-1");
}
```

`session_with_artifact` registers with `harness_session_id` = `hsid`, so the hooks find the session by the fixture's re-keyed `session_id`.

Run: `cargo test -p clax-hooks --test golden`
Expected: PASS.

- [ ] **Step 4: The throttle gate, test first**

The person's ruling: the `PostToolUse` hook runs a tiny shell check first and starts `clax` only when this session's last renewal is at least 60 s old. The gate is one POSIX `sh` script, `scripts/tool-hook.sh`, copied byte for byte into both plugins (as `ensure-clax.sh` is).

- **Where the stamp lives:** `${CLAX_HOME:-$HOME/.clax}/run/tool-hook/<harness>-<session ID>`, one file per harness session. The session ID comes from the hook input's `session_id`, and only the characters `A-Z a-z 0-9 . _ -` are accepted, so a hostile value cannot leave that directory. It never lives under `~/.claude` or `~/.codex`. The `session-end` hook deletes it.
- **Portability:** the gate compares the stamp's modification time with the clock using `date -r FILE +%s` (a file's mtime in epoch seconds) and `date +%s`. BSD date (macOS), GNU coreutils and BusyBox all support both. It does not use `stat`, whose flags differ (`-f %m` against `-c %Y`), or `find -mmin`, which rounds the age up to whole minutes on BSD and not on GNU. A stamp that is missing, unreadable or dated in the future counts as old.
- **Cost of a skipped call:** one `sh`, one `cat` and two `date` runs. No `clax`, no network.
- **Always exits 0 and prints nothing.** When the gate runs `clax`, its output goes to `/dev/null` and its status is ignored: `clax hook` logs its own failures to `hooks.log`.

`scripts/test-tool-hook.sh`:

```bash
#!/usr/bin/env bash
# Tests the PostToolUse gate (scripts/tool-hook.sh) against a fake
# ensure-clax.sh that records each run. Uses a scratch HOME and CLAX_HOME.
set -uo pipefail
cd "$(dirname "$0")/.."
FAILED=0
fail() { echo "FAIL: $1"; FAILED=1; }
pass() { echo "PASS: $1"; }
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
mkdir -p "$T/bin" "$T/fakehome"
cp scripts/tool-hook.sh "$T/bin/tool-hook.sh"
cat > "$T/bin/ensure-clax.sh" <<'SH'
#!/bin/sh
{ printf '%s|' "$*"; cat; echo; } >> "$CALLS"
echo '{"noise": true}'
echo 'noise' >&2
exit 3
SH
chmod +x "$T/bin/tool-hook.sh" "$T/bin/ensure-clax.sh"
export HOME="$T/fakehome" CLAX_HOME="$T/home" CALLS="$T/calls"
STAMPS="$CLAX_HOME/run/tool-hook"
calls() { if [ -f "$CALLS" ]; then wc -l < "$CALLS" | tr -d ' '; else echo 0; fi; }
gate() { printf '%s' "$2" | "$T/bin/tool-hook.sh" "$1" 2>"$T/err"; }
age() { python3 -c 'import os, sys, time; t = time.time() + float(sys.argv[2]); os.utime(sys.argv[1], (t, t))' "$1" "$2"; }
IN='{"session_id":"cc-1","hook_event_name":"PostToolUse","tool_name":"Edit"}'

out="$(gate claude "$IN")"; code=$?
if [ "$code" = 0 ] && [ -z "$out" ] && [ ! -s "$T/err" ]; then pass "exits 0 and prints nothing, even when clax fails and writes output"
else fail "exit $code, stdout '$out', stderr '$(cat "$T/err")'"; fi
if [ "$(calls)" = 1 ] && grep -qF "exec hook --agent claude tool|$IN" "$CALLS"; then pass "the first call runs clax hook ... tool with the hook input"
else fail "first call: $(cat "$CALLS" 2>/dev/null)"; fi
if [ -f "$STAMPS/claude-cc-1" ]; then pass "the stamp is under CLAX_HOME/run/tool-hook"; else fail "no stamp at $STAMPS/claude-cc-1"; fi
if [ -z "$(ls -A "$HOME")" ]; then pass "nothing is written under HOME (no ~/.claude, no ~/.codex)"; else fail "HOME holds: $(ls -A "$HOME")"; fi

gate claude "$IN" >/dev/null
if [ "$(calls)" = 1 ]; then pass "a call within 60 s starts no clax"; else fail "throttle: $(calls) runs"; fi
age "$STAMPS/claude-cc-1" -59
gate claude "$IN" >/dev/null
if [ "$(calls)" = 1 ]; then pass "a stamp 59 s old still skips"; else fail "59 s: $(calls) runs"; fi
age "$STAMPS/claude-cc-1" -61
gate claude "$IN" >/dev/null
if [ "$(calls)" = 2 ]; then pass "a stamp 61 s old renews again"; else fail "61 s: $(calls) runs"; fi
age "$STAMPS/claude-cc-1" 3600
gate claude "$IN" >/dev/null
if [ "$(calls)" = 3 ]; then pass "a stamp dated in the future counts as old"; else fail "future: $(calls) runs"; fi

gate claude '{"session_id": "cc-2"}' >/dev/null
gate codex '{"session_id":"cc-1"}' >/dev/null
if [ "$(calls)" = 5 ] && [ -f "$STAMPS/claude-cc-2" ] && [ -f "$STAMPS/codex-cc-1" ]; then pass "stamps are per harness and per session (spaced JSON too)"
else fail "per session: $(calls) runs, $(ls "$STAMPS")"; fi

gate claude '{"hook_event_name":"PostToolUse"}' >/dev/null
gate claude '{"hook_event_name":"PostToolUse"}' >/dev/null
gate claude '{"session_id":"../../escape"}' >/dev/null
if [ "$(calls)" = 8 ] && [ "$(ls "$STAMPS" | sort | tr '\n' ' ')" = "claude-cc-1 claude-cc-2 codex-cc-1 " ] && [ ! -e "$CLAX_HOME/escape" ]; then
  pass "without a usable session_id every call runs clax and no stamp is written"
else fail "no session: $(calls) runs, $(ls -R "$CLAX_HOME")"; fi

out="$(gate nope "$IN")"; code=$?
if [ "$code" = 0 ] && [ -z "$out" ] && [ "$(calls)" = 8 ]; then pass "an unknown harness does nothing and exits 0"; else fail "unknown harness: $code '$out' $(calls)"; fi

chmod 000 "$STAMPS"
out="$(gate claude '{"session_id":"cc-3"}')"; code=$?
chmod 755 "$STAMPS"
if [ "$code" = 0 ] && [ -z "$out" ] && [ "$(calls)" = 9 ]; then pass "an unwritable stamp directory still renews and exits 0"; else fail "unwritable: $code '$out' $(calls)"; fi

if [ "$FAILED" -ne 0 ]; then echo "tool hook gate checks failed"; exit 1; fi
echo "tool hook gate checks passed"
```

Run: `chmod +x scripts/test-tool-hook.sh && scripts/test-tool-hook.sh`
Expected: FAIL (`scripts/tool-hook.sh` does not exist).

`scripts/tool-hook.sh`:

```sh
#!/bin/sh
# PostToolUse gate for `clax hook --agent <harness> tool` (spec §13).
# Working records lapse 120 s after their last renewal, so renewing once a
# minute is enough: this starts clax only when this session's stamp file is
# at least 60 s old, and otherwise exits after a cat and two date calls.
# It always exits 0 and prints nothing, so it can never fail or steer the
# harness; clax logs its own failures to hooks.log.
#
# Usage: tool-hook.sh <claude|codex>     (the hook's JSON on stdin)
# Stamp: ${CLAX_HOME:-$HOME/.clax}/run/tool-hook/<harness>-<session ID>
# Portable: `date +%s` and `date -r FILE +%s` (a file's mtime) behave the
# same in BSD date (macOS), GNU coreutils and BusyBox; `stat` and
# `find -mmin` do not.
agent="$1"
case "$agent" in claude|codex) ;; *) exit 0 ;; esac
input="$(cat)"
sid=""
for pat in '"session_id":"' '"session_id": "'; do
  rest="${input#*"$pat"}"
  if [ "$rest" != "$input" ]; then
    sid="${rest%%\"*}"
    break
  fi
done
case "$sid" in ''|*[!A-Za-z0-9._-]*) sid="" ;; esac
if [ -n "$sid" ]; then
  dir="${CLAX_HOME:-$HOME/.clax}/run/tool-hook"
  stamp="$dir/$agent-$sid"
  now="$(date +%s)"
  last="$(date -r "$stamp" +%s 2>/dev/null)" || last=0
  age=$((now - last))
  if [ "$age" -ge 0 ] && [ "$age" -lt 60 ]; then
    exit 0
  fi
  { mkdir -p "$dir" && : > "$stamp"; } 2>/dev/null
fi
printf '%s' "$input" | bash "${0%/*}/ensure-clax.sh" exec hook --agent "$agent" tool >/dev/null 2>&1
exit 0
```

Then:

```bash
chmod +x scripts/tool-hook.sh
cp scripts/tool-hook.sh plugins/claude-code/scripts/tool-hook.sh
cp scripts/tool-hook.sh plugins/clax/scripts/tool-hook.sh
scripts/test-tool-hook.sh
```

Expected: every line `PASS`, and the last line `tool hook gate checks passed`.

In `scripts/quality_gates.sh`, after the `run "release installer"` line, add `run "tool hook gate"          scripts/test-tool-hook.sh`.

The stamp is touched before `clax` runs. Concurrent tool calls within the minute then start at most one `clax`, and a failed renewal is retried a minute later. The record's 120 s lifetime covers that gap.

- [ ] **Step 5: `session-end` removes the stamp**

In `crates/clax-cli/src/commands/hook.rs`, `handle`, change the `Event::SessionEnd` arm to:

```rust
        Event::SessionEnd => {
            let out = events::session_end(agent.harness(), &input, &client);
            if let Some(sid) = input.session_id.as_deref().filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))) {
                let _ = std::fs::remove_file(home.root().join("run/tool-hook").join(format!("{}-{sid}", agent.harness())));
            }
            out
        }
```

In `crates/clax-hooks/tests/golden.rs` `lifecycle`, before running `session-end`, create `d.home().join("run/tool-hook").join(format!("{agent}-{harness_session_id}"))` (with `create_dir_all` on its parent). After the hook, assert that it no longer exists.

- [ ] **Step 6: Wire the hooks**

`plugins/claude-code/hooks/hooks.json`, add after `Stop`:

```json
    "PostToolUse": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "\"${CLAUDE_PLUGIN_ROOT}/scripts/tool-hook.sh\" claude",
            "timeout": 5
          }
        ]
      }
    ],
```

`plugins/clax/hooks/hooks.json`, add after `Stop`:

```json
    "PostToolUse": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "bash \"${PLUGIN_ROOT}/scripts/tool-hook.sh\" codex",
            "timeout": 5
          }
        ]
      }
    ],
```

`scripts/test-plugins.sh`:
- In the installer-copy loop, add a loop that checks each plugin's `scripts/tool-hook.sh` is byte-identical to `scripts/tool-hook.sh` (`cmp -s`) and executable, with PASS/FAIL lines worded like the `ensure-clax.sh` ones.
- In the "Stop and prompt hooks are wired" check, add:

```python
ok = ok and [h["command"] for h in cmds(claude, "PostToolUse")] == ['"${CLAUDE_PLUGIN_ROOT}/scripts/tool-hook.sh" claude'] and all(h["timeout"] == 5 for h in cmds(claude, "PostToolUse"))
ok = ok and [h["command"] for h in cmds(codex, "PostToolUse")] == ['bash "${PLUGIN_ROOT}/scripts/tool-hook.sh" codex'] and all(h["timeout"] == 5 for h in cmds(codex, "PostToolUse"))
```

  and rename its PASS text to `Stop, prompt and PostToolUse hooks are wired`. The existing Codex check (`SessionStart`, `SessionEnd`, `Stop` run `exec hook --agent codex`) and the Claude quoting check keep their event lists: the `PostToolUse` command is the gate, checked above.

`plugins/claude-code/README.md`: after the `Stop` item, add

```markdown
  - `PostToolUse` (`scripts/tool-hook.sh claude`, 5 s): keeps the page's
    "working" status alive while the agent uses tools. A shell check skips
    starting `clax` unless this session's stamp file
    (`~/.clax/run/tool-hook/`) is at least a minute old, so most tool calls
    cost only a few small shell commands. It always exits 0.
```

and append to the `Stop` item: ``When nothing blocks, the turn is over: the page stops showing the agent as working.``

`plugins/clax/README.md`: in the Hooks paragraph, after `hands over comments sent to the session at the end of a turn`, insert ``, and when nothing is waiting ends the page's "working" status; `PostToolUse` (`scripts/tool-hook.sh codex`, 5 s) keeps that status alive, starting `clax` at most once a minute (not yet measured on Codex; see `docs/contract.md`)``.

- [ ] **Step 7: The manual Codex check (edit only; not run by this task)**

In `scripts/smoke-codex.sh`, inside the `--hooks` branch of the session check (after the `with_id` check), add to the Python block:

```python
if hooks:
    log = open(os.path.join(os.environ["CLAX_HOME"], "logs", "hooks.log")).read()
    ran = "agent=codex event=tool" in log  # the gate lets the first call of a session through
    print("smoke: " + ("the PostToolUse hook ran (renewal on every tool call works on this Codex)"
          if ran else "the PostToolUse hook did not run: Codex renews working records only on clax tool calls and the Stop hook"))
```

This prints the finding and fails nothing. The person runs `scripts/smoke-codex.sh --hooks` (it reads `~/.codex/auth.json`, which this plan must not), and the controller records the result in `docs/contract.md` ("Working" table, Codex row). Until then the contract keeps "not yet measured".

- [ ] **Step 8: Gates and staging**

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add crates/clax-hooks/src/events.rs crates/clax-cli/src/commands/hook.rs crates/clax-hooks/tests/golden.rs \
  crates/clax-hooks/tests/fixtures/claude-post-tool-use.json crates/clax-hooks/tests/fixtures/codex-post-tool-use.json \
  plugins/claude-code/hooks/hooks.json plugins/clax/hooks/hooks.json scripts/test-plugins.sh scripts/smoke-codex.sh \
  plugins/claude-code/README.md plugins/clax/README.md scripts/quality_gates.sh scripts/tool-hook.sh \
  plugins/claude-code/scripts/tool-hook.sh plugins/clax/scripts/tool-hook.sh scripts/test-tool-hook.sh
git status --short   # staged; this task continues below
```

#### B. The comment-loop smoke shows working on delivery and clearing on reply

**Files:**
- Modify: `scripts/smoke-comment-loop.sh`

- [ ] **Step 9: Assert the working state around tiers 1, 2 and the reply**

In the Python block, add a helper after `http(...)`:

```python
def working(aid):
    return http("GET", f"/api/artifacts/{aid}/working")["working"]
```

After step 3's `ok("tier 1: delivered once")`, add:

```python
w = working(aid)
if len(w) != 1 or w[0]["harness"] != "claude" or w[0]["thread_ids"] != [t1["id"]] or "session_id" in w[0]:
    fail(f"working after tier 1 delivery: {w}")
ok(f"working: a record naming the thread appeared when the comment was delivered (key {w[0]['key']})")
```

After step 4's `ok(f"tier 2: the Stop hook blocked with thread ...")` line, the `stop(True)` call has already allowed the stop, so add:

```python
if working(aid):
    fail(f"working after the Stop hook allowed the stop: {working(aid)}")
ok("working: cleared when the Stop hook allowed the stop (the turn ended)")
```

In step 5, after the `wait_for_feedback` assertion `ok(...)`, add:

```python
w = working(aid)
if [x["thread_ids"] for x in w] != [[sent_at["thread"]]]:
    fail(f"working after wait_for_feedback: {w}")
ok("working: wait_for_feedback returning a comment marked its thread")
```

In step 6, before `reply, _ = shim.call("comments_reply", ...)`, re-mark t1 explicitly and check the reply clears it:

```python
set_, _ = shim.call("working", {"url_or_id": aid, "thread_ids": [t1["id"]], "message": "Two columns"})
if not set_["working"] or set_["message"] != "Two columns":
    fail(f"working tool: {set_}")
ok("working tool: the top bar now reads 'claude: Two columns'")
```

and after `ok(f"agent reply shown as ...")`:

```python
if any(t1["id"] in x["thread_ids"] for x in working(aid)):
    fail(f"working still names thread 1 after the reply: {working(aid)}")
ok("working: the agent's reply to thread 1 took it out of the working record")
```

- [ ] **Step 10: Run it**

Run: `scripts/smoke-comment-loop.sh`
Expected: every step prints `PASS`, including the five new `working` lines, and the last line is `comment loop smoke passed`.

- [ ] **Step 11: Gates and staging**

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add scripts/smoke-comment-loop.sh
git status --short   # staged; the controller commits ("End working records when the Stop hook allows the stop, renew them after tool calls at most once a minute, and show both in the smoke")
```

---

### Task 12: Changelog storage: notes, links and version seen marks

**Files:**
- Create: `crates/clax-core/src/changelog.rs`, `crates/clax-core/src/store/changelog.rs`
- Modify: `crates/clax-core/src/lib.rs`, `crates/clax-core/src/store/mod.rs`, `crates/clax-core/src/store/migrations.rs`, `crates/clax-core/src/model.rs` (`Version`), `crates/clax-core/src/publish.rs`, `crates/clax-core/src/store/artifacts.rs`, `crates/clax-core/src/store/threads.rs` (`delete_thread_touched`), `crates/clax-core/src/working.rs` (`clean_line`)

**Interfaces:**
- Produces: `clax_core::working::clean_line(raw: &str, max: usize) -> (Option<String>, bool)`; `clean_message(raw)` becomes `clean_line(raw, MAX_MESSAGE_CHARS)`.
- Produces: `clax_core::changelog::{MAX_NOTE_CHARS: usize = 280, MAX_ADDRESSES: usize = 50, MAX_SEEN_PER_VIEWER: usize = 200, LinkSource { Working, Explicit, Resolve } (as_str: "working" | "explicit" | "resolve")}`.
- Changes: `PublishRequest` gains `note: Option<String>` and `addresses: Option<Vec<String>>`. `ValidatedPublish` gains `note: Option<String>`, `note_truncated: bool`, `addresses: Vec<String>`, and `working_threads: Vec<String>` (empty from `validate`; the publish route fills it). `Version` gains `note: Option<String>` and `addresses: Vec<String>` (`#[serde(default)]`).
- Produces (`Store`): `addressed_in(thread_id: &str) -> Result<Vec<u32>>`, `link_on_resolve(thread_id: &str) -> Result<Option<u32>>`, `seen(viewer_id: &str, aid: &ArtifactId) -> Result<Option<u32>>`, `mark_seen(viewer_id: &str, aid: &ArtifactId, n: u32) -> Result<u32>`.
- `publish_version` and `create_artifact` store the note and the links in the version's transaction. An explicit address that is not a thread of the artifact fails the publish with `unknown_thread`. A `working_threads` entry that no longer exists is skipped.

- [ ] **Step 1: Failing tests**

`crates/clax-core/src/store/changelog.rs`, test module first:

```rust
#[cfg(test)]
mod tests {
    use crate::publish::{PublishRequest, validate};
    use crate::store::test_util::{anchor, artifact, store};
    use crate::{ArtifactId, NewThread, Store};

    fn thread(st: &Store, id: &ArtifactId) -> String {
        st.create_thread(id, NewThread { version_n: 1, anchor: anchor(), body: "@agent x".into(), author_name: "Alex".into(), clip: None, via_page: false })
            .unwrap()
            .id
    }

    fn v2(st: &Store, id: &ArtifactId, note: Option<&str>, addresses: &[&str], working: &[String]) -> crate::Result<crate::model::Version> {
        let req: PublishRequest = serde_json::from_value(serde_json::json!({
            "if_version": 1, "note": note, "addresses": addresses,
            "files": {"index.html": {"content": "<main><h2>Quarterly goals</h2></main>", "encoding": "utf8"}}
        }))
        .unwrap();
        let mut p = validate(req)?;
        p.working_threads = working.to_vec();
        st.publish_version(id, p, None).map(|(_, v)| v)
    }

    #[test]
    fn a_publish_stores_its_note_and_links_explicit_and_working_threads_once() {
        let (_d, st) = store();
        let id = artifact(&st, None);
        let (t1, t2) = (thread(&st, &id), thread(&st, &id));
        let v = v2(&st, &id, Some("  Two\ncolumns  "), &[&t1], &[t1.clone(), t2.clone()]).unwrap();
        assert_eq!(v.note.as_deref(), Some("Two columns"));
        assert_eq!(v.addresses, [t1.clone(), t2.clone()]);
        assert_eq!(st.list_versions(&id).unwrap()[1].addresses, [t1.clone(), t2.clone()]);
        assert_eq!(st.addressed_in(&t1).unwrap(), [2]);
        assert_eq!(st.get_thread(&t1).unwrap().unwrap().status, "open", "linking never resolves");
    }

    #[test]
    fn an_unknown_address_fails_the_publish_and_a_vanished_working_thread_is_skipped() {
        let (_d, st) = store();
        let id = artifact(&st, None);
        let e = v2(&st, &id, None, &[&crate::new_ulid()], &[]).unwrap_err();
        assert!(matches!(e, crate::CoreError::Invalid { code, .. } if code == "unknown_thread"), "{e:?}");
        assert_eq!(st.get_artifact(&id).unwrap().unwrap().current_version, 1);
        let v = v2(&st, &id, None, &[], &[crate::new_ulid()]).unwrap();
        assert!(v.addresses.is_empty());
    }

    #[test]
    fn a_long_note_is_cut_and_flagged_and_too_many_addresses_are_refused() {
        let req: PublishRequest = serde_json::from_value(serde_json::json!({
            "note": "n".repeat(400), "files": {"index.html": {"content": "<p>", "encoding": "utf8"}}
        })).unwrap();
        let p = validate(req).unwrap();
        assert!(p.note_truncated);
        assert_eq!(p.note.unwrap().chars().count(), 280);
        let many: Vec<String> = (0..51).map(|_| crate::new_ulid()).collect();
        let req: PublishRequest = serde_json::from_value(serde_json::json!({
            "addresses": many, "files": {"index.html": {"content": "<p>", "encoding": "utf8"}}
        })).unwrap();
        assert!(validate(req).is_err());
    }

    #[test]
    fn an_agent_resolve_links_only_a_thread_with_no_link() {
        let (_d, st) = store();
        let id = artifact(&st, None);
        let (t1, t2) = (thread(&st, &id), thread(&st, &id));
        v2(&st, &id, None, &[&t1], &[]).unwrap();
        assert_eq!(st.link_on_resolve(&t1).unwrap(), None);
        assert_eq!(st.link_on_resolve(&t2).unwrap(), Some(2));
        assert_eq!(st.addressed_in(&t2).unwrap(), [2]);
    }

    #[test]
    fn deleting_a_thread_deletes_its_links() {
        let (_d, st) = store();
        let id = artifact(&st, None);
        let t1 = thread(&st, &id);
        v2(&st, &id, None, &[&t1], &[]).unwrap();
        st.delete_thread(&t1).unwrap();
        assert!(st.list_versions(&id).unwrap()[1].addresses.is_empty());
    }

    #[test]
    fn seen_marks_only_move_forward_and_are_bounded_per_viewer() {
        let (_d, st) = store();
        let viewer = st.upsert_viewer(&crate::new_ulid(), None).unwrap().id;
        let id = artifact(&st, None);
        assert_eq!(st.seen(&viewer, &id).unwrap(), None);
        assert_eq!(st.mark_seen(&viewer, &id, 3).unwrap(), 3);
        assert_eq!(st.mark_seen(&viewer, &id, 2).unwrap(), 3);
        let ids: Vec<ArtifactId> = (0..crate::changelog::MAX_SEEN_PER_VIEWER).map(|_| artifact(&st, None)).collect();
        for a in &ids {
            st.mark_seen(&viewer, a, 1).unwrap();
        }
        assert_eq!(st.seen(&viewer, &id).unwrap(), None, "the least recently updated row was pruned");
        assert_eq!(st.seen(&viewer, &ids[0]).unwrap(), Some(1));
        }
}
```

A viewer's ID is its cookie value; `upsert_viewer` creates the row the seen mark refers to.

Register the module: `pub mod changelog;` in `store/mod.rs`; `pub mod changelog;` in `lib.rs` for `crates/clax-core/src/changelog.rs`.

Run: `cargo test -p clax-core changelog`
Expected: FAIL to compile.

- [ ] **Step 2: Migration 10**

Append to `MIGRATIONS` in `store/migrations.rs`:

```rust
    // 10: the version changelog: each version's note, the threads it
    // addressed, and the latest version each viewer has viewed. No
    // foreign key to `artifacts`: the doctor's hard deletes of broken artifact
    // rows must not trip on them; artifact deletion removes them explicitly.
    "ALTER TABLE versions ADD COLUMN note TEXT;
    CREATE TABLE version_threads (
        artifact_id TEXT NOT NULL,
        version_n INTEGER NOT NULL,
        thread_id TEXT NOT NULL REFERENCES threads(id),
        source TEXT NOT NULL CHECK (source IN ('working', 'explicit', 'resolve')),
        created_at TEXT NOT NULL,
        PRIMARY KEY (artifact_id, version_n, thread_id)
    );
    CREATE INDEX version_threads_by_thread ON version_threads(thread_id);
    CREATE TABLE viewer_seen (
        viewer_id TEXT NOT NULL REFERENCES viewers(id),
        artifact_id TEXT NOT NULL,
        seen_n INTEGER NOT NULL,
        updated_at TEXT NOT NULL,
        PRIMARY KEY (viewer_id, artifact_id)
    );
    CREATE INDEX viewer_seen_by_age ON viewer_seen(viewer_id, updated_at);",
```

- [ ] **Step 3: Implement**

`crates/clax-core/src/working.rs`: rename the body of `clean_message` into `pub fn clean_line(raw: &str, max: usize) -> (Option<String>, bool)`, with `MAX_MESSAGE_CHARS` replaced by `max`. Keep `pub fn clean_message(raw: &str) -> (Option<String>, bool) { clean_line(raw, MAX_MESSAGE_CHARS) }`.

`crates/clax-core/src/changelog.rs`:

```rust
//! The version changelog (spec §10 "Version changelog"): bounds and link sources.

/// Longest version note, in characters, after [`crate::working::clean_line`].
pub const MAX_NOTE_CHARS: usize = 280;
/// Most threads one publish may name in `addresses`.
pub const MAX_ADDRESSES: usize = 50;
/// Most artifacts a viewer's seen marks are kept for.
pub const MAX_SEEN_PER_VIEWER: usize = 200;

/// Why a thread is linked to a version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkSource {
    /// The publishing session was marked working on it.
    Working,
    /// Named in `addresses`.
    Explicit,
    /// Resolved by an agent with no earlier link.
    Resolve,
}

impl LinkSource {
    pub fn as_str(self) -> &'static str {
        match self {
            LinkSource::Working => "working",
            LinkSource::Explicit => "explicit",
            LinkSource::Resolve => "resolve",
        }
    }
}
```

`publish.rs`: add to `PublishRequest` `#[serde(default)] pub note: Option<String>,` and `#[serde(default)] pub addresses: Option<Vec<String>>,`. Add to `ValidatedPublish` `pub note: Option<String>, pub note_truncated: bool, pub addresses: Vec<String>, pub working_threads: Vec<String>,`. In `validate`, before the files loop:

```rust
    let (note, note_truncated) = match req.note.as_deref() {
        Some(n) => crate::working::clean_line(n, crate::changelog::MAX_NOTE_CHARS),
        None => (None, false),
    };
    let addresses = req.addresses.clone().unwrap_or_default();
    if addresses.len() > crate::changelog::MAX_ADDRESSES {
        return Err(CoreError::invalid("invalid_args", format!("at most {} addresses", crate::changelog::MAX_ADDRESSES)));
    }
    if let Some(bad) = addresses.iter().find(|t| !crate::is_ulid(t)) {
        return Err(CoreError::invalid("invalid_args", format!("'{bad}' is not a thread ID")));
    }
```

and fill the new fields in the returned struct (`working_threads: Vec::new()`).

`model.rs`, `Version`: add `pub note: Option<String>,` and `#[serde(default)] pub addresses: Vec<String>,`. In `store/artifacts.rs`, `row_to_version` reads `note: r.get("note")?` and sets `addresses: Vec::new()`. `SELECT_VERSION` gains `note`. The versions `INSERT` in both `create_artifact` and `publish_version` gains the `note` column (`p.note`). After the insert, in the same transaction, call the new `link_version(tx, id.as_str(), n, &p)?`. `get_version` and `list_versions` fill `addresses` with `fill_addresses(c, id, &mut versions)`.

`store/changelog.rs`, above the tests:

```rust
//! Version notes' links to threads and viewers' seen marks (spec §5).

use super::Store;
use crate::changelog::{LinkSource, MAX_SEEN_PER_VIEWER};
use crate::model::Version;
use crate::publish::ValidatedPublish;
use crate::{ArtifactId, CoreError, Result};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

fn insert_link(tx: &Transaction<'_>, aid: &str, n: u32, tid: &str, source: LinkSource) -> Result<()> {
    tx.execute(
        "INSERT OR IGNORE INTO version_threads (artifact_id, version_n, thread_id, source, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![aid, n, tid, source.as_str(), Store::now()],
    )?;
    Ok(())
}

fn thread_on(tx: &Transaction<'_>, aid: &str, tid: &str) -> Result<bool> {
    Ok(tx
        .query_row("SELECT 1 FROM threads WHERE id = ?1 AND artifact_id = ?2", params![tid, aid], |_| Ok(()))
        .optional()?
        .is_some())
}

/// Links version `n` to `p.addresses` (each must be a thread of `aid`, else
/// `unknown_thread`) and then to `p.working_threads` (skipping any that no
/// longer exist). Runs inside the version's transaction.
pub(crate) fn link_version(tx: &Transaction<'_>, aid: &str, n: u32, p: &ValidatedPublish) -> Result<()> {
    for tid in &p.addresses {
        if !thread_on(tx, aid, tid)? {
            return Err(CoreError::invalid("unknown_thread", format!("{tid} is not a thread of {aid}")));
        }
        insert_link(tx, aid, n, tid, LinkSource::Explicit)?;
    }
    for tid in &p.working_threads {
        if thread_on(tx, aid, tid)? {
            insert_link(tx, aid, n, tid, LinkSource::Working)?;
        }
    }
    Ok(())
}

/// Sets each version's `addresses`, in link order.
pub(crate) fn fill_addresses(c: &Connection, aid: &ArtifactId, versions: &mut [Version]) -> Result<()> {
    let mut stmt = c.prepare(
        "SELECT version_n, thread_id FROM version_threads WHERE artifact_id = ?1 ORDER BY created_at, rowid",
    )?;
    let rows = stmt.query_map(params![aid.as_str()], |r| Ok((r.get::<_, u32>(0)?, r.get::<_, String>(1)?)))?;
    for row in rows {
        let (n, tid) = row?;
        if let Some(v) = versions.iter_mut().find(|v| v.n == n) {
            v.addresses.push(tid);
        }
    }
    Ok(())
}

impl Store {
    /// The versions `thread_id` is linked to, ascending.
    pub fn addressed_in(&self, thread_id: &str) -> Result<Vec<u32>> {
        self.with_conn(|c| {
            let mut stmt = c.prepare("SELECT version_n FROM version_threads WHERE thread_id = ?1 ORDER BY version_n")?;
            Ok(stmt.query_map(params![thread_id], |r| r.get(0))?.collect::<rusqlite::Result<Vec<u32>>>()?)
        })
    }

    /// An agent resolved `thread_id`: links it to its artifact's current
    /// version when it has no link yet. The version linked, or `None`.
    pub fn link_on_resolve(&self, thread_id: &str) -> Result<Option<u32>> {
        self.with_tx(|tx| {
            let linked: bool = tx
                .query_row("SELECT 1 FROM version_threads WHERE thread_id = ?1 LIMIT 1", params![thread_id], |_| Ok(()))
                .optional()?
                .is_some();
            if linked {
                return Ok(None);
            }
            let (aid, n): (String, u32) = tx.query_row(
                "SELECT a.id, a.current_version FROM threads t JOIN artifacts a ON a.id = t.artifact_id WHERE t.id = ?1",
                params![thread_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            ).optional()?.ok_or(CoreError::NotFound)?;
            insert_link(tx, &aid, n, thread_id, LinkSource::Resolve)?;
            Ok(Some(n))
        })
    }

    /// The viewer's seen mark on `aid`.
    pub fn seen(&self, viewer_id: &str, aid: &ArtifactId) -> Result<Option<u32>> {
        self.with_conn(|c| {
            Ok(c.query_row(
                "SELECT seen_n FROM viewer_seen WHERE viewer_id = ?1 AND artifact_id = ?2",
                params![viewer_id, aid.as_str()],
                |r| r.get(0),
            ).optional()?)
        })
    }

    /// Raises the viewer's seen mark on `aid` to `n` (never lowers it) and
    /// prunes the viewer's rows past [`MAX_SEEN_PER_VIEWER`], least recently
    /// updated first. The mark after the write.
    pub fn mark_seen(&self, viewer_id: &str, aid: &ArtifactId, n: u32) -> Result<u32> {
        self.with_tx(|tx| {
            tx.execute(
                "INSERT INTO viewer_seen (viewer_id, artifact_id, seen_n, updated_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (viewer_id, artifact_id) DO UPDATE SET seen_n = MAX(seen_n, excluded.seen_n), updated_at = excluded.updated_at",
                params![viewer_id, aid.as_str(), n, Store::now()],
            )?;
            tx.execute(
                "DELETE FROM viewer_seen WHERE viewer_id = ?1 AND artifact_id NOT IN
                   (SELECT artifact_id FROM viewer_seen WHERE viewer_id = ?1 ORDER BY updated_at DESC, rowid DESC LIMIT ?2)",
                params![viewer_id, MAX_SEEN_PER_VIEWER as i64],
            )?;
            Ok(tx.query_row(
                "SELECT seen_n FROM viewer_seen WHERE viewer_id = ?1 AND artifact_id = ?2",
                params![viewer_id, aid.as_str()],
                |r| r.get(0),
            )?)
        })
    }
}
```

`Store::now()` has millisecond resolution, so rows written in the same millisecond tie on `updated_at`. The `rowid DESC` tiebreak keeps the most recently inserted, and an update of an existing row keeps its original `rowid`. In the bound test every row is inserted once, so insertion order decides.

`store/threads.rs::delete_thread_touched`: before `DELETE FROM feedback`, add `tx.execute("DELETE FROM version_threads WHERE thread_id = ?1", params![thread_id])?;`. `store/artifacts.rs::delete_artifact`: in its transaction add `DELETE FROM viewer_seen WHERE artifact_id = ?1` and `DELETE FROM version_threads WHERE artifact_id = ?1`.

Run: `cargo test -p clax-core`
Expected: PASS. Update existing tests whose expected `Version` JSON is exact by adding `"note": null, "addresses": []`, and change nothing else.

- [ ] **Step 4: Gates and staging**

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add crates/clax-core/src/changelog.rs crates/clax-core/src/store/changelog.rs crates/clax-core/src/lib.rs crates/clax-core/src/store/mod.rs \
  crates/clax-core/src/store/migrations.rs crates/clax-core/src/model.rs crates/clax-core/src/publish.rs crates/clax-core/src/store/artifacts.rs \
  crates/clax-core/src/store/threads.rs crates/clax-core/src/working.rs
git add -u crates/clax-core
git status --short   # staged; the controller commits ("Store version notes, the threads each version addressed, and viewers' seen marks")
```

---

### Task 13: Changelog routes: automatic links at publish and resolve, version seen marks

**Files:**
- Modify: `crates/clax-server/src/routes/artifacts.rs` (`create`, `publish`), `crates/clax-server/src/routes/threads.rs` (`resolve`), `crates/clax-server/src/feedback.rs` (`thread_view`), `crates/clax-server/src/routes/viewers.rs`, `crates/clax-server/src/routes/mod.rs`
- Create: `crates/clax-server/tests/api_changelog.rs`

**Interfaces:**
- Consumes: Task 12's store API and `Working::threads_of`.
- Produces: publish responses carry `version.note`, `version.addresses` and a top-level `note_truncated`. Thread views carry `addressed_in`. A `thread` event goes out for each thread a publish or resolve linked.
- Produces: `GET /api/viewers/me/seen?artifact=<aid>` → `{seen: n | null}`, and `PUT /api/viewers/me/seen` `{artifact_id, version}` → `{seen}`. No token. `SameOrigin`. PUT without a cookie is 400 `no_viewer`. An unknown artifact is 404.

- [ ] **Step 1: Failing tests**

`crates/clax-server/tests/api_changelog.rs`:

```rust
mod common;
use common::TestServer;
use serde_json::{Value, json};

async fn setup(ts: &TestServer) -> (String, String, String) {
    let s = ts.register_session("claude", "cl1").await;
    let sid = s["id"].as_str().unwrap().to_string();
    let a = ts.publish_as(&sid, "T", "<main><h2>Quarterly goals</h2></main>").await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let t = ts.thread(&aid, 1, "@agent two columns").await;
    (sid, aid, t["id"].as_str().unwrap().to_string())
}

async fn publish(ts: &TestServer, sid: &str, aid: &str, extra: Value) -> reqwest::Response {
    let mut body = json!({"if_version": 1, "files": {"index.html": {"content": "<main><h2>Quarterly goals</h2></main>", "encoding": "utf8"}}});
    body.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    ts.authed(ts.client.post(format!("{}/api/artifacts/{aid}/versions", ts.base)))
        .header("x-clax-session", sid).json(&body).send().await.unwrap()
}

#[tokio::test]
async fn a_publish_links_the_threads_the_session_was_working_on_then_clears() {
    let ts = TestServer::spawn().await;
    let (sid, aid, tid) = setup(&ts).await;
    ts.get_authed(&format!("/api/sessions/{sid}/feedback?tier=piggyback")).await;
    let mut ev = ts.events(&format!("?artifact={aid}&types=thread")).await;
    let res = publish(&ts, &sid, &aid, json!({"note": "Two columns"})).await;
    assert_eq!(res.status(), 201);
    let v: Value = res.json().await.unwrap();
    assert_eq!(v["version"]["note"], "Two columns");
    assert_eq!(v["version"]["addresses"], json!([tid]));
    assert_eq!(v["note_truncated"], false);
    assert_eq!(ev.next_named("thread").await["thread"]["addressed_in"], json!([2]));
    let t: Value = ts.get(&format!("/api/artifacts/{aid}/threads/{tid}")).await.json().await.unwrap();
    assert_eq!(t["thread"]["status"], "open", "linking leaves the thread open");
    let w: Value = ts.get(&format!("/api/artifacts/{aid}/working")).await.json().await.unwrap();
    assert_eq!(w["working"], json!([]));
    let a: Value = ts.get(&format!("/api/artifacts/{aid}")).await.json().await.unwrap();
    assert_eq!(a["versions"][1]["addresses"], json!([tid]));
}

#[tokio::test]
async fn explicit_addresses_are_checked_before_anything_is_published() {
    let ts = TestServer::spawn().await;
    let (sid, aid, tid) = setup(&ts).await;
    let res = publish(&ts, &sid, &aid, json!({"addresses": [clax_core::new_ulid()]})).await;
    assert_eq!(res.status(), 400);
    assert_eq!(res.json::<Value>().await.unwrap()["error"]["code"], "unknown_thread");
    let a: Value = ts.get(&format!("/api/artifacts/{aid}")).await.json().await.unwrap();
    assert_eq!(a["artifact"]["current_version"], 1);
    let v: Value = publish(&ts, &sid, &aid, json!({"addresses": [tid]})).await.json().await.unwrap();
    assert_eq!(v["version"]["addresses"], json!([tid]));
}

#[tokio::test]
async fn an_agent_resolve_links_to_the_current_version_once() {
    let ts = TestServer::spawn().await;
    let (sid, aid, tid) = setup(&ts).await;
    ts.send_thread(&aid, &tid).await;
    let res = ts.authed(ts.client.post(format!("{}/api/artifacts/{aid}/threads/{tid}/resolve", ts.base)))
        .header("x-clax-session", &sid).json(&json!({"as": "agent"})).send().await.unwrap();
    let v: Value = res.json().await.unwrap();
    assert_eq!(v["thread"]["addressed_in"], json!([1]));
}

#[tokio::test]
async fn seen_marks_are_per_viewer_monotonic_and_refuse_foreign_origins() {
    let ts = TestServer::spawn().await;
    let (_sid, aid, _) = setup(&ts).await;
    let alex = ts.viewer(Some("Alex")).await;
    let other = ts.viewer(None).await;
    let get = |cookie: String| {
        let (ts, aid) = (&ts, aid.clone());
        async move {
            ts.client.get(format!("{}/api/viewers/me/seen?artifact={aid}", ts.base))
                .header("cookie", format!("clax_viewer={cookie}")).send().await.unwrap().json::<Value>().await.unwrap()
        }
    };
    let put = |cookie: Option<String>, n: u32, origin: Option<&'static str>| {
        let (ts, aid) = (&ts, aid.clone());
        async move {
            let mut r = ts.client.put(format!("{}/api/viewers/me/seen", ts.base)).json(&json!({"artifact_id": aid, "version": n}));
            if let Some(c) = cookie { r = r.header("cookie", format!("clax_viewer={c}")); }
            if let Some(o) = origin { r = r.header("origin", o); }
            r.send().await.unwrap()
        }
    };
    assert_eq!(get(alex.cookie.clone()).await, json!({"seen": null}));
    assert_eq!(put(Some(alex.cookie.clone()), 3, None).await.json::<Value>().await.unwrap(), json!({"seen": 3}));
    assert_eq!(put(Some(alex.cookie.clone()), 2, None).await.json::<Value>().await.unwrap(), json!({"seen": 3}));
    assert_eq!(get(other.cookie.clone()).await, json!({"seen": null}));
    assert_eq!(put(None, 1, None).await.status(), 400);
    assert_eq!(put(Some(alex.cookie.clone()), 4, Some("http://evil.example")).await.status(), 403);
}
```

Run: `cargo test -p clax-server --test api_changelog`
Expected: FAIL.

- [ ] **Step 2: Implement**

`routes/artifacts.rs::publish`: `validate` returns `p`, so make it `let mut p`. Inside the `store_call` closure, after `publishing_session`, add:

```rust
            if let Some(sid) = &session {
                p.working_threads = ctx.working.threads_of(sid, id.as_str());
            }
            let links_before: Vec<String> = p.addresses.iter().chain(&p.working_threads).cloned().collect();
            let truncated = p.note_truncated;
```

The working record must be read before the clear that Task 9 added after `ensure_watch`. After the publish succeeds and the version event is sent, publish a `thread` event for each linked thread that exists (`st.get_thread(tid)?` then `crate::routes::threads::publish_thread(&ctx, st, &t)?`; make `publish_thread` `pub(crate)`). Return `truncated` alongside and add `"note_truncated": truncated` to the response JSON. `create` does the same for `note_truncated`. `addresses` there can only fail, since no thread exists yet.

`feedback.rs::thread_view`: add `v["addressed_in"] = json!(st.addressed_in(&t.id)?);`.

`routes/threads.rs::resolve`, agent branch: after the resolve succeeds, `st.link_on_resolve(&tid)?;`, before the thread view is built and published.

`routes/viewers.rs`: add these (the file already imports `SameOrigin` and `ViewerCookie`, whose `.0` is the cookie, `Option<String>`):

```rust
#[derive(Deserialize)]
pub struct SeenQuery {
    artifact: String,
}

/// `GET /api/viewers/me/seen?artifact=<aid>`: `{seen}`, the highest version
/// this viewer has viewed unpinned; null for none or no cookie.
pub async fn seen(
    State(s): State<AppState>,
    _o: SameOrigin,
    viewer: ViewerCookie,
    q: Result<Query<SeenQuery>, QueryRejection>,
) -> Result<Json<Value>, ApiError> {
    let Query(q) = q.map_err(|e| ApiError::bad_request("invalid_query", e.body_text()))?;
    let id = parse_id(&q.artifact)?;
    let n = s.store_call(move |st| {
        st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
        match viewer.0.as_deref() {
            Some(cookie) => match st.get_viewer(cookie)? {
                Some(v) => st.seen(&v.id, &id),
                None => Ok(None),
            },
            None => Ok(None),
        }
    }).await?;
    Ok(Json(json!({"seen": n})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeenBody {
    artifact_id: String,
    version: u32,
}

/// `PUT /api/viewers/me/seen`: raises the mark (never lowers it); `{seen}`.
/// 400 `no_viewer` without a viewer cookie.
pub async fn set_seen(
    State(s): State<AppState>,
    _o: SameOrigin,
    viewer: ViewerCookie,
    req: Result<Json<SeenBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let b = body(req)?;
    let id = parse_id(&b.artifact_id)?;
    let cookie = viewer.0.ok_or_else(|| ApiError::bad_request("no_viewer", "open /api/viewers/me first"))?;
    let n = s.store_call(move |st| {
        st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
        let v = st.upsert_viewer(&cookie, None)?;
        st.mark_seen(&v.id, &id, b.version)
    }).await?;
    Ok(Json(json!({"seen": n})))
}
```

`routes/mod.rs`: `.route("/api/viewers/me/seen", get(viewers::seen).put(viewers::set_seen))`, placed before the `/api/viewers/me` route.

Run: `cargo test -p clax-server`
Expected: PASS. Existing exact thread-view assertions gain `"addressed_in": []`, and publish-response assertions gain `note_truncated`.

- [ ] **Step 3: Gates and staging**

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add crates/clax-server/src/routes/artifacts.rs crates/clax-server/src/routes/threads.rs crates/clax-server/src/feedback.rs \
  crates/clax-server/src/routes/viewers.rs crates/clax-server/src/routes/mod.rs crates/clax-server/tests/api_changelog.rs
git add -u crates/clax-server/tests
git status --short   # staged; the controller commits ("Link versions to the threads they addressed, and keep each viewer's seen mark")
```

---

### Task 14: `note` and `addresses` on publish (the tool, the CLI and Pi), and the smoke shows the links

#### A. `note` and `addresses` on the publish tool, the CLI and Pi

**Files:**
- Modify: `plugins/pi/test/fixtures/contract.json` (`publish` and `comments_resolve` descriptions), `crates/clax-mcp/src/tools.rs`, `crates/clax-mcp/tests/tools.rs`, `crates/clax-mcp/tests/comments.rs`, `crates/clax-cli/src/commands/publish.rs`, `crates/clax-cli/tests/cli.rs`, `plugins/pi/src/clax.ts`, `plugins/pi/test/clax.test.ts`, `plugins/*/skills/clax/SKILL.md` (the `### publish` argument list in each, and the shared "Comment loop"), `docs/contract.md` (`### Tools` row for `comments_resolve`)

**Interfaces:**
- `PublishArgs` (Rust and Pi) gains `note: Option<String>` ("A short change note for the person, at most 280 characters.") and `addresses: Option<Vec<String>>` ("IDs of the comment threads this version addresses.").
- The publish result gains `note` (string or null), `note_truncated` (bool) and `addressed` (thread IDs linked to the new version).
- `clax publish` gains `--note <TEXT>` and `--addresses <ID,ID,…>` (`value_delimiter = ','`). Its `--json` output gains `note` and `addressed`.
- New descriptions, verbatim in the fixture, `tools.rs` and `clax.ts`:

```text
Publish an HTML page as a new artifact, or as a new version of an existing one (pass `id` or `url`, with `if_version`). Give the page as `html` or `file_path`, plus optional supporting `files`. Returns the artifact ID, its URL for the person, and the new version number. Add a short `note` (at most 280 characters) saying what changed, and list the comment threads this version addresses in `addresses`; threads you were marked working on are added for you. The person sees both as the version's changelog; nothing is resolved by it.
```

```text
Resolve a comment thread that was sent to you, once you have acted on it and replied. Threads not sent to the agent are left alone (`resolved: false` with `guidance`). A thread no version lists yet is listed as addressed in the artifact's current version.
```

- [ ] **Step 1: Failing tests**

Append to `crates/clax-mcp/tests/comments.rs`:

```rust
#[tokio::test]
async fn publish_carries_a_note_and_links_addressed_and_working_threads() {
    let ts = TestServer::spawn().await;
    let (tools, sid) = session_tools(&ts).await;
    let (v1, _) = blocks(&tools.publish(Parameters(PublishArgs { html: Some("<h2>Goals</h2>".into()), title: Some("T".into()), ..Default::default() })).await.unwrap());
    let aid = v1["artifact_id"].as_str().unwrap().to_string();
    let t1 = ts.thread(&aid, 1, "@agent a").await["id"].as_str().unwrap().to_string();
    let t2 = ts.thread(&aid, 1, "plain b").await["id"].as_str().unwrap().to_string();
    ts.get_authed(&format!("/api/sessions/{sid}/feedback?tier=piggyback")).await;
    let (v2, _) = blocks(&tools.publish(Parameters(PublishArgs {
        id: Some(aid.clone()), html: Some("<h2>Goals</h2><p>2</p>".into()),
        note: Some("Two columns".into()), addresses: Some(vec![t2.clone()]), ..Default::default()
    })).await.unwrap());
    assert_eq!(v2["note"], "Two columns");
    assert_eq!(v2["note_truncated"], false);
    assert_eq!(v2["addressed"], json!([t2, t1]));
    let t: Value = ts.get(&format!("/api/artifacts/{aid}/threads/{t1}")).await.json().await.unwrap();
    assert_eq!(t["thread"]["status"], "open");
}
```

Append to `crates/clax-cli/tests/cli.rs`:

```rust
#[test]
fn publish_takes_a_note_and_addresses() {
    let e = Env::new();
    let index = write(e.dir.path(), "n/index.html", "<title>Noted</title><main><h2>Goals</h2></main>");
    let v: serde_json::Value = serde_json::from_slice(
        &e.cmd().args(["publish", "--json", "--port", "0"]).arg(&index).assert().success().get_output().stdout,
    ).unwrap();
    let id = v["id"].as_str().unwrap().to_string();
    let bad = e.cmd().args(["publish", "--json", "--id", &id, "--addresses", "01J9ZZZZZZZZZZZZZZZZZZZZZZ"]).arg(&index).assert().failure();
    assert!(String::from_utf8_lossy(&bad.get_output().stderr).contains("unknown_thread"));
    let v2: serde_json::Value = serde_json::from_slice(
        &e.cmd().args(["publish", "--json", "--id", &id, "--note", "Tighter spacing"]).arg(&index).assert().success().get_output().stdout,
    ).unwrap();
    assert_eq!(v2["version"], 2);
    assert_eq!(v2["note"], "Tighter spacing");
    assert_eq!(v2["addressed"], serde_json::json!([]));
}
```

Append to `plugins/pi/test/clax.test.ts`, in `describe("comments", ...)`:

```ts
  it("clax_publish carries a note and addresses, as the MCP tool does", async () => {
    const { pi, ctx } = load(daemon.home, "pi-note");
    await pi.emit("session_start", { type: "session_start", reason: "startup" }, ctx);
    const v1 = JSON.parse((await pi.callTool("clax_publish", { html: "<h2>Goals</h2>", title: "N" }, ctx)).content[0].text!);
    const form = new FormData();
    form.set("anchor", JSON.stringify({ kind: "element", selector: "body > h2", quote: "Goals", prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null }));
    form.set("body", "plain");
    form.set("version", "1");
    const tid = (await (await fetch(`${daemon.base}/api/artifacts/${v1.artifact_id}/threads`, { method: "POST", body: form })).json()).thread.id;
    const v2 = JSON.parse((await pi.callTool("clax_publish", { id: v1.artifact_id, html: "<h2>Goals</h2>", note: "Done", addresses: [tid] }, ctx)).content[0].text!);
    expect(v2).toMatchObject({ version: 2, note: "Done", note_truncated: false, addressed: [tid] });
  });
```

Run: `cargo test -p clax-mcp --test comments publish_carries && cargo test -p clax-cli --test cli publish_takes && (cd plugins/pi && npm test -- -t "carries a note")`
Expected: FAIL.

- [ ] **Step 2: Implement**

- Fixture: replace the `publish` and `comments_resolve` descriptions with the new strings above. `tools.rs` and `clax.ts`: replace the same two description literals.
- `tools.rs` `PublishArgs`: add the two fields with the doc comments above. In `do_publish`, after the `("label", a.label)` loop, add `if let Some(n) = a.note { body["note"] = json!(n); }` and `if let Some(t) = a.addresses { for tid in &t { check_thread_id(tid)?; } body["addresses"] = json!(t); }`. Add to the result object: `"note": res["version"]["note"], "note_truncated": res["note_truncated"], "addressed": res["version"]["addresses"],`. Update `crates/clax-mcp/tests/tools.rs` wherever it compares a whole publish result.
- `clax.ts` `PublishArgs`: add `note: opt(str("A short change note for the person, at most 280 characters.")),` and `addresses: opt(Type.Array(Type.String(), { description: "IDs of the comment threads this version addresses." })),`. In `publish`, extend the copy loop to `["description", "icon", "label", "note"]`, add `if (a.addresses) { for (const t of a.addresses) checkThreadId(t); body.addresses = a.addresses; }`, and add `note: res.version?.note ?? null, note_truncated: res.note_truncated ?? false, addressed: res.version?.addresses ?? []` to the result.
- `crates/clax-cli/src/commands/publish.rs` `Args`:

```rust
    /// A short change note shown to people as this version's changelog.
    #[arg(long)]
    pub note: Option<String>,
    /// IDs of comment threads this version addresses (comma-separated).
    #[arg(long, value_delimiter = ',')]
    pub addresses: Vec<String>,
```

Add `("note", &a.note)` to the body loop and `if !a.addresses.is_empty() { body["addresses"] = serde_json::json!(a.addresses); }`. Add `"note": res["version"]["note"], "addressed": res["version"]["addresses"]` to the printed JSON. For the text output, append `\naddresses N comment(s)` when `addressed` is non-empty.
- Each `SKILL.md` `### publish` argument list: after the `label` bullet, add

```markdown
- `note` (string, optional): a short change note for the person, at most 280
  characters ("Two columns; third bullet dropped"). Shown as this version's
  changelog.
- `addresses` (array of thread IDs, optional): the comment threads this
  version addresses. Threads you were marked working on are added for you.
  Listing a thread does not resolve it.
```

  In the shared `## Comment loop`, replace step 2's first sentence with ``2. Make the change, usually by publishing a new version of the same artifact (`publish` with its `id` or `url`), with a `note` saying what changed and `addresses` naming the threads it handles.`` Apply it to all three files identically.
- `docs/contract.md` `### Tools` table: the `comments_resolve` row's Result cell gains `; a thread no version lists yet is listed as addressed in the current version`.

Run: `python3 scripts/sync-skill-tools.py && bash scripts/test-plugins.sh | tail -1 && cargo test -p clax-mcp && cargo test -p clax-cli && (cd plugins/pi && npm run typecheck && npm test)`
Expected: `plugin checks passed`, then every suite PASS.

- [ ] **Step 3: Gates and staging**

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add plugins/pi/test/fixtures/contract.json crates/clax-mcp/src/tools.rs crates/clax-mcp/tests/tools.rs crates/clax-mcp/tests/comments.rs \
  crates/clax-cli/src/commands/publish.rs crates/clax-cli/tests/cli.rs plugins/pi/src/clax.ts plugins/pi/test/clax.test.ts \
  plugins/claude-code/skills/clax/SKILL.md plugins/clax/skills/clax/SKILL.md plugins/pi/skills/clax/SKILL.md docs/contract.md
git status --short   # staged; this task continues below
```

#### B. The comment-loop smoke shows a publish linking its threads

**Files:**
- Modify: `scripts/smoke-comment-loop.sh`

- [ ] **Step 4: Assert the links**

In step 6b of the Python block, the agent's reply in step 6 emptied its working record. Mark it working on thread 3 (the one `wait_for_feedback` returned in step 5), then publish the second page with a note, naming the plain thread explicitly. Replace the `v2, _ = shim.call("publish", {"id": aid, ...})` line with:

```python
shim.call("working", {"url_or_id": aid, "thread_ids": [sent_at["thread"]]})
v2, _ = shim.call("publish", {"id": aid, "html": page, "note": "Added the team page",
                              "addresses": [plain["id"]],
                              "files": {"about.html": {"content": "<main><h2>Our team</h2></main>"}}})
if v2["note"] != "Added the team page" or v2["addressed"] != [plain["id"], sent_at["thread"]]:
    fail(f"publish note and addresses: {v2}")
for tid in (plain["id"], sent_at["thread"]):
    linked = http("GET", f"/api/artifacts/{aid}/threads/{tid}")["thread"]
    if linked["addressed_in"] != [v2["version"]] or linked["status"] != "open":
        fail(f"addressed_in after publish: {linked}")
ok(f"publish v{v2['version']} carried its note and listed the named thread and the one it was working on, both still open")
if working(aid):
    fail(f"working after the publish: {working(aid)}")
ok("working: the publish cleared the session's working record")
```

Keep the lines that follow unchanged. `plain` is defined in step 6 above this point, and `working` in Task 11.

- [ ] **Step 5: Run, gates and staging**

Run: `scripts/smoke-comment-loop.sh`
Expected: all `PASS`, ending with `comment loop smoke passed`.

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add scripts/smoke-comment-loop.sh
git status --short   # staged; the controller commits ("Take a change note and addressed threads on publish, and show the links in the comment-loop smoke")
```

---

### Task 15: Participants: comment authors, @mentions, agent handles, looked-at marks and attention

The daemon side of "threads you're in". This task adds:
- comments record their author's viewer public ID;
- @mentions are stored;
- each viewer's looked-at marks are stored;
- the daemon computes each viewer's attention per artifact;
- sessions get an opaque `agent_handle`, so the shell can name and target an agent;
- version views name their publishing agent.

It all rides on the responses the shell already loads and on the bootstrap block. Only the gallery adds one request, made beside its list.

**Files:**
- Create: `crates/clax-core/src/mentions.rs`, `crates/clax-core/src/store/attention.rs`, `crates/clax-server/tests/api_attention.rs`
- Modify: `crates/clax-core/src/lib.rs`, `crates/clax-core/src/ids.rs`, `crates/clax-core/src/store/mod.rs`, `crates/clax-core/src/store/migrations.rs` (migration 11), `crates/clax-core/src/store/threads.rs` (`NewThread`, `NewComment`, the comment row, `delete_thread_touched`), `crates/clax-core/src/store/sessions.rs` (`register_session`), `crates/clax-core/src/store/artifacts.rs` (`delete_artifact`, the version row), `crates/clax-core/src/model.rs` (`Comment`, `Session`, `Version`), `crates/clax-server/src/viewer.rs` (`author`), `crates/clax-server/src/routes/threads.rs` (create, comment), `crates/clax-server/src/routes/artifacts.rs` (`get`, `list`, `with_owner`), `crates/clax-server/src/routes/viewers.rs`, `crates/clax-server/src/routes/mod.rs`, `crates/clax-server/src/boot.rs`, `crates/clax-server/src/testing.rs`, `crates/clax-server/tests/api_sessions.rs`, `crates/clax-server/tests/shell_boot.rs`, `crates/clax-core/src/store/changelog.rs` (test literals), `docs/follow-ups.md`, `web/shell/src/api.ts`, `web/shell/src/threads.ts`, `web/shell/src/view/boot.ts`

**Interfaces:**
- Token-less artifact views name no session. `GET /api/artifacts` and `GET /api/artifacts/<aid>` without the bearer token leave out `artifact.owner_session_id` and every `versions[].session_id`, as the `/a/…` bootstrap already does. With the token they are unchanged; the MCP server, the CLI and the Pi extension always send the token (`crates/clax-mcp/src/client.rs` `bearer_auth`, `plugins/pi/src/client.ts` `authorization`), so `scope: "mine"` and `clax publish` keep reading them. `crate::routes::artifacts::strip_sessions(v: &mut Value)` does the stripping, and `boot::without_sessions` calls it.
- `clax_core::ids::new_agent_handle() -> String`: `a_` and 22 lowercase hex digits. `is_agent_handle(&str) -> bool`.
- `clax_core::mentions::mentioned(body: &str, names: &[(String, String)]) -> Vec<String>` (decided: Q3) takes `(public_id, display_name)` pairs and returns the public IDs whose `@<display name>` appears in `body`. The match ignores case and needs a boundary after the name (end, whitespace, or one of `.,;:!?)]}'"`). `@agent` is never a viewer.
- `NewThread` and `NewComment` gain `author_public_id: Option<String>`. Comment views gain `author_public_id` (null for agents and anonymous viewers).
- `Session` gains `agent_handle: String`. Version views gain `agent: Option<String>` (the publishing session's handle) and `agent_harness: Option<String>`.
- The `Store` gains:
  - `participants(aid) -> Result<Participants>`, where `Participants { people: Vec<Person { public_id, display_name, seen }>, agents: Vec<AgentView { handle, harness, live }> }`.
    - `Person.seen: Option<u32>` is the person's `viewer_seen` on this artifact. It is public (decided: Q7): anyone who can read the artifact sees it, without a cookie.
    - Agents are the owner, the watchers and the version publishers, at most 10. `live` is true when the session is live and is the owner or a watcher, exactly the sessions `live_agent` accepts, so a send can reach every agent shown live.
    - The order is live first, then most recently active on the artifact (the newest of its versions, its comments on the artifact's threads and its watch, else its registration), so the shell's default target is the first live agent.
  - `live_agent(aid, handle) -> Result<Option<String>>`: the session ID of the live owner or watcher whose handle this is, for the send target in Task 21.
  - `mark_looked(viewer_id, aid, &[String]) -> Result<BTreeMap<String, String>>`: only threads of `aid`. It answers the marks after the write.
  - `attention(viewer_id, aid) -> Result<Attention { addressed, new_replies, open_in: Vec<String>, addressed_v: Option<u32> (the newest version among those links), seen: Option<u32>, looked: BTreeMap<String, String> }>`.
  - `attention_all(viewer_id) -> Result<BTreeMap<String, AttentionSummary { addressed, new_replies, open_in, seen }>>`, over live artifacts.
- HTTP, all viewer routes:
  - `GET /api/artifacts/<aid>` adds `participants`, and with a viewer cookie also `attention`. That response is then `Cache-Control: private, no-cache` with `Vary: Cookie`.
  - `GET /api/artifacts` adds `participants` per artifact.
  - `GET /api/viewers/me/attention` answers `{artifacts: {<aid>: summary}}`, and `{artifacts: {}}` without a cookie.
  - `PUT /api/viewers/me/looked` takes `{artifact_id, thread_ids}` (1 to 50 ULIDs) and answers `{looked}`. Without a cookie it is 400 `no_viewer`. A foreign `Origin` is 403. An unknown artifact is 404.
  - The bootstrap block gains `participants` (inside `artifact.artifact`) and `attention` (top level, only with a viewer).
- Shell types: `Comment.author_public_id?: string | null`; `Version.agent?: string | null` and `Version.agent_harness?: string | null`; `Artifact.participants?: Participants`; `Boot.attention?: Attention | null`; and `getAttention(): Promise<Record<string, AttentionSummary>>` and `putLooked(aid, ids): Promise<void>` in `api.ts`.

- [ ] **Step 1: Failing tests**

In `crates/clax-server/src/testing.rs`, add:

```rust
    /// Creates a thread as the viewer whose cookie value is `cookie`; returns the thread view.
    pub async fn thread_as(&self, aid: &str, cookie: &str, body: &str) -> serde_json::Value {
        let form = reqwest::multipart::Form::new()
            .text("anchor", element_anchor().to_string())
            .text("body", body.to_string())
            .text("version", "1");
        let res = self.client.post(format!("{}/api/artifacts/{aid}/threads", self.base))
            .header("cookie", format!("clax_viewer={cookie}")).multipart(form).send().await.unwrap();
        assert_eq!(res.status(), 201);
        res.json::<serde_json::Value>().await.unwrap()["thread"].clone()
    }

    /// Replies on `tid` as the viewer whose cookie value is `cookie`; returns the thread view.
    pub async fn reply_as(&self, aid: &str, tid: &str, cookie: &str, body: &str) -> serde_json::Value {
        let res = self.client.post(format!("{}/api/artifacts/{aid}/threads/{tid}/comments", self.base))
            .header("cookie", format!("clax_viewer={cookie}")).json(&serde_json::json!({"body": body})).send().await.unwrap();
        assert_eq!(res.status(), 201);
        res.json::<serde_json::Value>().await.unwrap()["thread"].clone()
    }
```

`crates/clax-server/tests/api_attention.rs`:

```rust
mod common;
use common::TestServer;
use serde_json::{Value, json};

async fn artifact(ts: &TestServer) -> (String, String) {
    let s = ts.register_session("claude", "att-1").await;
    let sid = s["id"].as_str().unwrap().to_string();
    let a = ts.publish_as(&sid, "T", "<main><h2>Quarterly goals</h2></main>").await;
    (sid, a["artifact"]["id"].as_str().unwrap().to_string())
}

async fn att(ts: &TestServer, aid: &str, cookie: &str) -> Value {
    let v: Value = ts.client.get(format!("{}/api/artifacts/{aid}", ts.base)).header("cookie", format!("clax_viewer={cookie}"))
        .send().await.unwrap().json().await.unwrap();
    v["attention"].clone()
}

async fn look(ts: &TestServer, aid: &str, cookie: &str, ids: &[&str]) -> reqwest::Response {
    ts.client.put(format!("{}/api/viewers/me/looked", ts.base)).header("cookie", format!("clax_viewer={cookie}"))
        .json(&json!({"artifact_id": aid, "thread_ids": ids})).send().await.unwrap()
}

#[tokio::test]
async fn comments_name_their_author_and_threads_you_are_in_are_yours() {
    let ts = TestServer::spawn().await;
    let (_sid, aid) = artifact(&ts).await;
    let alex = ts.viewer(Some("Alex")).await;
    let mia = ts.viewer(Some("Mia Kovač")).await;
    let t = ts.thread_as(&aid, &alex.cookie, "Two columns").await;
    let tid = t["id"].as_str().unwrap();
    assert_eq!(t["comments"][0]["author_public_id"], alex.public_id);
    assert_eq!(att(&ts, &aid, &alex.cookie).await["open_in"], json!([tid]));
    assert_eq!(att(&ts, &aid, &mia.cookie).await["open_in"], json!([]));
    let other = ts.thread_as(&aid, &alex.cookie, "@mia kovač which log?").await;
    let a = att(&ts, &aid, &mia.cookie).await;
    assert_eq!(a["open_in"], json!([other["id"]]), "a full-name mention puts Mia in the thread");
    assert_eq!(a["new_replies"], json!([other["id"]]), "someone else's comment she has not looked at");
}

#[tokio::test]
async fn an_address_after_your_last_look_needs_your_eyes_until_you_look() {
    let ts = TestServer::spawn().await;
    let (sid, aid) = artifact(&ts).await;
    let alex = ts.viewer(Some("Alex")).await;
    let tid = ts.thread_as(&aid, &alex.cookie, "Two columns").await["id"].as_str().unwrap().to_string();
    assert_eq!(look(&ts, &aid, &alex.cookie, &[&tid]).await.status(), 200);
    assert_eq!(att(&ts, &aid, &alex.cookie).await["addressed"], json!([]));
    let res = ts.authed(ts.client.post(format!("{}/api/artifacts/{aid}/versions", ts.base))).header("x-clax-session", &sid)
        .json(&json!({"if_version": 1, "addresses": [tid], "files": {"index.html": {"content": "<main><h2>Quarterly goals</h2></main>", "encoding": "utf8"}}}))
        .send().await.unwrap();
    assert_eq!(res.status(), 201);
    let a = att(&ts, &aid, &alex.cookie).await;
    assert_eq!(a["addressed"], json!([tid]));
    assert_eq!(a["addressed_v"], 2);
    look(&ts, &aid, &alex.cookie, &[&tid]).await;
    assert_eq!(att(&ts, &aid, &alex.cookie).await["addressed"], json!([]), "seeing the thread clears it; resolving is not needed");
}

#[tokio::test]
async fn attention_is_the_viewers_own_and_never_anyone_elses() {
    let ts = TestServer::spawn().await;
    let (_sid, aid) = artifact(&ts).await;
    let alex = ts.viewer(Some("Alex")).await;
    let tid = ts.thread_as(&aid, &alex.cookie, "Two columns").await["id"].as_str().unwrap().to_string();
    look(&ts, &aid, &alex.cookie, &[&tid]).await;
    let anon: Value = ts.get(&format!("/api/artifacts/{aid}")).await.json().await.unwrap();
    assert!(anon.get("attention").is_none(), "no cookie, no attention");
    let res = ts.client.get(format!("{}/api/artifacts/{aid}", ts.base)).header("cookie", format!("clax_viewer={}", alex.cookie)).send().await.unwrap();
    assert_eq!(res.headers()["vary"], "Cookie");
    assert!(res.headers()["cache-control"].to_str().unwrap().contains("private"));
    let threads: Value = ts.get(&format!("/api/artifacts/{aid}/threads")).await.json().await.unwrap();
    assert!(!threads.to_string().contains("looked"), "thread views never carry looked-at marks");
    assert_eq!(look(&ts, &aid, "not-a-cookie", &[&tid]).await.status(), 400);
    let foreign = ts.client.put(format!("{}/api/viewers/me/looked", ts.base)).header("cookie", format!("clax_viewer={}", alex.cookie))
        .header("origin", "http://evil.example").json(&json!({"artifact_id": aid, "thread_ids": [tid]})).send().await.unwrap();
    assert_eq!(foreign.status(), 403);
    let all: Value = ts.client.get(format!("{}/api/viewers/me/attention", ts.base)).header("cookie", format!("clax_viewer={}", alex.cookie))
        .send().await.unwrap().json().await.unwrap();
    assert_eq!(all["artifacts"][&aid]["open_in"], json!([tid]));
    assert!(all["artifacts"][&aid].get("looked").is_none());
}

#[tokio::test]
async fn participants_name_agents_by_handle_only() {
    let ts = TestServer::spawn().await;
    let (sid, aid) = artifact(&ts).await;
    let alex = ts.viewer(Some("Alex")).await;
    ts.thread_as(&aid, &alex.cookie, "Two columns").await;
    let v: Value = ts.get(&format!("/api/artifacts/{aid}")).await.json().await.unwrap();
    let p = &v["artifact"]["participants"];
    assert_eq!(p["people"], json!([{"public_id": alex.public_id, "display_name": "Alex", "seen": null}]));
    let agent = &p["agents"][0];
    assert_eq!(agent["harness"], "claude");
    assert_eq!(agent["live"], true);
    let handle = agent["handle"].as_str().unwrap();
    assert!(handle.starts_with("a_") && handle.len() == 24);
    assert!(!v.to_string().contains(&sid), "no session ID anywhere in the artifact view");
    assert_eq!(v["versions"][0]["agent"], handle);
    let list: Value = ts.get("/api/artifacts").await.json().await.unwrap();
    assert_eq!(list["artifacts"][0]["participants"]["agents"][0]["handle"], handle);
    assert!(!list.to_string().contains(&sid));
}

#[tokio::test]
async fn the_last_version_a_person_viewed_is_public_and_their_looks_are_not() {
    let ts = TestServer::spawn().await;
    let (_sid, aid) = artifact(&ts).await;
    let alex = ts.viewer(Some("Alex")).await;
    let tid = ts.thread_as(&aid, &alex.cookie, "Two columns").await["id"].as_str().unwrap().to_string();
    let res = ts.client.put(format!("{}/api/viewers/me/seen", ts.base)).header("cookie", format!("clax_viewer={}", alex.cookie))
        .json(&json!({"artifact_id": aid, "version": 1})).send().await.unwrap();
    assert_eq!(res.status(), 200);
    look(&ts, &aid, &alex.cookie, &[&tid]).await;
    let anon: Value = ts.get(&format!("/api/artifacts/{aid}")).await.json().await.unwrap();
    assert_eq!(anon["artifact"]["participants"]["people"][0]["seen"], 1, "another viewer, or no viewer, reads Alex's last seen version");
    assert!(!anon.to_string().contains("looked"), "Alex's looked-at marks stay Alex's");
    let list: Value = ts.get("/api/artifacts").await.json().await.unwrap();
    assert_eq!(list["artifacts"][0]["participants"]["people"][0]["seen"], 1);
}

#[tokio::test]
async fn agents_are_live_only_when_a_send_can_reach_them_and_the_most_recently_active_comes_first() {
    let ts = TestServer::spawn().await;
    let (owner, aid) = artifact(&ts).await;
    let w = ts.register_session("codex", "att-watch").await;
    let watcher = w["id"].as_str().unwrap().to_string();
    let res = ts.authed(ts.client.put(format!("{}/api/sessions/{watcher}/watches/{aid}", ts.base))).send().await.unwrap();
    assert!(res.status().is_success());
    let agents = |v: &Value| v["artifact"]["participants"]["agents"].as_array().unwrap().iter()
        .map(|a| (a["harness"].as_str().unwrap().to_string(), a["live"].as_bool().unwrap())).collect::<Vec<_>>();
    let v: Value = ts.get(&format!("/api/artifacts/{aid}")).await.json().await.unwrap();
    assert_eq!(agents(&v), [("codex".to_string(), true), ("claude".to_string(), true)], "the newest watch is the most recent activity");
    let res = ts.authed(ts.client.patch(format!("{}/api/sessions/{owner}", ts.base))).json(&json!({"ended": true})).send().await.unwrap();
    assert!(res.status().is_success());
    let v: Value = ts.get(&format!("/api/artifacts/{aid}")).await.json().await.unwrap();
    assert_eq!(agents(&v), [("codex".to_string(), true), ("claude".to_string(), false)], "an ended publisher stays listed, not live");
}

#[tokio::test]
async fn no_session_id_reaches_a_tokenless_caller() {
    let ts = TestServer::spawn().await;
    let (sid, aid) = artifact(&ts).await;
    for path in [format!("/api/artifacts/{aid}"), "/api/artifacts".to_string()] {
        let anon: Value = ts.get(&path).await.json().await.unwrap();
        assert!(!anon.to_string().contains(&sid), "{path} without the token: {anon}");
        let authed: Value = ts.get_authed(&path).await.json().await.unwrap();
        assert!(authed.to_string().contains(&sid), "{path} with the token keeps the owner session");
    }
    let one: Value = ts.get(&format!("/api/artifacts/{aid}")).await.json().await.unwrap();
    assert!(one["artifact"].get("owner_session_id").is_none());
    assert!(one["versions"][0].get("session_id").is_none());
    assert_eq!(one["artifact"]["owner_harness"], "claude", "the harness stays: the by-line needs it");
}
```

In `crates/clax-core/src/mentions.rs`, the tests module:

```rust
#[cfg(test)]
mod tests {
    use super::mentioned;

    fn names() -> Vec<(String, String)> {
        vec![("u_a".into(), "Alex".into()), ("u_m".into(), "Mia Kovač".into()), ("u_j".into(), "Jun".into())]
    }

    #[test]
    fn mentions_match_whole_names_in_any_case_at_a_boundary() {
        assert_eq!(mentioned("@alex and @JUN, look", &names()), ["u_a", "u_j"]);
        assert_eq!(mentioned("ask @mia kovač.", &names()), ["u_m"]);
        assert!(mentioned("@mia alone", &names()).is_empty(), "a two-word name needs both words");
        assert!(mentioned("@alexander", &names()).is_empty());
        assert!(mentioned("email alex@example.com", &names()).is_empty());
        assert!(mentioned("@agent please", &[("u_x".into(), "agent".into())]).is_empty());
    }
}
```

Run: `cargo test -p clax-core mentions; cargo test -p clax-server --test api_attention`
Expected: FAIL to compile.

- [ ] **Step 2: Migration 11**

Append to `MIGRATIONS` in `store/migrations.rs`:

```rust
    // 11: participants. A comment's author (the viewer's public ID), the
    // viewers a comment mentions, each viewer's last look at a thread, and an
    // opaque handle per session so the shell can name and target an agent
    // without a session ID. Existing sessions get a handle; existing comments
    // stay unattributed.
    "ALTER TABLE comments ADD COLUMN author_public_id TEXT;
    CREATE INDEX comments_by_author ON comments(author_public_id);
    CREATE TABLE mentions (
        comment_id TEXT NOT NULL REFERENCES comments(id),
        public_id TEXT NOT NULL,
        PRIMARY KEY (comment_id, public_id)
    );
    CREATE INDEX mentions_by_viewer ON mentions(public_id);
    CREATE TABLE viewer_threads (
        viewer_id TEXT NOT NULL REFERENCES viewers(id),
        thread_id TEXT NOT NULL REFERENCES threads(id),
        looked_at TEXT NOT NULL,
        PRIMARY KEY (viewer_id, thread_id)
    );
    ALTER TABLE sessions ADD COLUMN agent_handle TEXT;
    UPDATE sessions SET agent_handle = 'a_' || lower(hex(randomblob(11)));
    CREATE UNIQUE INDEX sessions_by_handle ON sessions(agent_handle);",
```

- [ ] **Step 3: Implement the core**

`ids.rs`: generalise `new_public_id` into `fn new_prefixed(prefix: &str) -> String`, and keep `new_public_id()` as `new_prefixed("u_")`. Add `pub fn new_agent_handle() -> String { new_prefixed("a_") }` and `pub fn is_agent_handle(s: &str) -> bool` (`a_` and 22 lowercase hex digits). Re-export both from `lib.rs`.

`crates/clax-core/src/mentions.rs`:

```rust
//! @mentions in comment text (spec §10, "Participants and attention").

/// The public IDs of the viewers `body` mentions: `@` and their whole display
/// name, in any case, followed by the end, whitespace or punctuation, and not
/// preceded by a word character (so an email address is no mention). A
/// viewer named `agent` is never mentioned: `@agent` sends to the agent.
pub fn mentioned(body: &str, names: &[(String, String)]) -> Vec<String> {
    let lower = body.to_lowercase();
    let mut out = Vec::new();
    for (public_id, name) in names {
        let n = name.trim().to_lowercase();
        if n.is_empty() || n == "agent" {
            continue;
        }
        let needle = format!("@{n}");
        let mut from = 0;
        while let Some(i) = lower[from..].find(&needle) {
            let at = from + i;
            let before_ok = lower[..at].chars().next_back().is_none_or(|c| !c.is_alphanumeric());
            let after = lower[at + needle.len()..].chars().next();
            let after_ok = after.is_none_or(|c| c.is_whitespace() || ".,;:!?)]}'\"".contains(c));
            if before_ok && after_ok {
                out.push(public_id.clone());
                break;
            }
            from = at + needle.len();
        }
    }
    out
}
```

`store/threads.rs`:
- `NewThread` and `NewComment` gain `pub author_public_id: Option<String>`. The comment `INSERT` writes it.
- After inserting a viewer comment (in `create_thread` and `add_comment`), insert its mentions in the same transaction:

```rust
fn insert_mentions(tx: &Transaction<'_>, comment_id: &str, body: &str) -> Result<()> {
    let names: Vec<(String, String)> = {
        let mut st = tx.prepare("SELECT public_id, display_name FROM viewers WHERE display_name IS NOT NULL AND display_name != ''")?;
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?
    };
    for p in crate::mentions::mentioned(body, &names) {
        tx.execute("INSERT OR IGNORE INTO mentions (comment_id, public_id) VALUES (?1, ?2)", params![comment_id, p])?;
    }
    Ok(())
}
```

- The comment row mapper reads `author_public_id`. `model::Comment` gains `pub author_public_id: Option<String>`, serialised.
- `delete_thread_touched` deletes the thread's `mentions` rows (through its comment IDs) and its `viewer_threads` rows, before the comments.

`store/sessions.rs::register_session`: insert `agent_handle = crate::new_agent_handle()`. `model::Session` gains `pub agent_handle: String`. Only token routes serve `Session`.

`store/artifacts.rs`: the version row mapper joins `sessions` on `versions.session_id` and fills `agent` (`agent_handle`) and `agent_harness`. `delete_artifact` deletes the `viewer_threads` and `mentions` rows of its threads.

`crates/clax-core/src/store/attention.rs`:

```rust
//! Participants and each viewer's attention per artifact (spec §10,
//! "Participants and attention"). Looked-at marks and attention are the
//! viewer's own: nothing here is served to anyone else.

use super::Store;
use crate::{ArtifactId, Result};
use rusqlite::{OptionalExtension, params};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Person {
    pub public_id: String,
    pub display_name: Option<String>,
    /// The latest version this person viewed at the artifact's latest URL. Public (spec §14).
    pub seen: Option<u32>,
}
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct AgentView { pub handle: String, pub harness: String, pub live: bool }
#[derive(Serialize, Debug, Clone, PartialEq, Default)]
pub struct Participants { pub people: Vec<Person>, pub agents: Vec<AgentView> }
#[derive(Serialize, Debug, Clone, PartialEq, Default)]
pub struct AttentionSummary { pub addressed: Vec<String>, pub addressed_v: Option<u32>, pub new_replies: Vec<String>, pub open_in: Vec<String>, pub seen: Option<u32> }
#[derive(Serialize, Debug, Clone, PartialEq, Default)]
pub struct Attention { #[serde(flatten)] pub summary: AttentionSummary, pub looked: BTreeMap<String, String> }

/// Most agents a participant list names.
pub const MAX_AGENTS: usize = 10;
/// Most threads one looked-at write may name.
pub const MAX_LOOKED: usize = 50;

/// Threads of `?1` (artifact) the viewer with public ID `?2` is in: wrote a
/// comment, is mentioned, or resolved it.
const IN_THREAD: &str = "SELECT t.id, t.status FROM threads t WHERE t.artifact_id = ?1 AND (
    EXISTS (SELECT 1 FROM comments c WHERE c.thread_id = t.id AND c.author_public_id = ?2)
    OR EXISTS (SELECT 1 FROM mentions m JOIN comments c ON c.id = m.comment_id WHERE c.thread_id = t.id AND m.public_id = ?2)
    OR t.resolved_by = 'viewer:' || ?2) ORDER BY t.created_at, t.id";

impl Store {
    pub fn participants(&self, aid: &ArtifactId) -> Result<Participants> {
        self.with_conn(|c| {
            // `seen` is public by design (decided: Q7); looked-at marks are not read here.
            let people = c.prepare(
                "SELECT v.public_id, v.display_name, s.seen_n FROM viewers v
                   LEFT JOIN viewer_seen s ON s.viewer_id = v.id AND s.artifact_id = ?1
                  WHERE v.public_id IN
                   (SELECT c.author_public_id FROM comments c JOIN threads t ON t.id = c.thread_id WHERE t.artifact_id = ?1 AND c.author_public_id IS NOT NULL)
                 ORDER BY v.created_at",
            )?.query_map(params![aid.as_str()], |r| Ok(Person { public_id: r.get(0)?, display_name: r.get(1)?, seen: r.get(2)? }))?
              .collect::<rusqlite::Result<Vec<_>>>()?;
            // `live`: a send can reach it (live, and the owner or a watcher), as `live_agent` checks.
            // Activity: the newest of its versions, its comments on this artifact's threads and its
            // watch, else its registration. SQLite's many-argument MAX is NULL when any argument is,
            // hence the COALESCEs.
            let agents = c.prepare(
                "SELECT s.agent_handle, s.harness,
                        (s.ended_at IS NULL AND (s.id = (SELECT owner_session_id FROM artifacts WHERE id = ?1)
                           OR s.id IN (SELECT session_id FROM watches WHERE artifact_id = ?1))) AS live,
                        MAX(COALESCE((SELECT MAX(created_at) FROM versions WHERE session_id = s.id AND artifact_id = ?1), ''),
                            COALESCE((SELECT MAX(c.created_at) FROM comments c JOIN threads t ON t.id = c.thread_id
                                       WHERE c.via_session_id = s.id AND t.artifact_id = ?1), ''),
                            COALESCE((SELECT created_at FROM watches WHERE session_id = s.id AND artifact_id = ?1), ''),
                            s.started_at) AS active_at
                   FROM sessions s
                  WHERE s.id = (SELECT owner_session_id FROM artifacts WHERE id = ?1)
                     OR s.id IN (SELECT session_id FROM watches WHERE artifact_id = ?1)
                     OR s.id IN (SELECT session_id FROM versions WHERE artifact_id = ?1)
                  ORDER BY live DESC, active_at DESC, s.id LIMIT ?2",
            )?.query_map(params![aid.as_str(), MAX_AGENTS as i64], |r| Ok(AgentView { handle: r.get(0)?, harness: r.get(1)?, live: r.get(2)? }))?
              .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(Participants { people, agents })
        })
    }

    /// The live owner or watcher of `aid` whose handle is `handle`, as a session ID.
    pub fn live_agent(&self, aid: &ArtifactId, handle: &str) -> Result<Option<String>> {
        self.with_conn(|c| Ok(c.query_row(
            "SELECT id FROM sessions WHERE agent_handle = ?2 AND ended_at IS NULL AND
               (id = (SELECT owner_session_id FROM artifacts WHERE id = ?1) OR id IN (SELECT session_id FROM watches WHERE artifact_id = ?1))",
            params![aid.as_str(), handle], |r| r.get(0)).optional()?))
    }

    /// Records that the viewer looked at `thread_ids` (threads of other
    /// artifacts are ignored) now; answers the viewer's marks on `aid`.
    pub fn mark_looked(&self, viewer_id: &str, aid: &ArtifactId, thread_ids: &[String]) -> Result<BTreeMap<String, String>> {
        let now = Store::now();
        self.with_tx(|tx| {
            for tid in thread_ids.iter().take(MAX_LOOKED) {
                tx.execute(
                    "INSERT INTO viewer_threads (viewer_id, thread_id, looked_at)
                     SELECT ?1, id, ?3 FROM threads WHERE id = ?2 AND artifact_id = ?4
                     ON CONFLICT (viewer_id, thread_id) DO UPDATE SET looked_at = excluded.looked_at",
                    params![viewer_id, tid, now, aid.as_str()],
                )?;
            }
            looked_in(tx, viewer_id, aid.as_str())
        })
    }

    pub fn attention(&self, viewer_id: &str, aid: &ArtifactId) -> Result<Attention> {
        self.with_conn(|c| {
            let public_id: Option<String> = c.query_row("SELECT public_id FROM viewers WHERE id = ?1", params![viewer_id], |r| r.get(0)).optional()?;
            let Some(p) = public_id else { return Ok(Attention::default()) };
            let looked = looked_in(c, viewer_id, aid.as_str())?;
            let summary = summary(c, viewer_id, &p, aid.as_str(), &looked)?;
            Ok(Attention { summary, looked })
        })
    }

    pub fn attention_all(&self, viewer_id: &str) -> Result<BTreeMap<String, AttentionSummary>> {
        let ids: Vec<ArtifactId> = self.list_artifacts()?.into_iter().map(|a| a.id).collect();
        let mut out = BTreeMap::new();
        for id in ids {
            let a = self.attention(viewer_id, &id)?;
            out.insert(id.as_str().to_string(), a.summary);
        }
        Ok(out)
    }
}

fn looked_in(c: &rusqlite::Connection, viewer_id: &str, aid: &str) -> Result<BTreeMap<String, String>> {
    let mut st = c.prepare("SELECT vt.thread_id, vt.looked_at FROM viewer_threads vt JOIN threads t ON t.id = vt.thread_id WHERE vt.viewer_id = ?1 AND t.artifact_id = ?2")?;
    Ok(st.query_map(params![viewer_id, aid], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?)
}

fn summary(c: &rusqlite::Connection, viewer_id: &str, public_id: &str, aid: &str, looked: &BTreeMap<String, String>) -> Result<AttentionSummary> {
    let mut s = AttentionSummary::default();
    let in_threads: Vec<(String, String)> = c.prepare(IN_THREAD)?.query_map(params![aid, public_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
    for (tid, status) in in_threads {
        let since = looked.get(&tid).map(String::as_str).unwrap_or("");
        let open = status == "open";
        if open { s.open_in.push(tid.clone()); }
        let addressed: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM version_threads WHERE thread_id = ?1 AND created_at >= ?2)", params![tid, since], |r| r.get(0))?;
        if open && addressed {
            let v: Option<u32> = c.query_row("SELECT MAX(version_n) FROM version_threads WHERE thread_id = ?1 AND created_at >= ?2", params![tid, since], |r| r.get(0))?;
            s.addressed_v = s.addressed_v.max(v);
            s.addressed.push(tid.clone());
        }
        let replied: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM comments WHERE thread_id = ?1 AND created_at > ?2 AND (author_public_id IS NULL OR author_public_id != ?3))",
            params![tid, since, public_id], |r| r.get(0))?;
        if replied { s.new_replies.push(tid); }
    }
    s.seen = c.query_row("SELECT seen_n FROM viewer_seen WHERE viewer_id = ?1 AND artifact_id = ?2", params![viewer_id, aid], |r| r.get(0)).optional()?;
    Ok(s)
}
```

Register `pub mod attention;` in `store/mod.rs`. Re-export `Participants`, `Attention`, `AttentionSummary` and `AgentView` from `lib.rs`.

`with_tx` hands a `Transaction`, which derefs to `Connection`, so `looked_in(tx, …)` compiles as written. If the store's helpers take `&Connection` explicitly, pass `&tx`.

- [ ] **Step 4: Implement the routes and the bootstrap**

`crates/clax-server/src/viewer.rs`:

```rust
/// The comment author for `cookie`: the display name as `author_name` gives
/// it, and the viewer's public ID when the cookie names a viewer.
pub fn author(st: &Store, cookie: Option<&str>) -> clax_core::Result<(String, Option<String>)> {
    let v = match cookie { Some(id) => st.get_viewer(id)?, None => None };
    let name = display_name(v.as_ref().and_then(|v| v.display_name.as_deref()).unwrap_or(""));
    Ok((name, v.map(|v| v.public_id)))
}
```

In `routes/threads.rs`, the thread create and viewer comment handlers call `author` in place of `author_name`, and fill `author_public_id`. Agent comments pass `None`.

`routes/artifacts.rs`:
- `with_owner` adds `v["participants"] = json!(st.participants(&a.id)?)`. It now also takes the store and returns `Result<Value>`; it keeps Task 8's `working` parameter. Update its callers: `get`, `list` and `boot.rs`.
- `get` reads the viewer cookie (`crate::viewer::read(&headers)`). With a viewer, it adds `"attention": st.attention(&vid, &id)?` to the body, and answers with `Cache-Control: private, no-cache` and `Vary: Cookie` (the headers `routes/shell.rs` already sets for its viewer-dependent page).

`routes/viewers.rs`:

```rust
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LookedBody { artifact_id: String, thread_ids: Vec<String> }

/// `GET /api/viewers/me/attention`: this viewer's attention on every live
/// artifact, without looked-at times; `{artifacts: {}}` without a cookie.
pub async fn attention(State(s): State<AppState>, viewer: ViewerCookie) -> Result<impl IntoResponse, ApiError> {
    let out = match viewer.0 {
        Some(v) => s.store_call(move |st| Ok(match st.get_viewer(&v)? { Some(_) => json!(st.attention_all(&v)?), None => json!({}) })).await?,
        None => json!({}),
    };
    Ok(([(header::CACHE_CONTROL, "private, no-cache"), (header::VARY, "Cookie")], Json(json!({"artifacts": out}))))
}

/// `PUT /api/viewers/me/looked`: records that this viewer looked at the
/// threads now (spec §10, "Participants and attention"); `{looked}`.
pub async fn set_looked(
    State(s): State<AppState>,
    _o: SameOrigin,
    viewer: ViewerCookie,
    req: Result<Json<LookedBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let b = body(req)?;
    let id = parse_id(&b.artifact_id)?;
    if b.thread_ids.is_empty() || b.thread_ids.len() > clax_core::store::attention::MAX_LOOKED || !b.thread_ids.iter().all(|t| clax_core::is_ulid(t)) {
        return Err(ApiError::bad_request("invalid_args", "thread_ids: 1 to 50 thread IDs"));
    }
    let Some(vid) = viewer.0 else { return Err(ApiError::bad_request("no_viewer", "no viewer cookie")) };
    let looked = s.store_call(move |st| {
        st.get_artifact(&id)?.ok_or(clax_core::CoreError::NotFound)?;
        st.get_viewer(&vid)?.ok_or_else(|| clax_core::CoreError::invalid("no_viewer", "no viewer cookie"))?;
        st.mark_looked(&vid, &id, &b.thread_ids)
    }).await?;
    Ok(Json(json!({"looked": looked})))
}
```

Use the file's existing helpers for parsing the body and the ID; their names may differ from `body` and `parse_id`. `routes/mod.rs`, before `/api/viewers/me`, adds:
- `.route("/api/viewers/me/attention", get(viewers::attention))`;
- `.route("/api/viewers/me/looked", put(viewers::set_looked))`.

`boot.rs::assemble`: the artifact JSON comes from `with_owner`, so it carries `participants`. When `viewer` is `Some`, add `"attention": st.attention(&vid, &id)?` to the block. `without_sessions` keeps stripping `owner_session_id` and the versions' `session_id`. Version views now carry `agent` (a handle), which is safe to embed. Extend `crates/clax-server/tests/shell_boot.rs` to assert that the block has `artifact.artifact.participants`, has `attention` only with a viewer cookie, and contains no session ID.

`web/shell/src/api.ts` and `threads.ts`: add the types from Interfaces, and:

```ts
/** `agents` is ordered live first, then most recently active; `live` means a send can reach it. */
export type Participants = { people: { public_id: string; display_name: string | null; seen: number | null }[]; agents: { handle: string; harness: string; live: boolean }[] };
export type AttentionSummary = { addressed: string[]; addressed_v: number | null; new_replies: string[]; open_in: string[]; seen: number | null };
export type Attention = AttentionSummary & { looked: Record<string, string> };

/** This viewer's attention on every artifact; {} without a viewer or on failure. */
export async function getAttention(): Promise<Record<string, AttentionSummary>> {
  try { const r = await fetch("/api/viewers/me/attention"); return r.ok ? (await r.json()).artifacts : {}; } catch { return {}; }
}
/** Records that this viewer looked at `ids`; failures are ignored (the next look writes again). */
export async function putLooked(aid: string, ids: string[]): Promise<Record<string, string> | null> {
  try {
    const r = await fetch("/api/viewers/me/looked", { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify({ artifact_id: aid, thread_ids: ids }) });
    return r.ok ? (await r.json()).looked : null;
  } catch { return null; }
}
```

`view/boot.ts`: `Boot` gains `attention?: Attention | null`. `readBoot` drops `attention` for `back_forward`, as it drops the viewer.

Token-less artifact views (the B4 ruling):
- In `routes/artifacts.rs`, add:

```rust
/// Leaves out the session IDs a token-less caller must not see (spec §14):
/// the artifact's `owner_session_id` and each version's `session_id`.
pub(crate) fn strip_sessions(v: &mut Value) {
    if let Some(a) = v.get_mut("artifact").and_then(Value::as_object_mut) {
        a.remove("owner_session_id");
    }
    for x in v.get_mut("versions").and_then(Value::as_array_mut).into_iter().flatten() {
        if let Some(x) = x.as_object_mut() { x.remove("session_id"); }
    }
    for x in v.get_mut("artifacts").and_then(Value::as_array_mut).into_iter().flatten() {
        if let Some(x) = x.as_object_mut() { x.remove("owner_session_id"); }
    }
}
```

- `get` and `list` take `headers: HeaderMap` and call `strip_sessions` on the body unless `has_token(&headers, &s.token)`.
- `boot::without_sessions` builds its `{artifact, versions}` and calls `strip_sessions` on it, so the two share one rule.
- `tests/api_sessions.rs` (the `owner_session_id == sid` assertions after `ts.get`, about line 207) and `tests/shell_boot.rs` ("the API names the owner session", about line 125): read with `ts.get_authed` instead, and keep their assertions.
- `docs/follow-ups.md`: remove the bullet **"`GET /api/artifacts/<ID>` names sessions to unauthenticated callers"**, which this fixes.

Test literals (F11): add `author_public_id: None` to every `NewThread { … }` and `NewComment { … }` literal in the workspace, Task 12's `store/changelog.rs` tests included (`grep -rn "NewThread {\|NewComment {" crates`).

The doctor's hard delete of broken artifact rows (`store/artifacts.rs`, the list it deletes before `threads`) also deletes `mentions` (by the thread's comments), `viewer_threads` and `version_threads` (by thread), since those hold foreign keys to the rows it removes.

Run: `cargo test --workspace && cd web && npm run typecheck && npx vitest run`
Expected: PASS. Exact JSON assertions elsewhere gain the following, and nothing else changes:
- comments: `"author_public_id": null`;
- versions: `"agent"` and `"agent_harness"`;
- artifacts: `"participants"`;
- token-less artifact views: no `owner_session_id` or `session_id`.

The bootstrap now runs `participants` and `attention` before the HTML's first byte. Measure it:

Run: `cd web && npm run build && npm run perf; echo "exit=$?"`
Expected: `exit=0`. Stop rule: if any of the five measures in `web/perf/budget.json` is over budget (link to first paint `firstPaint`, link to comment ready `commentReady`, frame paint `framePaint`, ready latency `readyLatency`, cold ready latency `coldLatency`, in either frame mode), stop and report all five numbers against their budgets. The first remedy is one aggregate query per artifact in `summary()` in place of the per-thread queries.

- [ ] **Step 5: Gates and staging**

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add crates/clax-core/src/mentions.rs crates/clax-core/src/store/attention.rs crates/clax-server/tests/api_attention.rs crates/clax-core/src/lib.rs crates/clax-core/src/ids.rs \
  crates/clax-core/src/store/mod.rs crates/clax-core/src/store/migrations.rs crates/clax-core/src/store/threads.rs crates/clax-core/src/store/sessions.rs \
  crates/clax-core/src/store/artifacts.rs crates/clax-core/src/model.rs crates/clax-server/src/viewer.rs crates/clax-server/src/routes/threads.rs \
  crates/clax-server/src/routes/artifacts.rs crates/clax-server/src/routes/viewers.rs crates/clax-server/src/routes/mod.rs crates/clax-server/src/boot.rs \
  crates/clax-server/src/testing.rs crates/clax-server/tests/shell_boot.rs crates/clax-server/tests/api_sessions.rs docs/follow-ups.md \
  web/shell/src/api.ts web/shell/src/threads.ts web/shell/src/view/boot.ts
git add -u crates web/shell/src
git status --short   # staged; the controller commits ("Record comment authors and mentions, give agents opaque handles, compute each viewer's attention, and keep session IDs out of token-less artifact views")
```

---

### Task 16: Working in Echo: the roster, the top bar summary, card chips, thread markers, split pins, and a haiku while working

The working signal in the Echo shell, as designed in "What the person sees when several sessions work at once":
- the roster (people on the left, agents on the right) and its two-line summary;
- the green sweep under the top bar;
- a `claude working on N` chip on gallery cards;
- `claude is working on it` with the elapsed time on thread cards, and a `working on it` event in the history line;
- split pins;
- a sidebar strip with a haiku.

The daemon's working views gain the agent's handle, so two sessions of one harness can be told apart.

**Files:**
- Create: `web/shell/src/view/working-model.ts`, `web/shell/src/view/working-model.test.ts`, `web/shell/src/ui/Roster.svelte`, `web/shell/src/ui/WorkingSummary.svelte`, `web/shell/src/ui/WorkingStrip.svelte`, `web/shell/src/ui/working-feed.svelte.ts`, `web/shell/src/working-ui.test.ts`, `web/e2e/working.spec.ts`, `web/e2e/pages/working-cap.html`
- Modify: `crates/clax-core/src/working.rs` (`Actor.agent`, `WorkingView.agent`), `crates/clax-server/src/routes/working.rs`, `crates/clax-server/src/working.rs`, and the `Actor { … }` literals in their tests; `web/shell/src/api.ts`, `web/shell/src/events.ts`, `web/shell/src/events.test.ts`, `web/shell/src/artifact.ts` (the sweep class), `web/shell/src/view/artifact-controller.ts`, `web/shell/src/view/artifact-controller.test.ts`, `web/shell/src/view/history-model.ts`, `web/shell/src/ui/TopbarIsland.svelte`, `web/shell/src/ui/SidebarIsland.svelte`, `web/shell/src/ui/Sidebar.svelte`, `web/shell/src/ui/ThreadCard.svelte`, `web/shell/src/ui/Pins.svelte`, `web/shell/src/ui/StageIsland.svelte`, `web/shell/src/ui/Gallery.svelte`, `web/shell/src/sidebar.test.ts`, `web/shell/src/gallery.test.ts`, `web/shell/src/theme.css`, `web/e2e/fixtures.ts`, `web/e2e/scenes.ts`

**Interfaces:**
- Rust:
  - `clax_core::working::Actor` gains `pub agent: String`, the session's `agent_handle` from Task 15.
  - `WorkingView` gains `pub agent: String`, so working views name the agent by handle and never by session.
  - The two server constructions (`routes/working.rs::put`, Task 8's PUT handler, and `working.rs::mark_items`) pass `agent: sess.agent_handle`. Test literals add `agent: format!("a_{sid}")`, or a fixed string.
- `view/working-model.ts` (no `svelte` import):
  - `type Working = { key: string; agent: string; harness: string; message: string | null; thread_ids: string[]; started_at: string; last_heartbeat: string }`.
  - `harnessLabel(h)` gives the product name, for the capability.
  - `newestFirst(list)`.
  - `agentNames(list: Working[], agents: AgentView[]): Map<string, string>` maps a handle to a name: the harness, plus the handle's first 4 hex digits when two agents share a harness (decided: Q8).
  - `clock(since: string, now: Date): string` gives `m:ss`, or `h:mm:ss` past an hour.
  - `summary(i: SummaryInput): Summary`, where `SummaryInput = { working: Working[]; names: Map<string, string>; mine: Set<string>; open: number; idle: string[]; addressed: string | null; now: Date }` and `Summary = { line1: string; agent: boolean; line2: string; elapsed: string | null }`.
  - `threadMarker(list, threadId, names): { text: string; since: string } | null`, and `threadAgent(list, threadId, names): string | null` (the name alone).
  - `stripText(w: Working, names, numbers: Map<string, number>, mine: Set<string>): string`.
  - `chips(list: Working[], names): string[]`.
  - `workingThreads(list): Set<string>`.
- `events.ts`: the `working` member of `ArtifactEvent`, and `subscribeWorking(onEvent)`, the gallery's `?types=working` stream.
- `ArtifactController`: `ViewState.working: Working[]` (initially `[]`).
- `ui/working-feed.svelte.ts`: `class WorkingFeed { byId; seed(list); start(onResync); stop() }`.
- Components:
  - `Roster` `{ people; agents; working: Working[]; me: string | null; max: number; small?: boolean }`.
  - `WorkingSummary` `{ s: Summary }`.
  - `WorkingStrip` `{ w: Working; text: string; commenting: boolean }`.
  - `ThreadCard` gains `marker?: { text: string; since: string } | null` (it already takes `now`).
  - `Pins` gains `onit?: Set<string>`.

- [ ] **Step 1: The daemon names the agent in working views**

In `crates/clax-core/src/working.rs`, add `pub agent: String` to `Actor` and to `WorkingView`. The record keeps the actor, so the view copies `agent` from it. Fill it in the two server constructions from `sess.agent_handle`. In `views_are_newest_first_and_carry_no_session_id`, also assert `v.agent.starts_with("a_")` once the actor literals carry `a_…` handles.

Run: `cargo test -p clax-core working && cargo test -p clax-server --test api_working`
Expected: PASS, with exact view JSON in those tests gaining `"agent"`.

- [ ] **Step 2: The model, test first**

`web/shell/src/view/working-model.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { agentNames, chips, clock, stripText, summary, threadMarker, type Working } from "./working-model";

const w = (over: Partial<Working>): Working => ({
  key: "k", agent: "a_1111aaaa", harness: "claude", message: null, thread_ids: [], started_at: "2026-09-30T10:00:00.000Z", last_heartbeat: "2026-09-30T10:00:00.000Z", ...over,
});
const now = new Date("2026-09-30T10:00:42.000Z");
const agents = [{ handle: "a_1111aaaa", harness: "claude", live: true }, { handle: "a_2222bbbb", harness: "codex", live: true }];

describe("working-model", () => {
  it("names agents by harness, and tells two of one harness apart", () => {
    expect(agentNames([w({})], agents).get("a_1111aaaa")).toBe("claude");
    const two = agentNames([w({}), w({ agent: "a_3333cccc" })], [...agents, { handle: "a_3333cccc", harness: "claude", live: true }]);
    expect([two.get("a_1111aaaa"), two.get("a_3333cccc")]).toEqual(["claude 1111", "claude 3333"]);
  });

  it("reads the summary for one agent, all yours, some yours, a message, and nobody", () => {
    const names = agentNames([w({})], agents);
    const base = { names, open: 3, idle: [], addressed: null, now };
    expect(summary({ ...base, working: [w({ thread_ids: ["a", "b"] })], mine: new Set(["a", "b"]) }))
      .toEqual({ line1: "claude working on 2", agent: true, line2: "all yours", elapsed: "0:42" });
    expect(summary({ ...base, working: [w({ thread_ids: ["a", "b"] })], mine: new Set(["a"]) }).line2).toBe("1 yours");
    expect(summary({ ...base, working: [w({ message: "Rebuilding the chart" })], mine: new Set() }).line1).toBe("claude: Rebuilding the chart");
    expect(summary({ ...base, working: [], mine: new Set(), idle: ["codex"] })).toEqual({ line1: "Nobody working", agent: false, line2: "3 open threads · codex idle", elapsed: null });
    expect(summary({ ...base, working: [], mine: new Set(), addressed: "v5 addressed 3" })).toEqual({ line1: "v5 addressed 3", agent: false, line2: "yours, not looked at yet", elapsed: null });
  });

  it("lists several agents and counts distinct threads", () => {
    const list = [w({ thread_ids: ["a", "b"] }), w({ key: "k2", agent: "a_2222bbbb", harness: "codex", thread_ids: ["b", "c"], started_at: "2026-09-30T10:00:10.000Z" })];
    expect(summary({ working: list, names: agentNames(list, agents), mine: new Set(), open: 3, idle: [], addressed: null, now }).line1).toBe("codex, claude working on 3");
    expect(chips(list, agentNames(list, agents))).toEqual(["codex working on 2", "claude working on 2"]);
  });

  it("marks a thread and writes the strip", () => {
    const list = [w({ thread_ids: ["t1", "t3"] })];
    const names = agentNames(list, agents);
    expect(threadMarker(list, "t1", names)).toEqual({ text: "claude is working on it", since: "2026-09-30T10:00:00.000Z" });
    expect(threadMarker(list, "t2", names)).toBeNull();
    expect(stripText(list[0], names, new Map([["t1", 1], ["t3", 3]]), new Set(["t1"]))).toBe("claude is working on #1 (yours) and #3");
    expect(stripText(w({ thread_ids: ["x", "y", "z", "q"] }), names, new Map(), new Set())).toBe("claude is working on 4 threads");
  });

  it("formats the clock", () => {
    expect([clock("2026-09-30T10:00:00.000Z", now), clock("2026-09-30T08:59:00.000Z", now)]).toEqual(["0:42", "1:01:42"]);
  });
});
```

Run: `cd web && npx vitest run shell/src/view/working-model.test.ts`
Expected: FAIL.

`web/shell/src/view/working-model.ts`:

```ts
// The working signal as Echo shows it (spec §8, "Working"): pure functions
// over the daemon's working views, shared by the top bar, the gallery, the
// sidebar, the pins and the capability.
import type { Participants } from "../api";

export type Working = { key: string; agent: string; harness: string; message: string | null; thread_ids: string[]; started_at: string; last_heartbeat: string };
export type AgentView = Participants["agents"][number];
export type SummaryInput = { working: Working[]; names: Map<string, string>; mine: Set<string>; open: number; idle: string[]; addressed: string | null; now: Date };
export type Summary = { line1: string; agent: boolean; line2: string; elapsed: string | null };

const LABELS: Record<string, string> = { claude: "Claude Code", codex: "Codex", pi: "Pi" };
/** The harness as a product name ("Claude Code"), for the page capability. */
export const harnessLabel = (h: string): string => LABELS[h] ?? h;

export function newestFirst(list: Working[]): Working[] {
  return [...list].sort((a, b) => b.started_at.localeCompare(a.started_at) || b.key.localeCompare(a.key));
}

/** Handle → name: the harness, with the handle's first four hex digits when two agents share it. */
export function agentNames(list: Working[], agents: AgentView[]): Map<string, string> {
  const all = new Map<string, string>(agents.map(a => [a.handle, a.harness]));
  for (const w of list) if (!all.has(w.agent)) all.set(w.agent, w.harness);
  const count = new Map<string, number>();
  for (const h of all.values()) count.set(h, (count.get(h) ?? 0) + 1);
  return new Map([...all].map(([handle, h]) => [handle, (count.get(h) ?? 0) > 1 ? `${h} ${handle.slice(2, 6)}` : h]));
}

export function clock(since: string, now: Date): string {
  const s = Math.max(0, Math.floor((now.getTime() - new Date(since).getTime()) / 1000));
  const two = (n: number) => String(n).padStart(2, "0");
  return s >= 3600 ? `${Math.floor(s / 3600)}:${two(Math.floor(s / 60) % 60)}:${two(s % 60)}` : `${Math.floor(s / 60)}:${two(s % 60)}`;
}

export const workingThreads = (list: Working[]) => new Set(list.flatMap(w => w.thread_ids));
const plural = (n: number, w: string) => `${n} ${w}${n === 1 ? "" : "s"}`;

export function summary(i: SummaryInput): Summary {
  const idle = i.idle.length ? ` · ${i.idle.join(", ")} idle` : "";
  const list = newestFirst(i.working);
  if (list.length) {
    const who = [...new Set(list.map(w => i.names.get(w.agent) ?? w.harness))].join(", ");
    const threads = workingThreads(list);
    const line1 = list.length === 1 && list[0].message ? `${who}: ${list[0].message}` : threads.size ? `${who} working on ${threads.size}` : `${who} working`;
    const mine = [...threads].filter(t => i.mine.has(t)).length;
    const line2 = (mine && mine === threads.size ? "all yours" : mine ? `${mine} yours` : plural(i.open, "open thread")) + idle;
    return { line1, agent: true, line2, elapsed: clock(list[0].started_at, i.now) };
  }
  if (i.addressed) return { line1: i.addressed, agent: false, line2: "yours, not looked at yet", elapsed: null };
  return { line1: "Nobody working", agent: false, line2: plural(i.open, "open thread") + idle, elapsed: null };
}

export function threadMarker(list: Working[], threadId: string, names: Map<string, string>): { text: string; since: string } | null {
  const w = newestFirst(list).find(x => x.thread_ids.includes(threadId));
  return w ? { text: `${names.get(w.agent) ?? w.harness} is working on it`, since: w.started_at } : null;
}

export function threadAgent(list: Working[], threadId: string, names: Map<string, string>): string | null {
  const w = newestFirst(list).find(x => x.thread_ids.includes(threadId));
  return w ? names.get(w.agent) ?? w.harness : null;
}

export function stripText(w: Working, names: Map<string, string>, numbers: Map<string, number>, mine: Set<string>): string {
  const who = names.get(w.agent) ?? w.harness;
  if (w.message) return `${who}: ${w.message}`;
  if (!w.thread_ids.length) return `${who} is working`;
  const numbered = w.thread_ids.filter(t => numbers.has(t));
  if (numbered.length !== w.thread_ids.length || numbered.length > 3) return `${who} is working on ${plural(w.thread_ids.length, "thread")}`;
  const parts = numbered.map(t => `#${numbers.get(t)}${mine.has(t) ? " (yours)" : ""}`);
  return `${who} is working on ${parts.length > 1 ? `${parts.slice(0, -1).join(", ")} and ${parts.at(-1)}` : parts[0]}`;
}

export function chips(list: Working[], names: Map<string, string>): string[] {
  return newestFirst(list).map(w => (w.thread_ids.length ? `${names.get(w.agent) ?? w.harness} working on ${w.thread_ids.length}` : `${names.get(w.agent) ?? w.harness} working`));
}
```

Run: `cd web && npx vitest run shell/src/view/working-model.test.ts`
Expected: PASS.

- [ ] **Step 3: Types, events, controller**

- `api.ts`: `import type { Working } from "./view/working-model";`. Add to `Artifact`: `/** Who is working on it now (never a session ID). */ working?: Working[];`.
- `events.ts`:
  - add `| { type: "working"; artifact_id: string; working: Working[] }` to `ArtifactEvent`, and `"working"` to `subscribe`'s listener names;
  - add `subscribeWorking`, which opens `/api/events?types=working`, listens for `working`, `ready`, `error` (as `stream_down`) and `resync`, and is a no-op when `EventSource` is undefined. Write it in `subscribe`'s style;
  - in `events.test.ts`, extend the listener-name test, and add one for `subscribeWorking`'s URL and its parse.
- `view/artifact-controller.ts`:
  - `ViewState.working: Working[]`, initially `[]`;
  - every `this.set({ data: … })` (load and bootstrap) also sets `working: d.artifact.working ?? []`;
  - `onEvent` handles `working` with `this.set({ working: e.working })`;
  - the resync refetch sets it too;
  - `working` is not part of `react()`: a working change never re-resolves anchors or refocuses the frame.
- `view/artifact-controller.test.ts`: using `FakeES.emit(name, data)` (the harness already has it), add a test. The artifact's `working` seeds the state, and a `working` event replaces it without changing `data`.
- `view/history-model.ts`: `historyOf(t, versions, names, more: { working?: string | null } = {})` appends `{ v: null, who: more.working, agent: true, verb: "working on it" }` when `more.working` is set and the thread is open.
- `artifact.ts` `pageFollows` toggles `working` on `sk.topbar` when `s.working.length > 0`.

`web/shell/src/ui/working-feed.svelte.ts`:

```ts
// The gallery's live working lists, by artifact: seeded from
// `GET /api/artifacts`, then kept current by the `working` event stream,
// which opens only after the gallery has rendered.
import type { Artifact } from "../api";
import { subscribeWorking } from "../events";
import type { Working } from "../view/working-model";

export class WorkingFeed {
  byId = $state<Record<string, Working[]>>({});
  #stop: (() => void) | null = null;
  seed(list: Artifact[]): void { this.byId = Object.fromEntries(list.map(a => [a.id, a.working ?? []])); }
  start(onResync: () => void): void {
    if (this.#stop) return;
    this.#stop = subscribeWorking(e => {
      if (e.type === "working") this.byId = { ...this.byId, [e.artifact_id]: e.working };
      else if (e.type === "ready" || e.type === "resync") onResync();
    });
  }
  stop(): void { this.#stop?.(); this.#stop = null; }
}
```

- [ ] **Step 4: Components, test first**

`web/shell/src/working-ui.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { mount } from "./test/svelte";
import Roster from "./ui/Roster.svelte";
import WorkingSummary from "./ui/WorkingSummary.svelte";

const agents = [{ handle: "a_1111aaaa", harness: "claude", live: true }, { handle: "a_2222bbbb", harness: "codex", live: true }];
const people = [{ public_id: "u_me", display_name: "alex" }, { public_id: "u_mia", display_name: "Mia Kovač" }];

describe("working UI", () => {
  it("the roster puts people left and agents right, the viewer nearest the centre, a working agent solid", () => {
    const working = [{ key: "k", agent: "a_1111aaaa", harness: "claude", message: null, thread_ids: [], started_at: "s", last_heartbeat: "s" }];
    const m = mount(Roster, { people, agents, working, me: "u_me", max: 5 });
    const ppl = Array.from(m.root.querySelectorAll(".ppl .tok")).map(e => e.textContent);
    expect(ppl).toEqual(["AL", "MK"]);
    expect(m.root.querySelector(".ppl .tok.me")!.textContent).toBe("AL");
    expect(Array.from(m.root.querySelectorAll(".agt .tok")).map(e => [e.textContent, e.classList.contains("work")])).toEqual([["cl", true], ["cx", false]]);
    m.unmount();
  });

  it("the roster overflows past max with a +n token", () => {
    const many = Array.from({ length: 7 }, (_, i) => ({ public_id: `u_${i}`, display_name: `P${i} Q` }));
    const m = mount(Roster, { people: many, agents: [], working: [], me: null, max: 3, small: true });
    expect(m.root.querySelector(".ppl .tok.more")!.textContent).toBe("+4");
    m.unmount();
  });

  it("the summary is a polite live region whose elapsed time sits outside it", () => {
    const m = mount(WorkingSummary, { s: { line1: "claude working on 2", agent: true, line2: "all yours", elapsed: "0:42" } });
    const live = m.root.querySelector("[role=status]")!;
    expect(live.getAttribute("aria-live")).toBe("polite");
    expect(live.textContent).toBe("claude working on 2all yours");
    expect(m.root.querySelector(".el")!.textContent).toBe("0:42");
    expect(live.contains(m.root.querySelector(".el"))).toBe(false);
    m.unmount();
  });
});
```

`web/shell/src/ui/Roster.svelte`:

```svelte
<script lang="ts">
  // Participants as Echo draws them (spec §8): people open toward the centre
  // from the left in red-orange, agents from the right in green, the page a
  // dot between. The viewer is nearest the centre and underlined.
  import type { Participants } from "../api";
  import type { Working } from "../view/working-model";

  type Props = { people: Participants["people"]; agents: Participants["agents"]; working: Working[]; me: string | null; max: number; small?: boolean; presence?: Record<string, "here" | "away"> };
  let { people, agents, working, me, max, small = false, presence = {} }: Props = $props();
  const initials = (n: string | null) => {
    const w = (n ?? "?").trim().split(/\s+/);
    return (w.length > 1 ? w[0][0] + w[1][0] : w[0].slice(0, 2)).toUpperCase();
  };
  const AGENT: Record<string, string> = { claude: "cl", codex: "cx", pi: "pi" };
  const ordered = $derived([...people].sort((a, b) => Number(b.public_id === me) - Number(a.public_id === me)));
  const busy = $derived(new Set(working.map(w => w.agent)));
</script>

<span class={["ros", small && "sm"]}>
  <span class="side ppl">
    {#each ordered.slice(0, max) as p (p.public_id)}
      <span class={["tok", "p", p.public_id === me && "me", presence[p.public_id]]} title={p.display_name ?? "Viewer"}>{initials(p.display_name)}</span>
    {/each}
    {#if ordered.length > max}<span class="tok more">+{ordered.length - max}</span>{/if}
  </span>
  <span class="hub" aria-hidden="true"></span>
  <span class="side agt">
    {#each agents.slice(0, max) as a (a.handle)}
      <span class={["tok", "a", busy.has(a.handle) && "work"]} title={a.harness}>{AGENT[a.harness] ?? a.harness.slice(0, 2)}</span>
    {/each}
    {#if agents.length > max}<span class="tok more">+{agents.length - max}</span>{/if}
  </span>
</span>
```

`web/shell/src/ui/WorkingSummary.svelte`:

```svelte
<script lang="ts">
  import type { Summary } from "../view/working-model";
  let { s }: { s: Summary } = $props();
</script>

<span class="sum">
  <span role="status" aria-live="polite" aria-atomic="true"><b class={["l1", s.agent && "ag"]}>{s.line1}</b><small class="l2">{s.line2}</small></span>{#if s.elapsed}<small class="el" aria-hidden="true">{s.elapsed}</small>{/if}
</span>
```

`web/shell/src/ui/WorkingStrip.svelte`:

```svelte
<script lang="ts">
  import type { Working } from "../view/working-model";
  import HaikuLine from "./HaikuLine.svelte";
  let { w, text, commenting }: { w: Working; text: string; commenting: boolean } = $props();
</script>

<div class="strip ag">
  <span class="tok a work" aria-hidden="true">{w.harness.slice(0, 2)}</span><b>{text}</b>
  {#if !commenting}<HaikuLine seed={w.key} />{/if}
</div>
```

`HaikuLine` loads its list lazily (Task 6). `WorkingStrip` itself loads by dynamic `import()` from the sidebar, only while someone works.

- [ ] **Step 5: Wire the islands, the sidebar, the pins and the gallery**

- `TopbarIsland.svelte`: replace `<div class="who-slot"></div>` with:

```svelte
  {@const parts = s.data.artifact.participants ?? { people: [], agents: [] }}
  {@const names = agentNames(s.working, parts.agents)}
  {@const busy = new Set(s.working.map(w => w.agent))}
  <div class="who">
    <Roster people={parts.people} agents={parts.agents} working={s.working} me={s.me?.public_id ?? null} max={s.narrow ? 1 : 5} />
    <WorkingSummary s={summary({ working: s.working, names, mine: new Set(s.attention?.open_in ?? []), open: ctl.openCount(s), now: tick.now,
      idle: parts.agents.filter(a => a.live && !busy.has(a.handle)).map(a => names.get(a.handle) ?? a.harness), addressed: null })} />
  </div>
```

  `tick` is the port's `ticker` (`ui/ticker.svelte.ts`), ticking each second while `s.working.length > 0`. Task 18 passes `addressed`.
- `ViewState` gains `attention: Attention | null`. It is set from `boot.attention`, else from the `attention` key of `getArtifact`'s response, which carries it whenever the request carries the viewer cookie (Task 15). `getArtifact`'s return type and `Loaded` gain `attention?: Attention`. Every refetch refreshes it.
- `SidebarIsland.svelte` passes `working={s.working}`, `commenting={s.commenting}`, `agents={s.data.artifact.participants?.agents ?? []}` and `mine={s.attention?.open_in ?? []}` to `Sidebar`, which declares the four props.
- `Sidebar.svelte`:
  - computes `names` from `agentNames(p.working ?? [], p.agents ?? [])`;
  - passes `marker={t.status === "open" ? threadMarker(p.working ?? [], t.id, names) : null}` to each card;
  - passes `{ working: t.status === "open" ? threadAgent(p.working ?? [], t.id, names) : null }` to `historyOf`;
  - renders the strip first, inside the aside, after the header, when anyone works. The `{#await}` keeps `WorkingStrip` and `HaikuLine` out of the entry:

```svelte
  {#each newestFirst(p.working ?? []) as w (w.key)}
    {#await import("./WorkingStrip.svelte") then { default: WorkingStrip }}
      <WorkingStrip {w} text={stripText(w, names, s.numbers, new Set(p.mine ?? []))} commenting={p.commenting ?? false} />
    {/await}
  {/each}
```

- `ThreadCard.svelte`:
  - adds `marker?: { text: string; since: string } | null`;
  - in place of the waiting line, while a marker shows: `<p class="st ag"><span class="tok a work sm" aria-hidden="true"></span>{marker.text}<small>{clock(marker.since, now)}</small></p>`;
  - otherwise the waiting line as before.
- `Pins.svelte`: adds `onit?: Set<string>`, and puts `class:onit={onit?.has(p.thread.id)}` on each pin. `StageIsland` passes `onit={workingThreads(s.working)}`.
- `Gallery.svelte`:
  - a `WorkingFeed`: seeded and started after the first list renders, and stopped in `onDestroy`;
  - each `GalleryCard` gets a `markers` snippet listing `chips(feed.byId[a.id] ?? [], agentNames(feed.byId[a.id] ?? [], a.participants?.agents ?? []))` as `<span class="chip ag">…</span>`;
  - each card gets a `footer` snippet rendering `<Roster people={a.participants?.people ?? []} agents={a.participants?.agents ?? []} working={feed.byId[a.id] ?? []} me={meId} max={3} small />`, where `meId` comes from the gallery's `getViewer()`.

Tests:
- in `sidebar.test.ts`, a thread named by a record shows `.st.ag` reading `claude is working on it` and the clock, and a history event `claude working on it`. When the record drops it, the waiting line returns;
- in `gallery.test.ts`, the first card shows `.chip.ag` reading `claude working`, and the second shows none.

- [ ] **Step 6: Styles**

Append to `web/shell/src/theme.css`:

```css
/* Participants and the working signal (spec §8, "Working"). */
.ros { display: flex; align-items: center; gap: 2px; }
.ros .side { display: flex; gap: 2px; } .ros .ppl { flex-direction: row-reverse; }
.ros .hub { width: 7px; height: 7px; border-radius: 50%; background: var(--fg); margin: 0 4px; flex: none; }
.tok { display: inline-grid; place-items: center; height: 26px; min-width: 30px; padding: 0 6px; font: 600 12px/1 var(--grot); flex: none; position: relative; }
.tok.p { border-radius: 0 13px 13px 0; padding-right: 8px; background: var(--card); box-shadow: inset 0 0 0 1.5px var(--you); color: var(--fg); }
.tok.p.me::after { content: ""; position: absolute; left: 4px; right: 8px; bottom: 3px; height: 1.5px; background: currentColor; }
.tok.p.away { opacity: .5; }
.tok.p.here::before { content: ""; position: absolute; right: 2px; top: 2px; width: 5px; height: 5px; border-radius: 50%; background: var(--agent); box-shadow: 0 0 0 1.5px var(--card); }
.tok.a { border-radius: 13px 0 0 13px; padding-left: 8px; background: var(--card); box-shadow: inset 0 0 0 1.5px var(--agent); color: var(--agent-ink); }
.tok.a.work { background: var(--agent); color: var(--on-accent); box-shadow: none; padding-left: 13px; }
.tok.a.work::before { content: ""; position: absolute; left: 5px; top: 50%; width: 4px; height: 4px; margin-top: -2px; border-radius: 50%; background: currentColor; animation: breathe 1.8s ease-in-out infinite; }
.tok.more { background: none; box-shadow: none; color: var(--muted); min-width: 0; padding: 0 3px; }
.ros.sm .tok, .tok.sm { height: 20px; min-width: 24px; font-size: 11px; padding: 0 5px; }
@keyframes breathe { 50% { opacity: .3; } }
.who { display: flex; align-items: center; gap: 10px; height: 40px; padding: 0 10px 0 6px; border: 1px solid var(--border-strong); background: var(--bg); flex: none; min-width: 0; }
.who .sum { display: flex; align-items: center; gap: 8px; min-width: 0; }
.who .sum b { display: block; font: 600 14px/1.1 var(--grot); white-space: nowrap; } .who .sum b.ag { color: var(--agent-ink); }
.who .sum small { display: block; font: 400 11px/1.3 var(--mono); color: var(--muted); white-space: nowrap; }
.topbar.working::after { content: ""; position: absolute; left: 0; bottom: -1px; height: 2px; width: 20%; background: var(--agent); animation: sweep 2.4s cubic-bezier(.4,0,.2,1) infinite; }
@keyframes sweep { from { transform: translateX(-100%); } to { transform: translateX(500%); } }
.st.ag { color: var(--agent-ink); font: 600 13.5px/1.2 var(--grot); } .st small { font: 400 11.5px var(--mono); color: var(--muted); margin-left: auto; }
.strip { margin: 0 0 4px; padding: 10px 12px; background: var(--card); border: 1px solid var(--border); display: grid; grid-template-columns: auto 1fr; gap: 3px 10px; align-items: center; }
.strip.ag { box-shadow: inset 3px 0 0 var(--agent); } .strip b { font: 600 15px/1.2 var(--grot); color: var(--agent-ink); } .strip .hk { grid-column: 2; }
.thread-pin.onit { background: linear-gradient(90deg, var(--you) 50%, var(--agent) 50%); color: #fff; text-shadow: 0 0 2px #2f0b04; }
.chip.ag { background: var(--agent); color: var(--on-accent); }
.card .ft .ros { margin-right: auto; }
@media (max-width: 700px) { .who { height: 36px; padding: 0 6px 0 3px; gap: 0; } .who .sum { display: none; } }
@media (prefers-reduced-motion: reduce) { .topbar.working::after { animation: none; display: none; } .tok.a.work::before { animation: none; } }
```

- [ ] **Step 7: Browser tests**

Add to `web/e2e/fixtures.ts` `setWorking(base, token, sid, aid, body)` (`PUT /api/sessions/<sid>/working/<aid>`) and `skewWorking(base, token, secs)` (the debug build's `POST /api/_test/working/skew`).

`web/e2e/pages/working-cap.html`: the page that prints `working()` and `onWorking` state, as in Task 17's test. Its script:

```html
<!doctype html><title>Working cap</title>
<main><h2 id="h">Goals</h2><p id="state">none</p><button id="add">Add</button></main>
<script>
  (async () => {
    const c = await window.claude.use("comments");
    const out = document.getElementById("state");
    const show = s => { out.textContent = s.working ? s.agents.map(a => `${a.label}|${a.message}|${a.threads.length}|${a.otherThreads}`).join(",") : "none"; };
    show(await c.working());
    await c.onWorking(show);
    document.getElementById("add").onclick = async () => { const r = await c.create({ anchor: { path: "#h" }, text: "from the page" }); document.body.dataset.handle = r.threadId; };
  })();
</script>
```

`web/e2e/working.spec.ts`:

```ts
import { test, expect } from "@playwright/test";
import { api, openArtifact, postThread, publishAs, registerSession, setWorking, skewWorking, startDaemon } from "./fixtures";

let d: Awaited<ReturnType<typeof startDaemon>>;
test.beforeAll(async () => { test.setTimeout(180_000); d = await startDaemon(); });
test.afterAll(async () => { await d?.stop(); });
const PAGE = "<main><h2>Quarterly goals</h2></main>";

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: the summary, roster, marker, pin and gallery chip follow the working record`, async ({ page }) => {
    const s = await registerSession(d.base, d.token, "claude", `work-${mode}`);
    const { artifact } = await publishAs(d.base, d.token, s.id, `Working ${mode}`, { "index.html": PAGE });
    const t = await postThread(d.base, artifact.id, "@agent two columns");
    await openArtifact(page, d.base, artifact.id, 1, mode);
    const line1 = page.locator(".who .sum b.l1");
    await expect(line1).toHaveText("Nobody working");
    await expect(page.locator(".who [role=status]")).toHaveAttribute("aria-live", "polite");
    await api(d.base, d.token, `/api/sessions/${s.id}/feedback?tier=piggyback`);
    await expect(line1).toHaveText("claude working on 1");
    await expect(page.locator(".who .agt .tok.work")).toHaveCount(1);
    await expect(page.locator(".topbar")).toHaveClass(/working/);
    if (!(await page.locator("aside.sidebar").isVisible())) await page.getByRole("button", { name: /Threads/ }).first().click();
    const card = page.locator(`.thread-card[data-thread="${t.id}"]`);
    await expect(card.locator(".st.ag")).toContainText("claude is working on it");
    await expect(card.locator(".hist")).toContainText("claude working on it");
    await expect(page.locator(".thread-pin.onit")).toHaveCount(1);
    await setWorking(d.base, d.token, s.id, artifact.id, { message: "Two columns" });
    await expect(line1).toHaveText("claude: Two columns");
    const gallery = await page.context().newPage();
    await gallery.goto(`${d.base}/`);
    await expect(gallery.locator(".card-wrap", { hasText: `Working ${mode}` }).locator(".chip.ag")).toHaveText("claude working on 1");
    await api(d.base, d.token, `/api/artifacts/${artifact.id}/threads/${t.id}/comments`, { method: "POST", session: s.id, body: JSON.stringify({ body: "Done.", author_kind: "agent" }) });
    await expect(card.locator(".st.ag")).toHaveCount(0);
    await expect(line1).toHaveText("Nobody working");
    await expect(gallery.locator(".card-wrap", { hasText: `Working ${mode}` }).locator(".chip.ag")).toHaveCount(0, { timeout: 10_000 });
  });

  test(`${mode}: a record lapses 2 minutes after its last renewal`, async ({ page }) => {
    const s = await registerSession(d.base, d.token, "pi", `lapse-${mode}`);
    const { artifact } = await publishAs(d.base, d.token, s.id, `Lapse ${mode}`, { "index.html": PAGE });
    await setWorking(d.base, d.token, s.id, artifact.id, { message: "Tidying" });
    await openArtifact(page, d.base, artifact.id, 1, mode);
    await expect(page.locator(".who .sum b.l1")).toHaveText("pi: Tidying");
    await skewWorking(d.base, d.token, 121);
    await expect(page.locator(".who .sum b.l1")).toHaveText("Nobody working");
  });
}

test("at phone width in dark mode with reduced motion the roster shrinks and nothing moves", async ({ page }) => {
  const s = await registerSession(d.base, d.token, "claude", "phone-work");
  const { artifact } = await publishAs(d.base, d.token, s.id, "A long title for a phone-width working check", { "index.html": PAGE });
  await setWorking(d.base, d.token, s.id, artifact.id, { message: "Rebuilding the quarterly chart with the new numbers" });
  await page.setViewportSize({ width: 390, height: 844 });
  await page.emulateMedia({ colorScheme: "dark", reducedMotion: "reduce" });
  await openArtifact(page, d.base, artifact.id, 1, "subdomain");
  await expect(page.locator(".who .agt .tok.work")).toBeVisible();
  await expect(page.locator(".who .sum")).toBeHidden();
  expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
  expect(await page.locator(".who .tok.work").evaluate(e => getComputedStyle(e, "::before").animationName)).toBe("none");
});
```

Run: `cd web && npx vitest run && npm run lint && npm run typecheck && npm run build && node scripts/bundle-size.mjs && npx playwright test e2e/working.spec.ts; echo "exit=$?"`
Expected: `exit=0`. `WorkingStrip` is outside `artifact.html`'s closure. If the `artifact` or `gallery` budget fails, first move what first paint does not need behind a dynamic `import()` (the strip's text, the roster's overflow, `working-model`'s clock), then stop and report the sizes. Never raise the budget.

- [ ] **Step 8: Screenshots and a look**

Append to `web/e2e/scenes.ts` a `working` scene: `prepare` calls `setWorking(s.base, s.token, s.sid, s.aid, { thread_ids: s.threads })`, reloads, and opens the panel.

Run: `cd web && CLAX_SHOTS=task-16 CLAX_SCENES=gallery,view,working npx playwright test e2e/shots.spec.ts`
Expected: PASS.

Report, against `concept-3-echo/shots/*-working.png`:
- the roster with the solid green agent token;
- `claude working on 3` over `all yours` and the clock (here 0:0x);
- the sweep is absent in the reduced-motion shots;
- the strip with its haiku;
- the split pins;
- the markers on each card;
- the gallery chip;
- the phone bar with one token per side.

Run: `cd web && npm run perf; echo "exit=$?"`
Expected: `exit=0`. Stop rule: if any of the five measures in `web/perf/budget.json` is over budget (link to first paint `firstPaint`, link to comment ready `commentReady`, frame paint `framePaint`, ready latency `readyLatency`, cold ready latency `coldLatency`, in either frame mode), stop and report all five numbers against their budgets. Lazy-load first; never raise a budget.

- [ ] **Step 9: Gates and staging**

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add crates/clax-core/src/working.rs crates/clax-server/src/routes/working.rs crates/clax-server/src/working.rs web/shell/src/view/working-model.ts \
  web/shell/src/view/working-model.test.ts web/shell/src/ui/Roster.svelte web/shell/src/ui/WorkingSummary.svelte web/shell/src/ui/WorkingStrip.svelte \
  web/shell/src/ui/working-feed.svelte.ts web/shell/src/working-ui.test.ts web/e2e/working.spec.ts web/e2e/pages/working-cap.html web/e2e/fixtures.ts web/e2e/scenes.ts
git add -u crates web/shell/src
git status --short   # staged; the controller commits ("Show who is working in Echo: the roster and summary, card chips, thread markers, split pins and a haiku")
```

---

### Task 17: The page capability: `working()` and `onWorking(fn)`

The page reads who is working through the `comments` capability (a Clax extension, marked as such). This needs no UI change: the page's own display is the page's business. The files under `web/contract/0.2.61/` are claude.ai's type definitions, byte for byte, and stay untouched. Clax's additions are declared in `web/contract/clax-extensions.d.ts` (`namespace ClaxExtensions`), which the daemon serves beside them.

**Files:**
- Modify: `web/contract/clax-extensions.d.ts`, `web/bridge/src/capabilities.ts` (`CAPABILITY_METHODS.comments`), `web/bridge/test/capabilities.test.ts`, `web/shell/src/caps/host.ts` (`CapEnv.working`), `web/shell/src/caps/comments.ts`, `web/shell/src/caps/comments.test.ts`, `web/bridge/src/caps/comments.ts`, `web/bridge/test/comments.test.ts`, `web/shell/src/view/artifact-controller.ts` (passes `working` into the host env), `web/e2e/working.spec.ts`

**Interfaces:**
- Produces, in `web/contract/clax-extensions.d.ts` inside `declare namespace ClaxExtensions`, after `CommentsErrorCode`:

```ts
  /** One agent session working on this artifact now (`comments.working()`). */
  interface WorkingAgent {
    /** `"claude"`, `"codex"`, `"pi"`, or another harness name. */
    harness: string;
    /** The harness as people read it ("Claude Code"). */
    label: string;
    /** The agent's own words, at most 140 characters; treat as untrusted text. */
    message: string | null;
    /** When it started (ISO 8601). */
    since: string;
    /** Handles of threads THIS document created that the agent is acting on. */
    threads: string[];
    /** How many other threads it is acting on. */
    otherThreads: number;
  }

  /** What `working()` resolves and `onWorking` reports. */
  interface WorkingState {
    working: boolean;
    /** Newest first. */
    agents: WorkingAgent[];
  }

  /**
   * Members Clax adds to the `comments` namespace (`comments.d.ts`
   * `interface Comments`). Granted under either declaration form, including
   * `composer_only` (which otherwise grants only openComposer and
   * anchorFor), with no consent prompt and no gesture. They never name a
   * session or a thread's store ID.
   */
  interface Comments {
    /** Which agents are working on this artifact now. Read-only. */
    working(): Promise<WorkingState>;
    /** Calls `fn` with the current state, then on every change. Resolves a function that stops the calls. */
    onWorking(fn: (state: WorkingState) => void): Promise<() => void>;
  }
```

- `web/bridge/src/capabilities.ts`: `CAPABILITY_METHODS.comments` gains `"working", "onWorking"`. `buildNamespace` exposes only listed members, so without this the page's `c.working` is undefined. It is the eager bridge; the two strings cost a few bytes.
- `web/bridge/test/capabilities.test.ts`: the `comments` row expects the `0.2.61` members plus the members of `interface Comments` in `clax-extensions.d.ts`. Read that file as the test reads the contract files, and take `members(ext, "interface Comments {")`. The expected list is the sorted union.
- Shell handler methods: `working` → `WorkingState`; `watchWorking` → `null` (starts pushes on topic `working`); `unwatchWorking` → `null`.
- `CapEnv` gains an optional `working?(): Working[]`, so a `CapEnv` built outside the controller still type-checks. The handler reads `env.working?.() ?? []`.
- Pure helper in `caps/comments.ts`: `pageWorking(list: Working[], handleOf: (id: string) => string | undefined): WorkingState`.

- [ ] **Step 1: Failing shell handler tests**

In `web/shell/src/caps/comments.test.ts`, give the harness a working list. Add at module level `let workingList: Working[] = [];` (with `import type { Working } from "../view/working-model";`), reset it in the `beforeEach` (`workingList = [];`), and add `working: () => workingList` to the `env` object literal in `setup`. Then add to `describe("comments in the shell", ...)`:

```ts
  const rec = (over: Partial<Working> = {}): Working => ({ key: "k", agent: "a_x", harness: "codex", message: "Chart", thread_ids: [], started_at: "2026-09-30T10:00:00.000Z", last_heartbeat: "2026-09-30T10:00:00.000Z", ...over });

  it("working() names only this document's own threads, by handle", async () => {
    const { h, posted } = setup({ comments: {} });
    daemon(T("01J9C"));
    const created = await h.call("create", [{ anchor: T("x").anchor, text: "hi", version: 1 }]) as { threadId: string };
    workingList = [rec({ thread_ids: ["01J9C", "01J9Z"] })];
    const s = await h.call("working", []);
    expect(s).toEqual({ working: true, agents: [{ harness: "codex", label: "Codex", message: "Chart", since: "2026-09-30T10:00:00.000Z", threads: [created.threadId], otherThreads: 1 }] });
    expect(JSON.stringify(s)).not.toContain("01J9C");
    expect(JSON.stringify(s)).not.toContain("\"k\"");
    expect(posted.filter(m => m.type === "clax:event" && m.topic === "working")).toEqual([]);
  });

  it("pushes working state to a watching page on every working event, until it stops watching", async () => {
    const { h, posted } = setup({ comments: {} });
    const pushes = () => posted.filter(m => m.type === "clax:event" && m.topic === "working") as unknown as { data: { working: boolean } }[];
    await h.call("watchWorking", []);
    expect(pushes().at(-1)!.data).toEqual({ working: false, agents: [] });
    workingList = [rec({ harness: "pi", message: null })];
    h.onEvent!({ type: "working", artifact_id: "7q3k9mzx2b4t", working: workingList });
    expect(pushes().at(-1)!.data.working).toBe(true);
    await h.call("unwatchWorking", []);
    const n = pushes().length;
    h.onEvent!({ type: "working", artifact_id: "7q3k9mzx2b4t", working: [] });
    expect(pushes().length).toBe(n);
  });

  it("answers working() under the composer-only form, without consent or gesture", async () => {
    const { h, prompt } = setup({ comments: { composer_only: true } });
    gesture(false);
    expect(await h.call("working", [])).toEqual({ working: false, agents: [] });
    expect(prompt).not.toHaveBeenCalled();
  });
```

Run: `cd web && npx vitest run shell/src/caps/comments.test.ts`
Expected: FAIL (`comments.working is not part of this runtime`).

- [ ] **Step 2: Implement the shell side**

In `caps/host.ts`, add to `CapEnv`: `/** Who is working on the artifact now (the view's latest working list). */ working?(): Working[];`. In `view/artifact-controller.ts` `viewChanged()`, add `working: () => this.s.working,` to the env object passed to `new CapabilityHost(...)`. It reads the controller's current state at each call. `onEvent` already forwards every event to `this.host`, so `working` events reach the handler.

In `caps/comments.ts`:

```ts
import { harnessLabel, newestFirst, type Working } from "../view/working-model";

/** The Clax `working()` state for the page: agents newest first; threads as
 * the handles this document holds, the rest counted. No record key, no
 * session, no store ID. */
export function pageWorking(list: Working[], handleOf: (id: string) => string | undefined) {
  const agents = newestFirst(list).map(w => {
    const threads = w.thread_ids.map(handleOf).filter((h): h is string => h !== undefined);
    return { harness: w.harness, label: harnessLabel(w.harness), message: w.message, since: w.started_at, threads, otherThreads: w.thread_ids.length - threads.length };
  });
  return { working: agents.length > 0, agents };
}
```

In `commentsHandler`, add `let watchingWorking = false;`, a `const ownHandle = (id: string) => [...created].find(([, v]) => v === id)?.[0];`, and a `const pushWorking = () => { if (watchingWorking && !disposed) env.post(event("working", pageWorking(env.working?.() ?? [], ownHandle))); };`. Add cases before `default`. These run before any consent or gesture check in the handler, and are allowed under `composer_only`:

```ts
        case "working":
          return pageWorking(env.working?.() ?? [], ownHandle);
        case "watchWorking":
          watchingWorking = true;
          pushWorking();
          return null;
        case "unwatchWorking":
          watchingWorking = false;
          return null;
```

In `onEvent`, add `if (e.type === "working") pushWorking();`. In `reset` and `dispose`, set `watchingWorking = false`.

If the handler's method gate (the place that rejects write verbs under `composer_only` with `not_granted`) runs before the `switch`, add `working`, `watchWorking` and `unwatchWorking` to its allowed list for the composer-only form.

- [ ] **Step 3: The bridge side**

In `web/bridge/src/caps/comments.ts` (after the port's Task 13 it is part of the lazy `caps` part, and `commentsLocals(rpc, config, env)` takes a `CapsEnv`), inside `commentsLocals`, add:

```ts
  const workingFns = new Set<(s: unknown) => void>();
  let offWorking: (() => void) | null = null;
  const working = () => rpc.call("comments", "working", []);
  const onWorking = async (fn: unknown) => {
    if (typeof fn !== "function") throw invalid("onWorking takes a function");
    const f = fn as (s: unknown) => void;
    workingFns.add(f);
    if (!offWorking) {
      offWorking = rpc.on("comments", "working", d => { for (const g of workingFns) { try { g(d); } catch { /* the page's own error */ } } });
      await rpc.call("comments", "watchWorking", []);
    } else {
      try { f(await working()); } catch { /* the page's own error */ }
    }
    return () => {
      workingFns.delete(f);
      if (workingFns.size === 0 && offWorking) {
        offWorking();
        offWorking = null;
        void rpc.call("comments", "unwatchWorking", []).catch(() => {});
      }
    };
  };
```

and add `working` and `onWorking` to the namespace object this function returns, under both declaration forms. Add to `web/bridge/test/comments.test.ts`, with its `fakeRpc`:

```ts
  it("onWorking shares one shell subscription and ends it with the last subscriber", async () => {
    const f = fakeRpc(() => ({ working: false, agents: [] }));
    const c = commentsLocals(f.rpc as never, {}, { ctx: commentsContext, clip: () => import("../src/parts/clip") }) as unknown as { onWorking(fn: (s: unknown) => void): Promise<() => void> };
    const a: unknown[] = [];
    const b: unknown[] = [];
    const offA = await c.onWorking(s => a.push(s));
    const offB = await c.onWorking(s => b.push(s));
    const methods = () => f.rpc.call.mock.calls.map(x => x[1]);
    expect(methods().filter(m => m === "watchWorking")).toHaveLength(1);
    f.emit("working", { working: true, agents: [] });
    expect(a.at(-1)).toEqual({ working: true, agents: [] });
    expect(b.at(-1)).toEqual({ working: true, agents: [] });
    offA();
    expect(methods()).not.toContain("unwatchWorking");
    offB();
    expect(methods()).toContain("unwatchWorking");
  });
```

- [ ] **Step 4: The browser test**

Add to the `for (const mode …)` loop in `web/e2e/working.spec.ts` (it needs `publishWith`, `reach` and `readFileSync`):

```ts
  test(`${mode}: the page reads working state through the comments capability`, async ({ page }) => {
    const html = readFileSync(new URL("./pages/working-cap.html", import.meta.url), "utf8");
    const s = await registerSession(d.base, d.token, "codex", `cap-${mode}`);
    const { artifact } = await publishWith(d.base, d.token, `Cap ${mode}`, html, { comments: {} });
    const frame = await openArtifact(page, d.base, artifact.id, 1, mode);
    await expect(frame.locator("#state")).toHaveText("none");
    await page.getByLabel("Your name").fill("Alex");
    await page.getByLabel("Your name").press("Enter");
    await reach(page, frame.locator("#add"));
    await frame.locator("#add").click();
    await page.getByRole("dialog").getByRole("button", { name: "Allow", exact: true }).click();
    await expect.poll(() => frame.locator("body").getAttribute("data-handle")).toBeTruthy();
    const tid = (await api(d.base, d.token, `/api/artifacts/${artifact.id}/threads`)).threads[0].id as string;
    await setWorking(d.base, d.token, s.id, artifact.id, { message: "Chart", thread_ids: [tid] });
    await expect(frame.locator("#state")).toHaveText("Codex|Chart|1|0");
    expect(await frame.locator("body").getAttribute("data-handle")).not.toBe(tid);
    await api(d.base, d.token, `/api/sessions/${s.id}/working/${artifact.id}`, { method: "DELETE" });
    await expect(frame.locator("#state")).toHaveText("none");
  });
```

Until Task 24 moves the name field into the people panel, `getByLabel("Your name")` finds it in the top bar. Task 24 updates this locator.

Run: `cd web && npx playwright test e2e/working.spec.ts`
Expected: PASS.


Run: `cd web && npm run perf; echo "exit=$?"`
Expected: `exit=0`. Stop rule: if any of the five measures in `web/perf/budget.json` is over budget (link to first paint `firstPaint`, link to comment ready `commentReady`, frame paint `framePaint`, ready latency `readyLatency`, cold ready latency `coldLatency`, in either frame mode), stop and report all five numbers against their budgets. Lazy-load first; never raise a budget.

- [ ] **Step 5: Run, gates and staging**

Run: `cd web && npm run lint && npm run typecheck && npx vitest run`
Expected: PASS.

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add web/contract/clax-extensions.d.ts web/bridge/src/capabilities.ts web/bridge/test/capabilities.test.ts web/shell/src/caps/host.ts \
  web/shell/src/caps/comments.ts web/shell/src/caps/comments.test.ts web/bridge/src/caps/comments.ts web/shell/src/view/artifact-controller.ts
git add web/bridge/test/comments.test.ts web/e2e/working.spec.ts
git status --short   # staged; the controller commits ("Let pages read who is working through the comments capability (Clax extension)")
```

---

### Task 18: The changelog in Echo: the Addressed group, the version menu and its dot, looked-at marks, history, and a jump with highlight

The version changelog with no band over the page:
- an "Addressed in vN" group at the top of the sidebar;
- `claude · addressed in vN` on the agent's reply, and `vN claude addressed it` in the history line;
- addressed pins (white, a green ring, a `vN` flag);
- a version button with a green dot while a version newer than this viewer's last view exists;
- `v5 addressed 3` (or `3 new versions · 7 addressed`) in the top bar summary;
- a version menu that reads as a changelog (V opens it).

This task also writes the viewer's marks: the version seen, and each thread looked at.

**Files:**
- Create: `web/shell/src/view/changelog-model.ts`, `web/shell/src/view/version-rows.ts`, `web/shell/src/view/changelog-model.test.ts`, `web/shell/src/ui/AddressedGroup.svelte`, `web/shell/src/ui/VersionMenu.svelte`, `web/shell/src/ui/VersionPanel.svelte`, `web/e2e/changelog.spec.ts`
- Modify: `web/shell/src/api.ts` (`Version.note`, `Version.addresses`, `putSeen`), `web/shell/src/threads.ts` (`Thread.addressed_in`), `web/shell/src/view/history-model.ts`, `web/shell/src/view/history-model.test.ts`, `web/shell/src/view/working-model.ts` and `working-model.test.ts` (`summary` gains `published`), `web/shell/src/view/keys.ts`, `web/shell/src/view/keys.test.ts`, `web/shell/src/view/artifact-controller.ts`, `web/shell/src/view/artifact-controller.test.ts`, `web/shell/src/ui/TopbarIsland.svelte`, `web/shell/src/ui/SidebarIsland.svelte`, `web/shell/src/ui/Sidebar.svelte`, `web/shell/src/ui/ThreadCard.svelte`, `web/shell/src/ui/Pins.svelte`, `web/shell/src/ui/StageIsland.svelte`, `web/shell/src/sidebar.test.ts`, `web/shell/src/theme.css`, `web/bridge/src/comment-mode.ts`, `web/bridge/src/bridge.ts`, `web/e2e/fixtures.ts`, `web/e2e/scenes.ts`, `web/e2e/viewer.spec.ts`

**Interfaces:**
- `api.ts`: `Version` gains `note?: string | null` and `addresses?: string[]`. `putSeen(aid: string, n: number): Promise<void>` sends `PUT /api/viewers/me/seen`, and ignores failures.
- `threads.ts`: `Thread` gains `addressed_in?: number[]`.
- `view/changelog-model.ts` (no `svelte` import):
  - `type Decided = { n: number; ids: string[]; dot: boolean; line: string | null }`;
  - `decide(versions: Version[], latest: number, attention: Attention | null, pinned: boolean): Decided`. Its `line` is the returning-viewer summary (decided: Q11);
  - nothing else: the button needs only `decide`.
- `view/version-rows.ts` (no `svelte` import), imported only by the lazy `VersionPanel.svelte`, so it stays out of the artifact entry:
  - `type Row = { n: number; current: boolean; latest: boolean; who: string; when: string; chips: Chip[]; did: string | null; note: string | null; label: string | null }`, `type Chip = { id: string; n: number | null; open: boolean }` and `type RowInput`;
  - `versionRows(i: RowInput): Row[]`;
  - `excerpt(t: Thread): string`.
- `history-model.ts`: `historyOf` also emits `{ v: n, who: <agent of vN>, agent: true, verb: "addressed it" }` for each `n` in `t.addressed_in`. Events are ordered by time: comments by `created_at`, an address by its version's `created_at`, the resolve by `resolved_at`, and working last. `addressedNote(t: Thread, c: Comment): number | null` gives the version an agent reply is labelled with: the first version in `addressed_in` created at or after that reply, else null.
- `keys.ts`: `v` maps to `versions`, with a row `{ keys: ["V"], what: "Versions, with what each one addressed", action: "versions" }` after `R`.
- `ArtifactController`:
  - `ViewState` gains `decided: Decided | null`, `menu: "versions" | "people" | null`, and `looked: Record<string, string>` (this viewer's marks, seeded from `attention.looked`).
  - New methods: `look(t: Thread): void`, which queues a mark and flushes at most once a second through `putLooked`; `openMenu(m)`; `closeMenu()`.
  - `shortcut("versions")` opens the menu.
  - A private `decideChangelog()` runs once the view is ready, and again when a new latest version is loaded. A separate private `writeSeen()` writes `seen` (unpinned latest only) once `me` is known, so a first visit, whose viewer arrives after the decision, still writes it.
- Components:
  - `AddressedGroup` `{ n: number; agent: string; count: number; children: Snippet }`, lazy.
  - `VersionMenu` `{ shown: number; latest: number; dot: boolean; open: boolean; onToggle(): void; input: () => RowInput; hrefFor(n: number): string; onChoose(n: number): void }`. It is eager and imports only types. Its panel, `VersionPanel` `{ input: RowInput; … }`, is lazy and computes the rows itself with `versionRows`.
  - `ThreadCard` gains `onSeen?(t: Thread): void`, which fires when the card has been at least half visible for 1 s (decided: Q4).
  - `Pins` gains `addressed?: Map<string, number>` (thread ID → version).

- [ ] **Step 1: The model, test first**

`web/shell/src/view/changelog-model.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import type { Attention, Version } from "../api";
import type { Thread } from "../threads";
import { decide } from "./changelog-model";
import { excerpt, versionRows } from "./version-rows";

const V = (n: number, addresses: string[] = [], note: string | null = null): Version =>
  ({ artifact_id: "a", n, label: null, created_at: `2026-09-30T1${n}:00:00.000Z`, files: {}, note, addresses, agent: "a_1", agent_harness: "claude" });
const A = (over: Partial<Attention>): Attention => ({ addressed: [], addressed_v: null, new_replies: [], open_in: [], seen: null, looked: {}, ...over });
const T = (id: string, status: "open" | "resolved" = "open", extra: Partial<Thread> = {}): Thread => ({
  id, artifact_id: "a", version_n: 1, status, sent_to_agent: true, has_clip: false, clip_url: null, created_at: "2026-09-30T10:30:00.000Z", resolved_at: null,
  resolved_by: null, feedback_state: null, comments: [{ id: `${id}c`, thread_id: id, author_kind: "viewer", author_name: "alex", author_public_id: "u_me", via_harness: null, body: "Make this  two\ncolumns", created_at: "2026-09-30T10:30:00.000Z" }],
  anchor: { kind: "element", selector: "h2", quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" }, ...extra,
});

describe("decide", () => {
  const vs = [V(1), V(2, ["t1"]), V(3, ["t1", "t2", "t3"], "Two columns")];
  it("holds the newest version's addressed threads you are in and have not looked at", () => {
    expect(decide(vs, 3, A({ addressed: ["t1", "t3", "t9"], seen: 2 }), false)).toEqual({ n: 3, ids: ["t1", "t3"], dot: true, line: "v3 addressed 2" });
  });
  it("summarises a return after several versions", () => {
    expect(decide(vs, 3, A({ addressed: ["t1", "t2"], seen: 1 }), false).line).toBe("2 new versions · 2 addressed");
  });
  it("shows no dot on a first visit or once seen, and nothing for a pinned or anonymous view", () => {
    expect(decide(vs, 3, A({ seen: null }), false).dot).toBe(false);
    expect(decide(vs, 3, A({ seen: 3 }), false)).toEqual({ n: 3, ids: [], dot: false, line: null });
    expect(decide(vs, 3, A({ addressed: ["t1"], seen: 2 }), true)).toEqual({ n: 3, ids: [], dot: false, line: null });
    expect(decide(vs, 3, null, false)).toEqual({ n: 3, ids: [], dot: false, line: null });
  });
});

describe("versionRows and excerpt", () => {
  it("lists versions newest first with who, the threads addressed, what you did, and the note", () => {
    const resolved = T("t1", "resolved", { resolved_by: "viewer:u_me", resolved_at: "2026-09-30T12:30:00.000Z" });
    const replied = T("t2", "open", { comments: [...T("t2").comments, { id: "r", thread_id: "t2", author_kind: "viewer", author_name: "alex", author_public_id: "u_me", via_harness: null, body: "not yet", created_at: "2026-09-30T12:40:00.000Z" }] });
    const rows = versionRows({ versions: [V(1), V(2, ["t1", "t2"], "Units: ms")], latest: 2, shown: 2, now: new Date("2026-09-30T12:45:00.000Z"),
      threads: [resolved, replied], numbers: new Map([["t2", 2]]), me: "u_me" });
    expect(rows[0]).toMatchObject({ n: 2, current: true, latest: true, who: "claude", chips: [{ id: "t1", n: null, open: false }, { id: "t2", n: 2, open: true }],
      did: "you resolved it; you replied on #2, still open", note: "Units: ms" });
    expect(rows[1]).toMatchObject({ n: 1, chips: [], did: null, note: "First publish" });
    expect(excerpt(T("t"))).toBe("Make this two columns");
  });
});
```

Run: `cd web && npx vitest run shell/src/view/changelog-model.test.ts`
Expected: FAIL.

The code below is one listing for reading; split it into two files. `changelog-model.ts` keeps its header comment, `Decided` and `decide`, and imports only `type Attention, Version` from `../api`. `version-rows.ts` takes `Chip`, `Row`, `RowInput`, `versionRows` and `excerpt`, with the imports they use (`relativeTime`, `Thread`, `Version`, `agentName`) and the header comment `// The version menu's rows (spec §8): who published each version and when, the threads it addressed, what this viewer did about them, and its note. Loaded with the menu's panel.`

`web/shell/src/view/changelog-model.ts`:

```ts
// The version changelog as Echo shows it (spec §8, §10): no band over the
// page. A load decides the Addressed group (frozen until the next decision),
// the version button's dot and the summary's line, from this viewer's
// attention; the version menu reads as a changelog.
import type { Attention, Version } from "../api";
import { relativeTime } from "../format";
import type { Thread } from "../threads";
import { agentName } from "./history-model";

export type Decided = { n: number; ids: string[]; dot: boolean; line: string | null };
export type Chip = { id: string; n: number | null; open: boolean };
export type Row = { n: number; current: boolean; latest: boolean; who: string; when: string; chips: Chip[]; did: string | null; note: string | null; label: string | null };
export type RowInput = { versions: Version[]; latest: number; shown: number; now: Date; threads: Thread[]; numbers: Map<string, number>; me: string | null };

export function decide(versions: Version[], latest: number, att: Attention | null, pinned: boolean): Decided {
  const none = { n: latest, ids: [], dot: false, line: null };
  if (pinned || !att) return none;
  const v = versions.find(x => x.n === latest);
  const addressed = new Set(att.addressed);
  const ids = (v?.addresses ?? []).filter(id => addressed.has(id));
  const seen = att.seen;
  const dot = seen !== null && latest > seen;
  let line: string | null = null;
  if (seen !== null && latest - seen > 1) {
    const k = new Set(versions.filter(x => x.n > seen).flatMap(x => x.addresses ?? []).filter(id => addressed.has(id))).size;
    if (k) line = `${latest - seen} new versions · ${k} addressed`;
  } else if (ids.length) line = `v${latest} addressed ${ids.length}`;
  return { n: latest, ids, dot, line };
}

export function versionRows(i: RowInput): Row[] {
  const byId = new Map(i.threads.map(t => [t.id, t]));
  return [...i.versions].sort((a, b) => b.n - a.n).map(v => {
    const chips: Chip[] = [];
    const did: string[] = [];
    for (const id of v.addresses ?? []) {
      const t = byId.get(id);
      if (!t) continue;
      const mineAfter = t.comments.some(c => c.author_public_id === i.me && c.created_at > v.created_at);
      chips.push({ id, n: i.numbers.get(id) ?? null, open: t.status === "open" && mineAfter });
      const num = i.numbers.get(id);
      if (t.status === "resolved" && t.resolved_by === `viewer:${i.me}`) did.push(`you resolved ${num ? `#${num}` : "it"}`);
      else if (t.status === "open" && mineAfter) did.push(`you replied on ${num ? `#${num}` : "it"}, still open`);
    }
    return {
      n: v.n, current: v.n === i.shown, latest: v.n === i.latest, who: v.agent_harness ? agentName(v.agent_harness) : "command line",
      when: relativeTime(v.created_at, i.now), chips, did: did.length ? did.join("; ") : null,
      note: v.note ?? (v.n === 1 ? "First publish" : null), label: v.label,
    };
  });
}

export function excerpt(t: Thread): string {
  const s = (t.comments[0]?.body ?? "").split(/\s+/).filter(Boolean).join(" ");
  return s.length > 80 ? `${s.slice(0, 80)}…` : s;
}
```

In `history-model.ts`, give `historyOf` its addressed events and time order:

```ts
export function addressedNote(t: Thread, c: Comment, versions: Version[]): number | null {
  if (c.author_kind !== "agent") return null;
  const at = (n: number) => versions.find(v => v.n === n)?.created_at ?? "";
  return (t.addressed_in ?? []).find(n => at(n) >= c.created_at) ?? null;
}
```

Build the list as `{ at, e }` pairs. A comment's `at` is its `created_at`. An address's `at` is its version's `created_at`, with the event `{ v: n, who: agentName(<version n>.agent_harness), agent: true, verb: "addressed it" }`. The resolve's `at` is `resolved_at`, and working's `at` is `"~"`, which sorts last. Sort by `at` and map to `e`. Add a test to `history-model.test.ts`: an agent reply at 11:20, then v3 at 12:00 addressing the thread, gives `[…, claude replied, v3 claude addressed it]`, and `addressedNote` for that reply is `3`.

Run: `cd web && npx vitest run shell/src/view/changelog-model.test.ts shell/src/view/history-model.test.ts`
Expected: PASS.

- [ ] **Step 2: The controller, test first**

Add to `view/artifact-controller.test.ts`, using Task 3's seeded `started()`. The `loaded` fixture is at v2. Declare `const seed: Seed = { … }` above the test, giving `threads: [thread("t1")]`, `versions` with v1 and v2 (v2 `addresses: ["t1"]`, `note: "Two columns"`), `attention: { addressed: ["t1"], addressed_v: 2, new_replies: [], open_in: ["t1"], seen: 1, looked: {} }`, and `routes` answering `/api/viewers/me/seen` with `{ seen: 2 }` and `/api/viewers/me/looked` with `{ looked: { t1: "x" } }`. The `fetch` mock records every call, so the test reads the `PUT` bodies from it.

```ts
  it("decides the changelog once ready, writes seen for the unpinned latest, and batches looked-at marks", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const { ctl } = await started(seed);
    await vi.waitFor(() => expect(ctl.state.get().decided).toEqual({ n: 2, ids: ["t1"], dot: true, line: "v2 addressed 1" }));
    const puts = () => (fetch as unknown as ReturnType<typeof vi.fn>).mock.calls.filter(([, i]) => (i as RequestInit | undefined)?.method === "PUT");
    expect(puts().some(([u, i]) => String(u) === "/api/viewers/me/seen" && JSON.parse((i as RequestInit).body as string).version === 2)).toBe(true);
    const t1 = ctl.state.get().threads.find(t => t.id === "t1")!;
    ctl.look(t1);
    ctl.look(t1);
    await vi.advanceTimersByTimeAsync(1100);
    const looked = puts().filter(([u]) => String(u) === "/api/viewers/me/looked");
    expect(looked).toHaveLength(1);
    expect(JSON.parse((looked[0][1] as RequestInit).body as string)).toEqual({ artifact_id: ID, thread_ids: ["t1"] });
    expect(ctl.state.get().decided!.ids).toEqual(["t1"]);
    ctl.dispose();
    vi.useRealTimers();
  });
```

The last assertion holds because the group stays decided until the next decision (decided: Q4).

Implement in `view/artifact-controller.ts`:
- `ViewState` fields: `decided: null`, `menu: null` and `looked: {}`. `looked` is seeded from `attention?.looked ?? {}` whenever attention is set.
- `private decideFor = 0;`, `private seenFor = 0;` and:

```ts
  /** The changelog for this load (spec §8): decided once the view is ready,
   * and again when a newer latest version loads; never on the path to first
   * paint. The decision stays frozen until then (decided: Q4). */
  private decideChangelog(): void {
    const s = this.s;
    if (!viewReady(s)) return;
    const latest = s.data.artifact.current_version;
    if (this.decideFor === latest) return;
    this.decideFor = latest;
    const pinned = this.pinnedVersion !== null || this.shown(s) !== latest;
    this.set({ decided: decide(s.data.versions, latest, s.attention, pinned) });
  }

  /** Writes the version seen once this viewer is known and views the
   * unpinned latest. Separate from the decision: on a first visit there is
   * no cookie yet, so `me` arrives after the decision, from `getViewer()`. */
  private writeSeen(): void {
    const s = this.s;
    if (!viewReady(s) || !s.me || s.deleted) return;
    const latest = s.data.artifact.current_version;
    if (this.pinnedVersion !== null || this.shown(s) !== latest || this.seenFor === latest) return;
    this.seenFor = latest;
    void putSeen(this.id, latest);
  }

  private pendingLook = new Set<string>();
  private lookTimer: ReturnType<typeof setTimeout> | undefined;
  /** This viewer looked at `t` (spec §10, "Participants and attention"); marks go out at most once a second. */
  look(t: Thread): void {
    if (this.s.looked[t.id] && this.s.looked[t.id] >= (t.comments.at(-1)?.created_at ?? "")) return;
    this.pendingLook.add(t.id);
    this.lookTimer ??= setTimeout(() => {
      this.lookTimer = undefined;
      const ids = [...this.pendingLook];
      this.pendingLook.clear();
      void putLooked(this.id, ids).then(m => { if (m && !this.disposed) this.set(s => ({ looked: { ...s.looked, ...m } })); });
    }, 1000);
  }

  openMenu(m: "versions" | "people"): void { this.set(s => ({ menu: s.menu === m ? null : m })); }
  closeMenu(): void { this.set({ menu: null }); }
```

- Call `decideChangelog()` at the end of `loaded(d)`, after the bootstrap seed, and in the `react()` pass when `prev.data !== s.data`. Call `writeSeen()` in the `react()` pass whenever `prev.me !== s.me` or `prev.data !== s.data`, and at the end of `loaded(d)`.
- `selectThread(t)` also calls `this.look(t)`.
- `dispose` clears `lookTimer`.
- The Escape branch closes `menu` before `sheet` and comment mode.
- `shortcut("versions")` calls `this.openMenu("versions")`.
- Import `decide` and `type Decided` from `./changelog-model`, and `putSeen` and `putLooked` from `../api`.

- [ ] **Step 3: Components**

`web/shell/src/ui/AddressedGroup.svelte`:

```svelte
<script lang="ts">
  import type { Snippet } from "svelte";
  let { n, agent, count, children }: { n: number; agent: string; count: number; children: Snippet } = $props();
</script>

<section class="section-addressed" aria-label={`Addressed in v${n}`}>
  <h2 class="gh ag"><span class="sw" aria-hidden="true"></span><span class="t">Addressed in v{n}</span> <span class="c">{count}</span></h2>
  <p class="gsub">{agent} addressed these. Have a look, then resolve each one or reply.</p>
  {@render children()}
</section>
```

`web/shell/src/ui/VersionMenu.svelte`:

```svelte
<script lang="ts">
  // The version button (spec §8): `v5 of 5`, a green dot while a version
  // newer than this viewer's last view exists. Its panel loads on first open.
  import type { RowInput } from "../view/version-rows";

  let { shown, latest, dot, open, onToggle, input, hrefFor, onChoose }: {
    shown: number; latest: number; dot: boolean; open: boolean; onToggle(): void; input: () => RowInput; hrefFor(n: number): string; onChoose(n: number): void;
  } = $props();
  let button: HTMLButtonElement | undefined = $state();
</script>

<div class="version-menu hide-sm">
  <button type="button" class="vbtn" bind:this={button} aria-haspopup="dialog" aria-expanded={open} aria-label={`Version ${shown} of ${latest}${dot ? ", a newer version you have not seen" : ""}`} onclick={onToggle}>
    v{shown}<span>of {latest} ▾</span>{#if dot}<i class="new" aria-hidden="true"></i>{/if}
  </button>
  {#if open}
    {#await import("./VersionPanel.svelte") then { default: VersionPanel }}
      <VersionPanel input={input()} {hrefFor} {onChoose} onClose={() => { onToggle(); button?.focus(); }} />
    {/await}
  {/if}
</div>
```

`web/shell/src/ui/VersionPanel.svelte`:

```svelte
<script lang="ts">
  import { type RowInput, versionRows } from "../view/version-rows";

  let { input, hrefFor, onChoose, onClose }: { input: RowInput; hrefFor(n: number): string; onChoose(n: number): void; onClose(): void } = $props();
  const rows = $derived(versionRows(input));
  let panel: HTMLDivElement | undefined = $state();
  $effect(() => { panel?.querySelector<HTMLElement>("a[aria-current=page]")?.focus(); });
  function choose(e: MouseEvent, n: number) {
    // A plain click moves through the controller, as the select did; a
    // modified click keeps the link's own behaviour (a new tab).
    if (e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
    e.preventDefault();
    onChoose(n);
  }
  function outside(e: PointerEvent) { if (panel && !panel.contains(e.target as Node) && !(e.target as Element).closest?.(".vbtn")) onClose(); }
</script>

<svelte:window onpointerdown={outside} />
<!-- Escape closes the dialog. -->
<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div class="vmenu" role="dialog" aria-label="Versions" tabindex="-1" bind:this={panel}
  onkeydown={e => { if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); onClose(); } }}>
  <ol>
    {#each rows as r (r.n)}
      <li class={["vrow", r.current && "cur"]}>
        <a href={hrefFor(r.n)} aria-current={r.current ? "page" : undefined} onclick={e => choose(e, r.n)}>
          <span class="g">v{r.n}</span>
          <span class="h">{r.who}<small>{r.when}{r.label ? ` · ${r.label}` : ""}</small></span>
          {#if r.chips.length}<span class="cl">Addressed {#each r.chips as c (c.id)}<span class={["pc", c.open && "open"]}><i>{c.n ?? "•"}</i></span>{/each}</span>{/if}
          {#if r.did}<span class="cl">{r.did}</span>{/if}
          {#if r.note}<span class="cl note">{r.note}</span>{/if}
        </a>
      </li>
    {/each}
  </ol>
</div>
```

`ui/ThreadCard.svelte`:
- Add `onSeen?(t: Thread): void`, and attach to the `<article>`:

```ts
  const seen = (el: HTMLElement) => {
    if (!onSeen || typeof IntersectionObserver !== "function") return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const io = new IntersectionObserver(([e]) => {
      clearTimeout(timer);
      if (e.intersectionRatio >= 0.5) timer = setTimeout(() => onSeen(t), 1000);
    }, { threshold: [0, 0.5] });
    io.observe(el);
    return () => { clearTimeout(timer); io.disconnect(); };
  };
```

- In the author line of an agent comment, append `{#if note}<span class="muted"> · addressed in v{note}</span>{/if}`, with `{@const note = addressedNote(t, c, versions)}`. The card takes `versions` from `Sidebar`.

`ui/Sidebar.svelte`:
- Add `decided?: Decided | null` and `onSeen?(t: Thread): void`.
- The Open section lists `s.open` minus `decided.ids`.
- Before it, render the group with the `{#await}`:

```svelte
  {@const group = p.decided ? p.threads.filter(t => p.decided!.ids.includes(t.id)) : []}
  {#if group.length}
    {#await import("./AddressedGroup.svelte") then { default: AddressedGroup }}
      <AddressedGroup n={p.decided!.n} agent={p.agent} count={group.length}>{@render cards(group)}</AddressedGroup>
    {/await}
  {/if}
```

- `cards` passes `onSeen={p.onSeen}` and `versions={p.versions}`.

`ui/SidebarIsland.svelte` passes `decided={s.decided}` and `onSeen={t => ctl.look(t)}`.

`ui/Pins.svelte`: `addressed?: Map<string, number>` adds `class:addressed` and `data-v={`v${n}`}` to a pin whose thread is in the map. `StageIsland` passes the map of `s.decided.ids` (all at `s.decided.n`) minus threads in `s.looked` newer than the decision.

`ui/TopbarIsland.svelte`:
- Replace the `<select class="version …">` with:

```svelte
  <VersionMenu {shown} {latest} dot={s.decided?.dot ?? false} open={s.menu === "versions"} onToggle={() => ctl.openMenu("versions")}
    input={() => ({ versions: s.data.versions, latest, shown, now: new Date(), threads: s.threads, numbers: ctl.numbers(s), me: s.me?.public_id ?? null })}
    hrefFor={n => ctl.here(n === latest ? null : n, s)} onChoose={n => { ctl.closeMenu(); ctl.chooseVersion(n); }} />
```

  `ctl.numbers(s)` is the pin numbering the sidebar uses (`sidebarSections(...).numbers`). Expose it as a public method.
- Pass `addressed: s.decided?.line ?? null` into `summary(...)`.

`view/keys.ts`: add `v: "versions"` to `MAP`, and the `V` row after `R`. Update `keys.test.ts`'s row list.

In `web/e2e/viewer.spec.ts`, replace `await page.selectOption("select", "1");` with:

```ts
  await page.getByRole("button", { name: /^Version 2 of 2/ }).click();
  await page.getByRole("dialog", { name: "Versions" }).getByRole("link", { name: /^v1\b/ }).click();
```

- [ ] **Step 4: The version banners move off the page (decided: Q12)**

The port shows two bands over the stage: `vN published` with Reload, and `viewing vN; latest is vM` with a link. Echo puts nothing over the page, so both move into the top bar.
- In `ui/StageIsland.svelte`, delete the two `<div class="banner">` blocks for `s.newer` and for `shown < latest`. The `s.notice` alert stays: it reports a failure the person dismisses.
- `summary(...)` takes `published: number | null`. With nobody working, `line1` is `v{n} published` and `line2` is `reload to see it`. In `TopbarIsland.svelte`, pass `published: s.deleted ? null : s.newer`. Right after the `.who` block, add:

```svelte
  {#if s.newer && !s.deleted}<button class="primary reload" onclick={() => ctl.reloadLatest()}>Reload</button>{/if}
  {#if shown < latest && !s.newer && !s.deleted}<a class="latest hide-sm" href={ctl.here(null, s)}>Latest</a>{/if}
```

- In `working-model.test.ts`, add: `published: 3` with nobody working reads `v3 published` over `reload to see it`. Every other case passes `published: null`.
- The e2e step that clicked `Reload` in the stage still finds the button by role and name. Specs that asserted the stage `.banner` text assert the summary line instead: `grep -rln '"\.banner"\|v[0-9] published' web/e2e`.

Add to the CSS in Step 6: `.topbar a.latest { font: 600 14px var(--grot); color: var(--accent-ink); }`.

- [ ] **Step 5: Reduced-motion highlight in the bridge**

In `web/bridge/src/comment-mode.ts` (the lazy comment part), in `flash`, use a 1200 ms timeout instead of 1800 when `matchMedia("(prefers-reduced-motion: reduce)").matches`. Its `CSS` already holds the reduced-motion rule with a static tint, which is the static outline the spec promises; add nothing to it.

In `web/bridge/src/bridge.ts`, in the `clax:scroll-to` case, compute `const behavior: ScrollBehavior = matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth";` and pass it to both `scrollBy` and `scrollIntoView`.

- [ ] **Step 6: Styles**

Append to `web/shell/src/theme.css`:

```css
/* The changelog (spec §8): no band over the page. */
.gsub { margin: -4px 2px 10px 17px; font-size: 12px; color: var(--muted); line-height: 1.5; }
.version-menu { position: relative; }
.vbtn { display: flex; align-items: baseline; gap: 4px; padding: 0 10px; height: 36px; position: relative; font: 600 22px/36px var(--grot); }
.vbtn span { font: 400 12px var(--mono); color: var(--muted); }
.vbtn .new { position: absolute; top: 5px; right: 5px; width: 7px; height: 7px; border-radius: 50%; background: var(--agent); }
.vmenu { position: absolute; right: 0; top: calc(100% + 8px); z-index: 20; width: 440px; max-height: 70vh; overflow: auto; background: var(--raised); border: 1px solid var(--border-strong); box-shadow: 0 14px 40px var(--shadow); padding: 6px 0; }
.vmenu ol { list-style: none; margin: 0; padding: 0; }
.vrow a { display: grid; grid-template-columns: 48px 1fr; gap: 2px 10px; padding: 9px 16px; border-bottom: 1px solid var(--border); }
.vrow:last-child a { border-bottom: 0; }
.vrow a:hover, .vrow a:focus-visible { background: var(--bg); outline: none; }
.vrow .g { font-size: 24px; line-height: 1; grid-row: span 4; }
.vrow.cur a { background: var(--bg); } .vrow.cur .g { color: var(--agent-ink); }
.vrow .h { font: 600 14px var(--grot); } .vrow .h small { font: 400 11.5px var(--mono); color: var(--muted); margin-left: 6px; }
.vrow .cl { display: flex; flex-wrap: wrap; gap: 4px 10px; font-size: 12px; color: var(--muted); align-items: center; }
.pc i { font-style: normal; display: inline-block; width: 17px; height: 17px; border-radius: 50%; font: 600 10px/17px var(--mono); text-align: center; background: var(--card); color: var(--fg); box-shadow: inset 0 0 0 1.5px var(--agent); }
.pc.open i { box-shadow: inset 0 0 0 1.5px var(--you); }
.thread-pin.addressed { background: #fff; border-color: #457d26; color: #2f0b04; }
.thread-pin[data-v]::after { content: attr(data-v); position: absolute; left: 22px; top: 2px; font: 600 10.5px/14px var(--grot); background: #fff; color: #2f0b04; border: 1px solid #457d26; padding: 0 3px; white-space: nowrap; }
@media (max-width: 700px) { .vmenu { position: fixed; left: 0; right: 0; top: 56px; bottom: 52px; width: auto; max-height: none; box-shadow: none; border-width: 1px 0 0; } }
```

The pin colours are literal, because pins sit over the artifact, whose colours Clax does not set.

- [ ] **Step 7: Browser tests**

Add to `web/e2e/fixtures.ts`:
- `publishNext(base, token, sid, aid, ifVersion, extra: { note?: string; addresses?: string[] })`;
- `seenOf(page, aid)`, which reads `/api/viewers/me/seen?artifact=` from inside the page.

`web/e2e/changelog.spec.ts`:

```ts
import { test, expect, type Page } from "@playwright/test";
import { contentFrame, openArtifact, publishAs, publishNext, reach, registerSession, seenOf, startDaemon } from "./fixtures";

let d: Awaited<ReturnType<typeof startDaemon>>;
test.beforeAll(async () => { test.setTimeout(180_000); d = await startDaemon(); });
test.afterAll(async () => { await d?.stop(); });
const PAGE = "<main><h2>Quarterly goals</h2></main>";

/** Names this page's viewer and comments on the heading through the shell's own API call, so the thread is theirs. */
async function commentAs(page: Page, aid: string, name: string, body: string): Promise<string> {
  return page.evaluate(async ([aid, name, body]) => {
    await fetch("/api/viewers/me", { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify({ display_name: name }) });
    const f = new FormData();
    f.set("anchor", JSON.stringify({ kind: "element", selector: "body > main > h2", quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" }));
    f.set("body", body); f.set("version", "1");
    return (await (await fetch(`/api/artifacts/${aid}/threads`, { method: "POST", body: f })).json()).thread.id as string;
  }, [aid, name, body] as const);
}

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: a new version puts nothing over the page: a dot, a summary line, and the Addressed group`, async ({ page }) => {
    const s = await registerSession(d.base, d.token, "claude", `cl-${mode}`);
    const { artifact } = await publishAs(d.base, d.token, s.id, `Changelog ${mode}`, { "index.html": PAGE });
    await openArtifact(page, d.base, artifact.id, 1, mode);
    const tid = await commentAs(page, artifact.id, "alex", "Two columns");
    await expect.poll(() => seenOf(page, artifact.id)).toBe(1);
    await publishNext(d.base, d.token, s.id, artifact.id, 1, { note: "Two columns", addresses: [tid] });
    await page.getByRole("button", { name: "Reload" }).click();
    await contentFrame(page, artifact.id, 2);
    await expect(page.locator(".stage .banner:not(.notice)")).toHaveCount(0);
    await expect(page.locator(".vbtn .new")).toHaveCount(1);
    await expect(page.locator(".who .sum b.l1")).toHaveText("v2 addressed 1");
    if (!(await page.locator("aside.sidebar").isVisible())) await page.getByRole("button", { name: /Threads/ }).first().click();
    const group = page.locator(".section-addressed");
    await expect(group.locator("h2")).toContainText("Addressed in v2");
    await expect(group.locator(".hist")).toContainText("v2claude addressed it");
    await expect(page.locator(".thread-pin.addressed")).toHaveAttribute("data-v", "v2");
    await page.waitForTimeout(2500);
    await page.reload();
    await contentFrame(page, artifact.id, 2);
    await expect(page.locator(".section-addressed")).toHaveCount(0);
    await expect(page.locator(".vbtn .new")).toHaveCount(0);
  });

  test(`${mode}: the version menu reads as a changelog, opens with V, and closes with Escape`, async ({ page }) => {
    const s = await registerSession(d.base, d.token, "claude", `menu-${mode}`);
    const { artifact } = await publishAs(d.base, d.token, s.id, `Menu ${mode}`, { "index.html": PAGE });
    await openArtifact(page, d.base, artifact.id, 1, mode);
    const tid = await commentAs(page, artifact.id, "alex", "Two columns");
    await publishNext(d.base, d.token, s.id, artifact.id, 1, { addresses: [tid], note: "Two columns" });
    await openArtifact(page, d.base, artifact.id, 2, mode);
    await page.locator("body").press("v");
    const dialog = page.getByRole("dialog", { name: "Versions" });
    await expect(dialog.locator(".vrow").first()).toContainText("Two columns");
    await expect(dialog.locator(".vrow").first().locator(".pc")).toHaveCount(1);
    await expect(dialog.locator("a[aria-current=page]")).toBeFocused();
    await page.keyboard.press("Escape");
    await expect(dialog).toHaveCount(0);
    await page.getByRole("button", { name: /^Version 2 of 2/ }).click();
    await dialog.getByRole("link", { name: /^v1\b/ }).click();
    await expect(page).toHaveURL(new RegExp(`/a/${artifact.id}/v/1$`));
  });

  test(`${mode}: a pinned view writes no seen mark, and a group card jumps with a highlight and resolves`, async ({ page }) => {
    const s = await registerSession(d.base, d.token, "claude", `jump-${mode}`);
    const { artifact } = await publishAs(d.base, d.token, s.id, `Jump ${mode}`, { "index.html": PAGE });
    await openArtifact(page, d.base, artifact.id, 1, mode);
    const tid = await commentAs(page, artifact.id, "alex", "Two columns");
    await publishNext(d.base, d.token, s.id, artifact.id, 1, { addresses: [tid] });
    // Below 900px the sidebar starts closed, so no card is on screen long
    // enough to count as looked at while the pinned view waits.
    const size = page.viewportSize()!;
    await page.setViewportSize({ width: 800, height: size.height });
    await page.goto(`${d.base}/a/${artifact.id}/v/1`);
    await page.waitForTimeout(1500);
    expect(await seenOf(page, artifact.id)).toBe(1);
    await page.setViewportSize(size);
    const frame = await openArtifact(page, d.base, artifact.id, 2, mode);
    if (!(await page.locator("aside.sidebar").isVisible())) await page.getByRole("button", { name: /Threads/ }).first().click();
    const card = page.locator(`.section-addressed .thread-card[data-thread="${tid}"]`);
    await reach(page, card.locator(".card-head"));
    await card.locator(".card-head").click();
    await expect(frame.locator("clax-overlay .o.flash")).toHaveCount(1);
    await card.getByRole("button", { name: "Resolve" }).click();
    await expect(card.locator(".hist")).toContainText("alex resolved");
  });
}
```

Run: `cd web && npx vitest run && npm run lint && npm run typecheck && npm run build && node scripts/bundle-size.mjs && npx playwright test e2e/changelog.spec.ts e2e/viewer.spec.ts; echo "exit=$?"`
Expected: `exit=0`. `AddressedGroup`, `VersionPanel` and `version-rows.ts` are outside `artifact.html`'s closure. If the `artifact` budget fails, first move what first paint does not need behind a dynamic `import()` (the history line's addressed events, the menu's panel parts), then stop and report the sizes. Never raise the budget.

- [ ] **Step 8: Screenshots and a look**

Append scenes to `web/e2e/scenes.ts`. Each one publishes v2 addressing the seeded threads with the note `Two columns; units in ms`, after naming the viewer "alex" and posting a thread as them:
- `changelog`: panel open;
- `versions`: the menu open.

Run: `cd web && CLAX_SHOTS=task-18 CLAX_SCENES=changelog,versions,threads npx playwright test e2e/shots.spec.ts`
Expected: PASS.

Report, against `concept-3-echo/shots/*-changelog.png` and `*-versions.png`:
- nothing covers the page;
- the dot sits on the version button;
- the summary line;
- the green group head and its sub-line;
- `claude · addressed in v2` with the green rule on the right;
- the history line;
- the addressed pins with their flags;
- the menu rows (numeral, who, when, chips, what you did, the note);
- the phone sheet.

Run: `cd web && npm run perf; echo "exit=$?"`
Expected: `exit=0`. Stop rule: if any of the five measures in `web/perf/budget.json` is over budget (link to first paint `firstPaint`, link to comment ready `commentReady`, frame paint `framePaint`, ready latency `readyLatency`, cold ready latency `coldLatency`, in either frame mode), stop and report all five numbers against their budgets. Lazy-load first; never raise a budget.

- [ ] **Step 9: Gates and staging**

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add web/shell/src/view/changelog-model.ts web/shell/src/view/version-rows.ts web/shell/src/view/changelog-model.test.ts web/shell/src/ui/AddressedGroup.svelte web/shell/src/ui/VersionMenu.svelte \
  web/shell/src/ui/VersionPanel.svelte web/e2e/changelog.spec.ts web/bridge/src/comment-mode.ts web/bridge/src/bridge.ts web/e2e/fixtures.ts web/e2e/scenes.ts web/e2e/viewer.spec.ts
git add -u web/shell/src
git status --short   # staged; the controller commits ("Show each version's changelog without covering the page: the Addressed group, the version menu and its dot, and history")
```

---

### Task 19: Needs your eyes: the gallery's grouping and card markers

The gallery floats what needs this viewer's eyes and puts everything else below, pinned first and then the most recent. Each card carries its markers, its roster and `seen vK`. Attention comes from one request made beside the artifact list. A version or a thread event refreshes it, at most once a second.

**Files:**
- Create: `web/shell/src/view/attention-model.ts`, `web/shell/src/view/attention-model.test.ts`, `web/e2e/attention.spec.ts`
- Modify: `web/shell/src/ui/Gallery.svelte`, `web/shell/src/ui/GalleryCard.svelte`, `web/shell/src/ui/working-feed.svelte.ts`, `web/shell/src/events.ts`, `web/shell/src/events.test.ts`, `web/shell/src/gallery.test.ts`, `web/shell/src/theme.css`, `web/e2e/scenes.ts`

**Interfaces:**
- `view/attention-model.ts` (no `svelte` import):
  - `type Marker = { kind: "you" | "new" | "rep" | "ag" | "oth"; text: string }`;
  - `needsEyes(a: Artifact, att?: AttentionSummary): boolean`;
  - `markers(a: Artifact, att: AttentionSummary | undefined, working: string[]): Marker[]`;
  - `groups(list: Artifact[], att: Record<string, AttentionSummary>): { needs: Artifact[]; rest: Artifact[] }`;
  - `seenText(att?: AttentionSummary): string | null`.
- `events.ts`: `subscribeWorking` becomes `subscribeGallery(onEvent)`, which opens `/api/events?types=working,version,thread`. The working feed keeps its name and API, and adds `onChange(fn)` for version and thread events.

- [ ] **Step 1: The model, test first**

`web/shell/src/view/attention-model.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import type { Artifact, AttentionSummary } from "../api";
import { groups, markers, needsEyes, seenText } from "./attention-model";

const A = (id: string, over: Partial<Artifact> = {}): Artifact => ({ id, title: id, description: null, icon: null, updated_at: "2026-09-30T10:00:00Z", current_version: 5, pinned: false, ...over });
const S = (over: Partial<AttentionSummary> = {}): AttentionSummary => ({ addressed: [], addressed_v: null, new_replies: [], open_in: [], seen: 5, ...over });

describe("attention-model", () => {
  it("needs your eyes for an address, a version you have not seen, or a reply", () => {
    expect(needsEyes(A("a"), S({ addressed: ["t"], addressed_v: 5 }))).toBe(true);
    expect(needsEyes(A("a"), S({ seen: 4 }))).toBe(true);
    expect(needsEyes(A("a"), S({ new_replies: ["t"] }))).toBe(true);
    expect(needsEyes(A("a"), S({ open_in: ["t"] }))).toBe(false);
    expect(needsEyes(A("a"), S({ seen: null }))).toBe(false);
    expect(needsEyes(A("a"), undefined)).toBe(false);
  });

  it("orders markers: addressed, new version, replies, working, open", () => {
    expect(markers(A("a"), S({ addressed: ["t"], addressed_v: 5, seen: 4, new_replies: ["t", "u"], open_in: ["t", "u"] }), ["claude working on 2"])).toEqual([
      { kind: "you", text: "1 addressed in v5" }, { kind: "new", text: "v5 new" }, { kind: "rep", text: "2 new replies" },
      { kind: "ag", text: "claude working on 2" }, { kind: "oth", text: "2 open" },
    ]);
    expect(markers(A("a"), S({ new_replies: ["t"] }), [])).toEqual([{ kind: "rep", text: "1 new reply" }]);
  });

  it("groups needs first by recency, then the rest pinned first", () => {
    const list = [A("old", { updated_at: "2026-09-01" }), A("pin", { pinned: true, updated_at: "2026-08-01" }), A("eyes", { updated_at: "2026-09-02" }), A("new", { updated_at: "2026-09-30" })];
    const g = groups(list, { eyes: S({ seen: 4 }) });
    expect(g.needs.map(a => a.id)).toEqual(["eyes"]);
    expect(g.rest.map(a => a.id)).toEqual(["pin", "new", "old"]);
  });

  it("says which version you last saw", () => {
    expect([seenText(S({ seen: 4 })), seenText(S({ seen: null })), seenText(undefined)]).toEqual(["seen v4", null, null]);
  });
});
```

Run: `cd web && npx vitest run shell/src/view/attention-model.test.ts`
Expected: FAIL.

`web/shell/src/view/attention-model.ts`:

```ts
// Needs your eyes (spec §8, "Gallery"): from this viewer's attention, which
// artifacts float to the top and which markers each card carries. A
// never-viewed artifact does not need your eyes for its version alone.
import type { Artifact, AttentionSummary } from "../api";
import { orderArtifacts } from "./gallery-model";

export type Marker = { kind: "you" | "new" | "rep" | "ag" | "oth"; text: string };
const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

export function needsEyes(a: Artifact, att?: AttentionSummary): boolean {
  if (!att) return false;
  return att.addressed.length > 0 || att.new_replies.length > 0 || (att.seen !== null && a.current_version > att.seen);
}

export function markers(a: Artifact, att: AttentionSummary | undefined, working: string[]): Marker[] {
  const out: Marker[] = [];
  if (att?.addressed.length) out.push({ kind: "you", text: `${att.addressed.length} addressed in v${att.addressed_v ?? a.current_version}` });
  if (att && att.seen !== null && a.current_version > att.seen) out.push({ kind: "new", text: `v${a.current_version} new` });
  if (att?.new_replies.length) out.push({ kind: "rep", text: plural(att.new_replies.length, "new reply", "new replies") });
  for (const w of working) out.push({ kind: "ag", text: w });
  if (att?.open_in.length) out.push({ kind: "oth", text: `${att.open_in.length} open` });
  return out;
}

export function groups(list: Artifact[], att: Record<string, AttentionSummary>): { needs: Artifact[]; rest: Artifact[] } {
  const needs = list.filter(a => needsEyes(a, att[a.id])).sort((x, y) => y.updated_at.localeCompare(x.updated_at));
  const rest = orderArtifacts(list.filter(a => !needs.includes(a)));
  return { needs, rest };
}

export const seenText = (att?: AttentionSummary): string | null => (att?.seen != null ? `seen v${att.seen}` : null);
```

Run: `cd web && npx vitest run shell/src/view/attention-model.test.ts`
Expected: PASS.

- [ ] **Step 2: The gallery**

`events.ts`: rename `subscribeWorking` to `subscribeGallery`. It opens `/api/events?types=working,version,thread` and forwards `version` and `thread` events as well. Update `events.test.ts`. `working-feed.svelte.ts` calls it. Its `start(onResync)` gains `onChange: () => void`, which is called for `version` and `thread` events.

`ui/Gallery.svelte`:
- `let att = $state<Record<string, AttentionSummary>>({});`.
- `refresh` starts both requests at once, but never waits for attention before showing cards: `listArtifacts().then(a => { error = null; artifacts = a; feed.seed(a); feed.start(refresh, refreshSoon); }, e => { error = describe(e); })` and, beside it, `getAttention().then(t => { att = t; })`. Until attention arrives, `att` is `{}`, so `groups` puts every artifact under Everything else, ordered as before; the cards then regroup once. `getAttention` never rejects (it answers `{}` on failure).
- `refreshSoon` debounces `refresh` to at most once a second.
- With a query, the gallery shows one flat list, as before (`orderArtifacts(filterArtifacts(…))`). Without one, it shows `groups(shown, att)`:

```svelte
    {@const g = groups(shown, att)}
    {#if g.needs.length}
      <section class="grp needs">
        <h2><span class="sw" aria-hidden="true"></span>Needs your eyes<small>{g.needs.length}</small></h2>
        <p class="rule">A thread you're in was addressed and you haven't looked, or there's a version or reply you haven't seen.</p>
        <div class="cards">{#each g.needs as a (a.id)}{@render card(a)}{/each}</div>
      </section>
    {/if}
    <section class="grp rest">
      <h2><span class="sw" aria-hidden="true"></span>{g.needs.length ? "Everything else" : "Artifacts"}<small>pinned first, then most recent</small></h2>
      <div class="cards">{#each g.rest as a (a.id)}{@render card(a)}{/each}</div>
    </section>
```

  `card(a)` is a snippet rendering `GalleryCard` with these snippets:
  - `markers`: `{#each markers(a, att[a.id], chips(feed.byId[a.id] ?? [], names)) as m}<span class={["chip", m.kind]}>{m.text}</span>{/each}`;
  - `footer`: the `Roster` from Task 16, then `{#if seenText(att[a.id])}<span class="seen">{seenText(att[a.id])}</span>{/if}`.

  Task 16's working-chip snippet folds into `markers`.

In `gallery.test.ts`, stub `/api/viewers/me/attention` to return `{ artifacts: { aaaaaaaaaaaa: { addressed: [], addressed_v: null, new_replies: ["t"], open_in: ["t"], seen: 1 } } }`. Assert:
- `.grp.needs` holds the `Other` card, whose `.chip.rep` reads `1 new reply` and `.chip.oth` reads `1 open`;
- the `Pinned one` card is under `.grp.rest`;
- with the attention request failing, there is no `.grp.needs` and both cards show.

- [ ] **Step 3: Styles**

Append to `web/shell/src/theme.css`:

```css
/* Needs your eyes (spec §8). */
.needs h2 .sw { border-radius: 0 12px 12px 0; background: var(--you); }
.rest h2 .sw { border-radius: 50%; width: 14px; height: 14px; background: var(--border-strong); }
.needs .cards { grid-template-columns: repeat(auto-fit, minmax(380px, 1fr)); }
.needs .card .v { font-size: 48px; } .needs .card h3 { font-size: 20px; }
.chip.you { background: var(--you); color: var(--on-you); }
.chip.new { box-shadow: inset 0 0 0 1.5px var(--agent); color: var(--agent-ink); }
.chip.new::before { content: ""; width: 6px; height: 6px; border-radius: 50%; background: var(--agent); }
.chip.rep { box-shadow: inset 0 0 0 1.5px var(--border-strong); }
.chip.oth { box-shadow: inset 0 0 0 1.5px var(--you); }
.card .ft .seen { font-size: 11px; color: var(--muted); margin-left: auto; }
@media (max-width: 700px) { .needs .cards { grid-template-columns: 1fr; } .needs .card .v { font-size: 30px; } .needs .card h3 { font-size: 15px; } }
```

- [ ] **Step 4: Browser tests across two viewers**

`web/e2e/attention.spec.ts`:

```ts
import { test, expect, type Page } from "@playwright/test";
import { contentFrame, openArtifact, publishAs, publishNext, registerSession, startDaemon } from "./fixtures";

let d: Awaited<ReturnType<typeof startDaemon>>;
test.beforeAll(async () => { test.setTimeout(180_000); d = await startDaemon(); });
test.afterAll(async () => { await d?.stop(); });
const PAGE = "<main><h2>Quarterly goals</h2></main>";

async function name(page: Page, n: string) {
  await page.evaluate(n => fetch("/api/viewers/me", { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify({ display_name: n }) }), n);
}
async function comment(page: Page, aid: string, body: string, tid?: string): Promise<string> {
  return page.evaluate(async ([aid, body, tid]) => {
    if (tid) return (await (await fetch(`/api/artifacts/${aid}/threads/${tid}/comments`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ body }) })).json()).thread.id;
    const f = new FormData();
    f.set("anchor", JSON.stringify({ kind: "element", selector: "body > main > h2", quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, file: "index.html" }));
    f.set("body", body); f.set("version", "1");
    return (await (await fetch(`/api/artifacts/${aid}/threads`, { method: "POST", body: f })).json()).thread.id;
  }, [aid, body, tid] as const);
}

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: attention across viewers: addressed, new version, a mention, and looking clears`, async ({ browser }) => {
    const s = await registerSession(d.base, d.token, "claude", `att-${mode}`);
    const { artifact } = await publishAs(d.base, d.token, s.id, `Eyes ${mode}`, { "index.html": PAGE });
    const alex = await (await browser.newContext()).newPage();
    const mia = await (await browser.newContext()).newPage();
    await openArtifact(alex, d.base, artifact.id, 1, mode);
    await openArtifact(mia, d.base, artifact.id, 1, mode);
    await name(alex, "alex");
    await name(mia, "Mia");
    const tid = await comment(alex, artifact.id, "Two columns");
    await comment(alex, artifact.id, "@Mia which log?", tid);
    await publishNext(d.base, d.token, s.id, artifact.id, 1, { addresses: [tid] });
    await alex.goto(`${d.base}/`);
    const card = alex.locator(".grp.needs .card-wrap", { hasText: `Eyes ${mode}` });
    await expect(card.locator(".chip.you")).toHaveText("1 addressed in v2");
    await expect(card.locator(".chip.new")).toHaveText("v2 new");
    await expect(card.locator(".ft .seen")).toHaveText("seen v1");
    await mia.goto(`${d.base}/`);
    await expect(mia.locator(".grp.needs .card-wrap", { hasText: `Eyes ${mode}` }).locator(".chip.rep")).toHaveText("1 new reply");
    await openArtifact(alex, d.base, artifact.id, 2, mode);
    if (!(await alex.locator("aside.sidebar").isVisible())) await alex.getByRole("button", { name: /Threads/ }).first().click();
    await expect(alex.locator(`.thread-card[data-thread="${tid}"]`)).toBeVisible();
    await alex.waitForTimeout(2500);
    await alex.goto(`${d.base}/`);
    await expect(alex.locator(".grp.needs .card-wrap", { hasText: `Eyes ${mode}` })).toHaveCount(0);
    await expect(alex.locator(".grp.rest .card-wrap", { hasText: `Eyes ${mode}` }).locator(".chip.oth")).toHaveText("1 open");
    await alex.context().close();
    await mia.context().close();
  });
}
```

Run: `cd web && npx vitest run && npm run lint && npm run typecheck && npm run build && node scripts/bundle-size.mjs && npx playwright test e2e/attention.spec.ts e2e/working.spec.ts; echo "exit=$?"`
Expected: `exit=0`. If the `gallery` budget fails, move `attention-model` and the markers behind a dynamic `import()` that resolves with the attention request. Do not raise the budget.

- [ ] **Step 5: Screenshots and a look**

Make the `gallery` scene in `web/e2e/scenes.ts` produce a "needs your eyes" artifact:
1. name the viewer "alex";
2. post a thread as them on the seeded artifact;
3. view it (open `/a/<id>` once);
4. publish v2 addressing that thread;
5. go back to `/`.

Run: `cd web && CLAX_SHOTS=task-19 CLAX_SCENES=gallery npx playwright test e2e/shots.spec.ts`
Expected: PASS.

Report, against `concept-3-echo/shots/*-gallery.png`:
- Needs your eyes comes first, with its red-orange swatch, its rule line, and wider cards;
- Everything else follows, pinned first;
- the markers sit in their order and colours;
- each footer holds the roster and `seen v1`;
- at phone width there is one column and no sideways scroll.


Run: `cd web && npm run perf; echo "exit=$?"`
Expected: `exit=0`. Stop rule: if any of the five measures in `web/perf/budget.json` is over budget (link to first paint `firstPaint`, link to comment ready `commentReady`, frame paint `framePaint`, ready latency `readyLatency`, cold ready latency `coldLatency`, in either frame mode), stop and report all five numbers against their budgets. Lazy-load first; never raise a budget.

- [ ] **Step 6: Gates and staging**

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add web/shell/src/view/attention-model.ts web/shell/src/view/attention-model.test.ts web/e2e/attention.spec.ts web/shell/src/ui/Gallery.svelte \
  web/shell/src/ui/GalleryCard.svelte web/shell/src/ui/working-feed.svelte.ts web/shell/src/events.ts web/shell/src/events.test.ts web/shell/src/gallery.test.ts \
  web/shell/src/theme.css web/e2e/scenes.ts
git status --short   # staged; the controller commits ("Float what needs each viewer's eyes in the gallery, with markers, the roster and the last version seen")
```

---

### Task 20: Batch send in the store: one transaction, one batch, grouped payload

Decisions: the design record, §6 "Batch send" and §7 "Where a send goes". The rules this task implements are in "Design: batch send to agent" above.

**Files:**
- Create: `crates/clax-core/src/store/batches.rs`
- Modify: `crates/clax-core/src/store/mod.rs`, `crates/clax-core/src/store/migrations.rs` (migration 12), `crates/clax-core/src/store/feedback.rs` (`send_to_agent` split, `take_feedback` fills `batch`), `crates/clax-core/src/store/threads.rs` (`delete_thread_touched`), `crates/clax-core/src/store/artifacts.rs` (`delete_artifact`), `crates/clax-core/src/feedback.rs` (`FeedbackItem.batch`, `FeedbackBatch`, `render_items`), `crates/clax-core/src/lib.rs`

**Interfaces:**
- `clax_core::feedback::FeedbackBatch { id: String, size: u32, note: Option<String>, sent_by: String }`. `FeedbackItem` gains `batch: Option<FeedbackBatch>` (`#[serde(default)]`).
- `clax_core::store::batches::{MAX_BATCH_THREADS: usize = 20, MAX_BATCH_NOTE_CHARS: usize = 280, SendBatch { thread_ids: Vec<String>, note: Option<String>, sent_by: String }, BatchResult { batch: FeedbackBatch, sent: Vec<String>, unchanged: Vec<String>, touched: Touched }, ThreadSend { batch_id: String, size: u32, note: Option<String>, sent_by: String, sent_at: String }}`.
- `Store::send_batch(&self, aid: &ArtifactId, b: SendBatch) -> Result<BatchResult>` and `Store::thread_sends(&self, thread_id: &str) -> Result<Vec<ThreadSend>>`.
- `render_items` output. A run of items from one batch is led by `[clax] N comments on "<title>", sent together by <name>.`, followed by ` Note: "<note>"` when the batch has a note. N counts that batch's items in this delivery. Items with no batch render as before.

- [ ] **Step 1: Failing tests**

`crates/clax-core/src/store/batches.rs`, tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::test_util::{anchor, artifact, session, store};
    use crate::{NewThread, Store, TakeFeedback, Tier};

    fn thread(st: &Store, id: &ArtifactId, body: &str) -> String {
        st.create_thread(id, NewThread { version_n: 1, anchor: anchor(), author_name: "Alex".into(), author_public_id: None, body: body.into(), clip: None, via_page: false })
            .unwrap()
            .id
    }
    fn batch(ids: &[&String], note: Option<&str>) -> SendBatch {
        SendBatch { thread_ids: ids.iter().map(|s| s.to_string()).collect(), note: note.map(String::from), sent_by: "Alex".into() }
    }
    fn code(e: crate::CoreError) -> &'static str {
        match e { crate::CoreError::Invalid { code, .. } => code, other => panic!("{other:?}") }
    }

    #[test]
    fn a_batch_sends_every_thread_in_one_transaction_and_one_delivery() {
        let (_d, st) = store();
        let sid = session(&st, "claude", "b1");
        let id = artifact(&st, Some(&sid));
        st.ensure_watch(&sid, &id).unwrap();
        let ts: Vec<String> = (0..3).map(|i| thread(&st, &id, &format!("c{i}"))).collect();
        let r = st.send_batch(&id, batch(&ts.iter().collect::<Vec<_>>(), Some("  Before the demo "))).unwrap();
        assert_eq!(r.sent, ts);
        assert_eq!((r.batch.size, r.batch.note.as_deref()), (3, Some("Before the demo")));
        for t in &ts {
            assert!(st.get_thread(t).unwrap().unwrap().sent_to_agent);
            assert_eq!(st.thread_sends(t).unwrap()[0].size, 3);
        }
        let (items, _) = st.take_feedback(&TakeFeedback { session_id: sid.clone(), tier: Tier::Piggyback, artifact_id: None, include_resends: true }, "http://h").unwrap();
        assert_eq!(items.len(), 3);
        assert!(items.iter().all(|i| i.batch.as_ref().map(|b| b.id.as_str()) == Some(r.batch.id.as_str())));
    }

    #[test]
    fn any_bad_thread_fails_the_whole_batch_and_writes_nothing() {
        let (_d, st) = store();
        let id = artifact(&st, None);
        let other = artifact(&st, None);
        let (a, b) = (thread(&st, &id, "a"), thread(&st, &id, "b"));
        let foreign = thread(&st, &other, "x");
        assert_eq!(code(st.send_batch(&id, batch(&[&a, &foreign], None)).unwrap_err()), "unknown_thread");
        let gone = crate::new_ulid();
        assert_eq!(code(st.send_batch(&id, batch(&[&a, &gone], None)).unwrap_err()), "unknown_thread");
        st.resolve_thread(&b, "viewer:anonymous").unwrap();
        assert_eq!(code(st.send_batch(&id, batch(&[&a, &b], None)).unwrap_err()), "thread_resolved");
        assert!(!st.get_thread(&a).unwrap().unwrap().sent_to_agent, "nothing was written");
        assert!(st.thread_sends(&a).unwrap().is_empty());
        assert_eq!(code(st.send_batch(&id, batch(&[], None)).unwrap_err()), "invalid_args");
        let many: Vec<String> = (0..21).map(|_| crate::new_ulid()).collect();
        assert_eq!(code(st.send_batch(&id, batch(&many.iter().collect::<Vec<_>>(), None)).unwrap_err()), "invalid_args");
        assert_eq!(code(st.send_batch(&id, batch(&[&a], Some(&"n".repeat(281)))).unwrap_err()), "note_too_long");
    }

    #[test]
    fn already_sent_threads_are_accepted_and_reported_unchanged() {
        let (_d, st) = store();
        let id = artifact(&st, None);
        let (a, b) = (thread(&st, &id, "a"), thread(&st, &id, "b"));
        st.send_to_agent(&a).unwrap();
        let r = st.send_batch(&id, batch(&[&a, &b, &b], None)).unwrap();
        assert_eq!((r.sent, r.unchanged), (vec![b.clone()], vec![a.clone()]));
        assert_eq!(r.batch.size, 1);
        assert_eq!(code(st.send_batch(&id, batch(&[&a, &b], None)).unwrap_err()), "nothing_to_send");
    }

    #[test]
    fn deleting_a_thread_drops_it_from_its_batches() {
        let (_d, st) = store();
        let id = artifact(&st, None);
        let a = thread(&st, &id, "a");
        st.send_batch(&id, batch(&[&a], None)).unwrap();
        st.delete_thread(&a).unwrap();
        assert!(st.thread_sends(&a).unwrap().is_empty());
    }
}
```

Add to the tests in `crates/clax-core/src/feedback.rs`, using its `item()` helper (a "Quarterly Review" item). The helper gains `batch: None`:

```rust
    #[test]
    fn a_batch_is_led_by_one_line_with_its_note() {
        let b = FeedbackBatch { id: "B".into(), size: 2, note: Some("Before the \"demo\"".into()), sent_by: "Alex".into() };
        let one = FeedbackItem { thread_id: "T1".into(), batch: Some(b.clone()), ..item() };
        let two = FeedbackItem { thread_id: "T2".into(), batch: Some(b), ..item() };
        let lone = FeedbackItem { thread_id: "T3".into(), ..item() };
        let text = render_items(&[one, two, lone]);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "[clax] 3 comments sent to you:");
        assert_eq!(lines[1], "[clax] 2 comments on \"Quarterly Review\", sent together by Alex. Note: \"Before the \\\"demo\\\"\"");
        assert!(lines[2].starts_with("[clax] Comment sent to you on \"Quarterly Review\""));
        assert_eq!(text.matches("sent together").count(), 1);
        assert!(text.contains("thread T3"));
    }

    #[test]
    fn items_without_a_batch_render_as_before() {
        assert_eq!(render_items(&[item()]), format!("[clax] 1 comment sent to you:\n{}", render_item(&item())));
    }
```

Run: `cargo test -p clax-core batches && cargo test -p clax-core feedback`
Expected: FAIL to compile.

- [ ] **Step 2: Migration 12**

Append to `MIGRATIONS`:

```rust
    // 12: batch sends: the batch (its note and who sent it), its threads, and
    // the batch each feedback row came from.
    "CREATE TABLE send_batches (
        id TEXT PRIMARY KEY,
        artifact_id TEXT NOT NULL,
        note TEXT,
        sent_by TEXT NOT NULL,
        size INTEGER NOT NULL,
        created_at TEXT NOT NULL
    );
    CREATE TABLE batch_threads (
        batch_id TEXT NOT NULL REFERENCES send_batches(id),
        thread_id TEXT NOT NULL REFERENCES threads(id),
        PRIMARY KEY (batch_id, thread_id)
    );
    CREATE INDEX batch_threads_by_thread ON batch_threads(thread_id);
    ALTER TABLE feedback ADD COLUMN batch_id TEXT;",
```

- [ ] **Step 3: Implement**

`store/feedback.rs`: move the body of `send_to_agent`'s transaction into `pub(crate) fn send_in(tx: &Transaction<'_>, thread_id: &str, batch_id: Option<&str>, touched: &mut Touched) -> Result<usize>`. It returns how many rows it inserted. It is the same code, with `batch_id` written into the new column of every row it inserts. `send_to_agent` calls it with `None`. The `take_feedback` query gains `f.batch_id` and a `LEFT JOIN send_batches b ON b.id = f.batch_id` (selecting `b.size, b.note, b.sent_by`). The row mapper fills `batch: Some(FeedbackBatch { .. })` when `f.batch_id` is not null.

`feedback.rs`: add `FeedbackBatch` (derive `Clone, Debug, PartialEq, Serialize, Deserialize`) and the field. Then `render_items`:

```rust
/// `[clax] N comments sent to you:` (`1 comment` for one), then each item,
/// separated by a blank line. A run of items from one batch is led by one
/// line naming how many of its comments this delivery holds, who sent them,
/// and the batch's note, quoted like a comment body.
pub fn render_items(items: &[FeedbackItem]) -> String {
    let n = items.len();
    let noun = if n == 1 { "comment" } else { "comments" };
    let mut parts: Vec<String> = Vec::new();
    let mut i = 0;
    while i < items.len() {
        if let Some(b) = &items[i].batch {
            let run = items[i..].iter().take_while(|x| x.batch.as_ref().map(|y| &y.id) == Some(&b.id)).count();
            let noun = if run == 1 { "comment" } else { "comments" };
            let note = b.note.as_deref().map(|t| format!(" Note: {}", quoted(t))).unwrap_or_default();
            let lead = format!(
                "[clax] {run} {noun} on {}, sent together by {}.{note}",
                quoted(&items[i].artifact_title),
                display_name(&b.sent_by),
            );
            let body = items[i..i + run].iter().map(render_item).collect::<Vec<_>>().join("\n\n");
            parts.push(format!("{lead}\n{body}"));
            i += run;
        } else {
            parts.push(render_item(&items[i]));
            i += 1;
        }
    }
    format!("[clax] {n} {noun} sent to you:\n{}", parts.join("\n\n"))
}
```

`store/batches.rs`, above the tests:

```rust
//! Batch send to agent (spec §10 "Batch send"): several threads sent in one
//! transaction, as one batch with an optional note, delivered together.

use super::Store;
use crate::feedback::{FeedbackBatch, Touched};
use crate::working::clean_line;
use crate::{ArtifactId, CoreError, Result, new_ulid};
use rusqlite::{OptionalExtension, params};

/// Most threads one batch holds (as many as one working record names).
pub const MAX_BATCH_THREADS: usize = crate::working::MAX_WORKING_THREADS;
/// Longest batch note, in characters, after whitespace is collapsed.
pub const MAX_BATCH_NOTE_CHARS: usize = 280;

#[derive(Clone, Debug)]
pub struct SendBatch {
    pub thread_ids: Vec<String>,
    pub note: Option<String>,
    /// The sender's display name, as a viewer comment's author.
    pub sent_by: String,
}

#[derive(Clone, Debug)]
pub struct BatchResult {
    pub batch: FeedbackBatch,
    /// Threads that got new feedback rows (the batch), in request order.
    pub sent: Vec<String>,
    /// Threads already sent with nothing new to send.
    pub unchanged: Vec<String>,
    pub touched: Touched,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct ThreadSend {
    pub batch_id: String,
    pub size: u32,
    pub note: Option<String>,
    pub sent_by: String,
    pub sent_at: String,
}

impl Store {
    /// Sends `b.thread_ids` (duplicates dropped, request order kept) as one
    /// batch, all or nothing. Errors, checked in this order before anything is
    /// written: `invalid_args` (none, more than [`MAX_BATCH_THREADS`], or not
    /// ULIDs), `note_too_long`, `unknown_thread` (missing or on another
    /// artifact; the message names them), `thread_resolved` (names them), and
    /// `nothing_to_send` when no thread has a viewer comment without a row.
    /// Already-sent threads are accepted: they send only what they have not
    /// sent (as [`Store::send_to_agent`]) and are reported `unchanged` when
    /// that is nothing.
    pub fn send_batch(&self, aid: &ArtifactId, b: SendBatch) -> Result<BatchResult> {
        let mut ids: Vec<String> = Vec::new();
        for t in b.thread_ids {
            if !ids.contains(&t) {
                ids.push(t);
            }
        }
        if ids.is_empty() || ids.len() > MAX_BATCH_THREADS {
            return Err(CoreError::invalid("invalid_args", format!("send 1 to {MAX_BATCH_THREADS} threads")));
        }
        if let Some(bad) = ids.iter().find(|t| !crate::is_ulid(t)) {
            return Err(CoreError::invalid("invalid_args", format!("'{bad}' is not a thread ID")));
        }
        let note = match b.note.as_deref() {
            Some(n) => match clean_line(n, MAX_BATCH_NOTE_CHARS) {
                (_, true) => return Err(CoreError::invalid("note_too_long", format!("a note is at most {MAX_BATCH_NOTE_CHARS} characters"))),
                (n, false) => n,
            },
            None => None,
        };
        self.with_tx(|tx| {
            let mut unknown = Vec::new();
            let mut resolved = Vec::new();
            for t in &ids {
                let row: Option<(String, String)> = tx
                    .query_row("SELECT artifact_id, status FROM threads WHERE id = ?1", params![t], |r| Ok((r.get(0)?, r.get(1)?)))
                    .optional()?;
                match row {
                    Some((a, _)) if a != aid.as_str() => unknown.push(t.clone()),
                    None => unknown.push(t.clone()),
                    Some((_, s)) if s == "resolved" => resolved.push(t.clone()),
                    Some(_) => {}
                }
            }
            if !unknown.is_empty() {
                return Err(CoreError::invalid("unknown_thread", format!("not threads of {aid}: {}", unknown.join(", "))));
            }
            if !resolved.is_empty() {
                return Err(CoreError::invalid("thread_resolved", format!("resolved threads cannot be sent: {}", resolved.join(", "))));
            }
            let batch_id = new_ulid();
            let mut touched = Touched::default();
            let (mut sent, mut unchanged) = (Vec::new(), Vec::new());
            for t in &ids {
                if super::feedback::send_in(tx, t, Some(&batch_id), &mut touched)? > 0 { sent.push(t.clone()) } else { unchanged.push(t.clone()) }
            }
            if sent.is_empty() {
                return Err(CoreError::invalid("nothing_to_send", "every thread was already sent with nothing new"));
            }
            tx.execute(
                "INSERT INTO send_batches (id, artifact_id, note, sent_by, size, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![batch_id, aid.as_str(), note, b.sent_by, sent.len() as u32, Store::now()],
            )?;
            for t in &sent {
                tx.execute("INSERT INTO batch_threads (batch_id, thread_id) VALUES (?1, ?2)", params![batch_id, t])?;
            }
            let batch = FeedbackBatch { id: batch_id, size: sent.len() as u32, note, sent_by: b.sent_by };
            Ok(BatchResult { batch, sent, unchanged, touched })
        })
    }

    /// The batches that sent `thread_id`, oldest first (its send history).
    pub fn thread_sends(&self, thread_id: &str) -> Result<Vec<ThreadSend>> {
        self.with_conn(|c| {
            let mut stmt = c.prepare(
                "SELECT b.id, b.size, b.note, b.sent_by, b.created_at FROM batch_threads t JOIN send_batches b ON b.id = t.batch_id
                 WHERE t.thread_id = ?1 ORDER BY b.created_at, b.id",
            )?;
            Ok(stmt
                .query_map(params![thread_id], |r| Ok(ThreadSend { batch_id: r.get(0)?, size: r.get(1)?, note: r.get(2)?, sent_by: r.get(3)?, sent_at: r.get(4)? }))?
                .collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }
}
```

The `send_batches` row is inserted after the feedback rows in the same transaction. SQLite checks the `batch_threads` foreign key at insert time, and `feedback.batch_id` has no foreign key, so this order is valid. The whole transaction commits or rolls back as one.

`store/threads.rs::delete_thread_touched`: add `tx.execute("DELETE FROM batch_threads WHERE thread_id = ?1", params![thread_id])?;` before the thread row is deleted. `store/artifacts.rs::delete_artifact`: in its transaction, delete `batch_threads` rows whose batch is on the artifact, then its `send_batches` rows. The doctor's hard delete of broken artifact rows (the same file, the list it deletes before `threads`) also deletes those `batch_threads` and `send_batches` rows, since `batch_threads` holds a foreign key to the threads it removes.

Register `pub mod batches;` in `store/mod.rs`, and re-export `FeedbackBatch` from `lib.rs` next to `FeedbackItem`.

Run: `cargo test -p clax-core`
Expected: PASS. Update exact `FeedbackItem` JSON expectations elsewhere with `"batch": null`, and change nothing else.

- [ ] **Step 4: Gates and staging**

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add crates/clax-core/src/store/batches.rs crates/clax-core/src/store/mod.rs crates/clax-core/src/store/migrations.rs crates/clax-core/src/store/feedback.rs \
  crates/clax-core/src/store/threads.rs crates/clax-core/src/store/artifacts.rs crates/clax-core/src/feedback.rs crates/clax-core/src/lib.rs
git add -u crates/clax-core
git status --short   # staged; the controller commits ("Send several threads to the agent as one batch with an optional note")
```

---

### Task 21: The batch send route, and the send target

The batch route from "Design: batch send to agent", plus the optional `to` (an agent handle, Task 15) on both the batch and the single send, and the thread's target (spec §10, "Data flow"):
- With `to`, only that agent's live session gets rows, and the session becomes the thread's target (`threads.target_session_id`).
- Without `to`, the rows fan out to the live owner and every live watcher (or wait untargeted when none is live), and the thread's target is cleared.
- A later viewer comment on a sent thread follows the thread's target while that session is live, and fans out once it has ended.

**Files:**
- Modify: `crates/clax-server/src/routes/threads.rs` (new `send_batch` handler; `send` takes an optional `{to}`), `crates/clax-server/src/routes/mod.rs`, `crates/clax-server/src/feedback.rs` (`thread_view` gains `sends`), `crates/clax-core/src/store/migrations.rs` (migration 13), `crates/clax-core/src/store/feedback.rs` (`SendTarget`, `send_to`), `crates/clax-core/src/store/batches.rs` (`SendBatch.to`), `crates/clax-core/src/lib.rs`
- Create: `crates/clax-server/tests/api_batch.rs`

**Interfaces:**
- `POST /api/artifacts/<aid>/threads:send` with body `{thread_ids: [ULID], note?: string, to?: agent handle}` (`deny_unknown_fields`). Auth is exactly that of `POST .../threads/<tid>/send`: no token, `SameOrigin` (a foreign `Origin` is 403 `forbidden_origin`), and the optional viewer cookie names the sender (`crate::viewer::author_name`). This is the route the sidebar calls, as its single send does. The capability's `sendToClaude` keeps calling the single route (see "Design: batch send to agent").
- `200 {batch: {id, size, note, sent_by}, sent: [ID], unchanged: [ID], threads: [thread view]}`. Errors: 400 `invalid_args`, 400 `note_too_long`, 400 `unknown_agent`, 400 `unknown_thread`, 400 `thread_resolved`, 409 `nothing_to_send`, 404 `not_found` (unknown or deleted artifact). In every error case nothing is written.
- Thread views gain `sends: [{batch_id, size, note, sent_by, sent_at}]`.
- `POST .../threads/<tid>/send` takes an optional JSON body `{to}` (an agent handle); 400 `unknown_agent` when it names no live owner or watcher of the artifact. Without a body, or without `to`, it sends to everyone and clears the thread's target.
- `clax_core::store::feedback::SendTarget<'a> { Agent(&'a str) /* a session ID */, Everyone, Thread }`, re-exported from `lib.rs`. `Store::send_to(tid, SendTarget) -> Result<(Thread, Touched)>`. `send_to_agent(tid)` stays, as `send_to(tid, SendTarget::Thread)`: the path for later comments and for `@agent`.
- Migration 13: `threads.target_session_id TEXT`, stored and never served (no thread view, event or capability answer carries it).
- After the commit: one `feedback::apply` for the whole batch's `touched` (one wake-up, one `codex queue` dispatch per target), then a `thread` event per sent thread.

- [ ] **Step 1: Failing tests**

`crates/clax-server/tests/api_batch.rs`:

```rust
mod common;
use clax_core::working::{ManualClock, Working};
use common::TestServer;
use serde_json::{Value, json};
use std::sync::Arc;

async fn setup(ts: &TestServer) -> (String, String, Vec<String>) {
    let s = ts.register_session("claude", "batch-1").await;
    let sid = s["id"].as_str().unwrap().to_string();
    let a = ts.publish_as(&sid, "Quarterly Review", "<main><h2>Quarterly goals</h2></main>").await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let mut tids = Vec::new();
    for body in ["one", "two", "three"] {
        tids.push(ts.thread(&aid, 1, body).await["id"].as_str().unwrap().to_string());
    }
    (sid, aid, tids)
}

async fn send(ts: &TestServer, aid: &str, body: Value) -> reqwest::Response {
    ts.client.post(format!("{}/api/artifacts/{aid}/threads:send", ts.base)).json(&body).send().await.unwrap()
}

fn code(v: &Value) -> &str {
    v["error"]["code"].as_str().unwrap()
}

#[tokio::test]
async fn a_batch_is_delivered_as_one_group_led_by_its_note_through_every_tier() {
    for tier in ["piggyback", "stop_hook", "prompt_hook", "wait"] {
        let ts = TestServer::spawn().await;
        let (sid, aid, tids) = setup(&ts).await;
        let named = ts.viewer(Some("Alex")).await;
        let res = ts.client.post(format!("{}/api/artifacts/{aid}/threads:send", ts.base))
            .header("cookie", format!("clax_viewer={}", named.cookie))
            .json(&json!({"thread_ids": tids, "note": "Before the demo"})).send().await.unwrap();
        assert_eq!(res.status(), 200, "{tier}");
        let v: Value = res.json().await.unwrap();
        assert_eq!(v["sent"], json!(tids));
        assert_eq!(v["batch"]["sent_by"], "Alex");
        assert_eq!(v["threads"][0]["sends"][0]["note"], "Before the demo");
        let got: Value = ts.get_authed(&format!("/api/sessions/{sid}/feedback?tier={tier}")).await.json().await.unwrap();
        let text = got["text"].as_str().unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "[clax] 3 comments sent to you:", "{tier}");
        assert_eq!(lines[1], "[clax] 3 comments on \"Quarterly Review\", sent together by Alex. Note: \"Before the demo\"", "{tier}");
        for t in &tids {
            assert!(text.contains(t.as_str()), "{tier}: {t}");
        }
    }
}

#[tokio::test]
async fn a_bad_thread_fails_the_batch_and_nothing_is_sent() {
    let ts = TestServer::spawn().await;
    let (_sid, aid, tids) = setup(&ts).await;
    let other = ts.publish("Other", &[("index.html", "<p>")]).await;
    let oid = other["artifact"]["id"].as_str().unwrap().to_string();
    let foreign = ts.thread(&oid, 1, "elsewhere").await["id"].as_str().unwrap().to_string();
    let r = send(&ts, &aid, json!({"thread_ids": [tids[0], foreign]})).await;
    assert_eq!(r.status(), 400);
    let v: Value = r.json().await.unwrap();
    assert_eq!(code(&v), "unknown_thread");
    assert!(v["error"]["message"].as_str().unwrap().contains(&foreign));
    ts.client.post(format!("{}/api/artifacts/{aid}/threads/{}/resolve", ts.base, tids[1])).send().await.unwrap();
    let v: Value = send(&ts, &aid, json!({"thread_ids": [tids[0], tids[1]]})).await.json().await.unwrap();
    assert_eq!(code(&v), "thread_resolved");
    let t: Value = ts.get(&format!("/api/artifacts/{aid}/threads/{}", tids[0])).await.json().await.unwrap();
    assert_eq!(t["thread"]["sent_to_agent"], false, "nothing was written");
    assert_eq!(t["thread"]["sends"], json!([]));
    let v: Value = send(&ts, &aid, json!({"thread_ids": []})).await.json().await.unwrap();
    assert_eq!(code(&v), "invalid_args");
    let v: Value = send(&ts, &aid, json!({"thread_ids": [tids[0]], "note": "n".repeat(281)})).await.json().await.unwrap();
    assert_eq!(code(&v), "note_too_long");
    assert_eq!(send(&ts, "7q3k9mzx2b4t", json!({"thread_ids": [tids[0]]})).await.status(), 404);
}

#[tokio::test]
async fn already_sent_threads_are_reported_and_a_batch_of_nothing_is_refused() {
    let ts = TestServer::spawn().await;
    let (_sid, aid, tids) = setup(&ts).await;
    ts.send_thread(&aid, &tids[0]).await;
    let v: Value = send(&ts, &aid, json!({"thread_ids": [tids[0], tids[1]]})).await.json().await.unwrap();
    assert_eq!((v["sent"].clone(), v["unchanged"].clone()), (json!([tids[1]]), json!([tids[0]])));
    let r = send(&ts, &aid, json!({"thread_ids": [tids[0], tids[1]]})).await;
    assert_eq!(r.status(), 409);
    assert_eq!(code(&r.json().await.unwrap()), "nothing_to_send");
}

#[tokio::test]
async fn the_batch_route_refuses_a_foreign_origin_like_the_single_send() {
    let ts = TestServer::spawn().await;
    let (_sid, aid, tids) = setup(&ts).await;
    let r = ts.client.post(format!("{}/api/artifacts/{aid}/threads:send", ts.base))
        .header("origin", "http://evil.example").json(&json!({"thread_ids": [tids[0]]})).send().await.unwrap();
    assert_eq!(r.status(), 403);
    assert_eq!(code(&r.json().await.unwrap()), "forbidden_origin");
}

#[tokio::test]
async fn a_delivered_batch_marks_every_thread_working_and_a_publish_links_them_all() {
    let clock = Arc::new(ManualClock::at("2026-09-30T10:00:00Z"));
    let c = clock.clone();
    let ts = TestServer::spawn_with(move |s| s.working = Arc::new(Working::new(c))).await;
    let (sid, aid, tids) = setup(&ts).await;
    send(&ts, &aid, json!({"thread_ids": tids})).await;
    ts.get_authed(&format!("/api/sessions/{sid}/feedback?tier=piggyback")).await;
    let w: Value = ts.get(&format!("/api/artifacts/{aid}/working")).await.json().await.unwrap();
    assert_eq!(w["working"][0]["thread_ids"], json!(tids));
    let res = ts.authed(ts.client.post(format!("{}/api/artifacts/{aid}/versions", ts.base)))
        .header("x-clax-session", &sid)
        .json(&json!({"if_version": 1, "files": {"index.html": {"content": "<p>2</p>", "encoding": "utf8"}}}))
        .send().await.unwrap();
    let v: Value = res.json().await.unwrap();
    assert_eq!(v["version"]["addresses"], json!(tids));
}
```

Run: `cargo test -p clax-server --test api_batch`
Expected: FAIL (404: no such route).

- [ ] **Step 2: Implement**

`routes/threads.rs`:

```rust
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatchBody {
    thread_ids: Vec<String>,
    #[serde(default)]
    note: Option<String>,
}

/// `POST /api/artifacts/<aid>/threads:send`: sends several threads to the
/// agent as one batch ([`Store::send_batch`]), all or nothing. The same
/// access as the single send (no token; a foreign `Origin` is refused); the
/// viewer cookie names the sender. One fan-out for the whole batch, so every
/// tier hands its rows over together.
pub async fn send_batch(
    State(s): State<AppState>,
    headers: HeaderMap,
    _o: SameOrigin,
    viewer: ViewerCookie,
    aid: Result<Path<String>, PathRejection>,
    req: Result<Json<BatchBody>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let id = parse_id(&path(aid)?)?;
    let b = body(req)?;
    let ctx = s.feedback_ctx();
    let with_path = has_token(&headers, &s.token);
    let v = s
        .store_call(move |st| {
            st.get_artifact(&id)?.ok_or(CoreError::NotFound)?;
            let sent_by = crate::viewer::author_name(st, viewer.0.as_deref())?;
            let r = st.send_batch(&id, clax_core::store::batches::SendBatch { thread_ids: b.thread_ids, note: b.note, sent_by })?;
            apply(&ctx, st, &r.touched);
            let mut views = Vec::new();
            for tid in &r.sent {
                let t = thread_of(st, &id, tid)?;
                publish_thread(&ctx, st, &t)?;
                views.push(thread_view(st, &t, ctx.codex_push(), with_path)?);
            }
            Ok(json!({"batch": r.batch, "sent": r.sent, "unchanged": r.unchanged, "threads": views}))
        })
        .await?;
    Ok(Json(v))
}
```

`nothing_to_send` must answer 409. In `crates/clax-server/src/error.rs`, the `From<CoreError>` arm `CoreError::Invalid { code, message } => ApiError::bad_request(code, message)` becomes:

```rust
            CoreError::Invalid { code: "nothing_to_send", message } => ApiError::new(StatusCode::CONFLICT, "nothing_to_send", message),
            CoreError::Invalid { code, message } => ApiError::bad_request(code, message),
```

`routes/mod.rs`: in `api_fast`, add `.route("/api/artifacts/{aid}/threads:send", post(threads::send_batch))`.

`feedback.rs::thread_view`: add `v["sends"] = json!(st.thread_sends(&t.id)?);`.

Run: `cargo test -p clax-server`
Expected: PASS. Exact thread-view assertions elsewhere gain `"sends": []`.

- [ ] **Step 3: The send target, test first**

Append to `crates/clax-server/tests/api_batch.rs`:

```rust
#[tokio::test]
async fn to_sends_only_to_that_agent_and_an_unknown_handle_writes_nothing() {
    let ts = TestServer::spawn().await;
    let (owner, aid, tids) = setup(&ts).await;
    let w = ts.register_session("codex", "batch-watch").await;
    let watcher = w["id"].as_str().unwrap().to_string();
    let res = ts.authed(ts.client.put(format!("{}/api/sessions/{watcher}/watches/{aid}", ts.base))).send().await.unwrap();
    assert!(res.status().is_success());
    let a: Value = ts.get(&format!("/api/artifacts/{aid}")).await.json().await.unwrap();
    let handle = a["artifact"]["participants"]["agents"].as_array().unwrap().iter()
        .find(|x| x["harness"] == "codex").unwrap()["handle"].as_str().unwrap().to_string();
    let bad = send(&ts, &aid, json!({"thread_ids": [tids[0]], "to": "a_00000000000000000000aa"})).await;
    assert_eq!(bad.status(), 400);
    assert_eq!(bad.json::<Value>().await.unwrap()["error"]["code"], "unknown_agent");
    let t: Value = ts.get(&format!("/api/artifacts/{aid}/threads/{}", tids[0])).await.json().await.unwrap();
    assert_eq!(t["thread"]["sent_to_agent"], false, "nothing was written");
    assert_eq!(send(&ts, &aid, json!({"thread_ids": [tids[0], tids[1]], "to": handle})).await.status(), 200);
    let to_watcher: Value = ts.get_authed(&format!("/api/sessions/{watcher}/feedback?tier=piggyback")).await.json().await.unwrap();
    let to_owner: Value = ts.get_authed(&format!("/api/sessions/{owner}/feedback?tier=piggyback")).await.json().await.unwrap();
    assert_eq!(to_watcher["feedback"].as_array().unwrap().len(), 2);
    assert_eq!(to_owner["feedback"].as_array().unwrap().len(), 0);
    let single = ts.client.post(format!("{}/api/artifacts/{aid}/threads/{}/send", ts.base, tids[2])).json(&json!({"to": handle})).send().await.unwrap();
    assert_eq!(single.status(), 200);
    let again: Value = ts.get_authed(&format!("/api/sessions/{owner}/feedback?tier=piggyback")).await.json().await.unwrap();
    assert_eq!(again["feedback"].as_array().unwrap().len(), 0, "the single send with to skips the owner too");
}

async fn watcher_of(ts: &TestServer, aid: &str, hsid: &str) -> (String, String) {
    let w = ts.register_session("codex", hsid).await;
    let sid = w["id"].as_str().unwrap().to_string();
    let res = ts.authed(ts.client.put(format!("{}/api/sessions/{sid}/watches/{aid}", ts.base))).send().await.unwrap();
    assert!(res.status().is_success());
    let a: Value = ts.get(&format!("/api/artifacts/{aid}")).await.json().await.unwrap();
    let handle = a["artifact"]["participants"]["agents"].as_array().unwrap().iter()
        .find(|x| x["harness"] == "codex").unwrap()["handle"].as_str().unwrap().to_string();
    (sid, handle)
}

async fn taken(ts: &TestServer, sid: &str) -> Vec<String> {
    let v: Value = ts.get_authed(&format!("/api/sessions/{sid}/feedback?tier=piggyback")).await.json().await.unwrap();
    v["feedback"].as_array().unwrap().iter().map(|f| f["body"].as_str().unwrap().to_string()).collect()
}

async fn reply(ts: &TestServer, aid: &str, tid: &str, body: &str) {
    let res = ts.client.post(format!("{}/api/artifacts/{aid}/threads/{tid}/comments", ts.base)).json(&json!({"body": body})).send().await.unwrap();
    assert_eq!(res.status(), 201);
}

#[tokio::test]
async fn later_comments_follow_the_agent_the_thread_was_sent_to() {
    let ts = TestServer::spawn().await;
    let (owner, aid, tids) = setup(&ts).await;
    let (watcher, handle) = watcher_of(&ts, &aid, "follow-watch").await;
    assert_eq!(send(&ts, &aid, json!({"thread_ids": [tids[0]], "to": handle})).await.status(), 200);
    assert_eq!(taken(&ts, &watcher).await, ["one"]);
    reply(&ts, &aid, &tids[0], "and the footer").await;
    reply(&ts, &aid, &tids[0], "@agent also the header").await;
    assert_eq!(taken(&ts, &watcher).await, ["and the footer", "@agent also the header"], "later comments, @agent or not, follow the target");
    assert!(taken(&ts, &owner).await.is_empty(), "the owner never gets a comment sent to another agent");
    let t: Value = ts.get(&format!("/api/artifacts/{aid}/threads/{}", tids[0])).await.json().await.unwrap();
    assert!(!t.to_string().contains(&watcher), "the target is never served");
}

#[tokio::test]
async fn once_the_target_ends_later_comments_go_to_everyone_and_a_send_without_to_clears_it() {
    let ts = TestServer::spawn().await;
    let (owner, aid, tids) = setup(&ts).await;
    let (watcher, handle) = watcher_of(&ts, &aid, "ended-watch").await;
    send(&ts, &aid, json!({"thread_ids": [tids[0], tids[1]], "to": handle})).await;
    taken(&ts, &watcher).await;
    let res = ts.authed(ts.client.patch(format!("{}/api/sessions/{watcher}", ts.base))).json(&json!({"ended": true})).send().await.unwrap();
    assert!(res.status().is_success());
    reply(&ts, &aid, &tids[0], "still there?").await;
    assert_eq!(taken(&ts, &owner).await, ["still there?"], "the target ended: the comment fans out");
    let (second, _) = watcher_of(&ts, &aid, "second-watch").await;
    let res = ts.client.post(format!("{}/api/artifacts/{aid}/threads/{}/send", ts.base, tids[1])).send().await.unwrap();
    assert_eq!(res.status(), 200, "a send without to");
    reply(&ts, &aid, &tids[1], "for everyone").await;
    assert_eq!(taken(&ts, &owner).await, ["for everyone"]);
    assert_eq!(taken(&ts, &second).await, ["for everyone"], "no target: the owner and every live watcher");
}
```

Run: `cargo test -p clax-server --test api_batch to_sends`
Expected: FAIL.

Implement:
- Migration 13, appended to `MIGRATIONS`:

```rust
    // 13: the session a thread was last sent to with `to`. Later viewer
    // comments on the thread follow it while it is live. Never served.
    "ALTER TABLE threads ADD COLUMN target_session_id TEXT;",
```

- `store/feedback.rs`, beside `live_targets`:

```rust
/// Where a send's rows go (spec §10, "Data flow").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendTarget<'a> {
    /// This live owner or watcher only (a session ID); it becomes the thread's target.
    Agent(&'a str),
    /// Every live owner and watcher, or untargeted when none is live; clears the thread's target.
    Everyone,
    /// A later comment, or `@agent`: the thread's target while it is live, else as `Everyone`
    /// (the stored target is kept, and simply no longer matches a live session).
    Thread,
}

/// The sessions `target` names for a thread of `aid`, writing the thread's
/// target when the send sets or clears it.
fn targets_for(tx: &Transaction<'_>, thread_id: &str, aid: &str, owner: Option<&str>, target: SendTarget<'_>) -> Result<Vec<String>> {
    let live = live_targets(tx, aid, owner)?;
    match target {
        SendTarget::Agent(sid) => {
            if !live.iter().any(|s| s == sid) {
                return Err(CoreError::invalid("unknown_agent", "no live agent on this artifact has that handle"));
            }
            tx.execute("UPDATE threads SET target_session_id = ?2 WHERE id = ?1", params![thread_id, sid])?;
            Ok(vec![sid.to_string()])
        }
        SendTarget::Everyone => {
            tx.execute("UPDATE threads SET target_session_id = NULL WHERE id = ?1", params![thread_id])?;
            Ok(live)
        }
        SendTarget::Thread => {
            let stored: Option<String> =
                tx.query_row("SELECT target_session_id FROM threads WHERE id = ?1", params![thread_id], |r| r.get(0))?;
            Ok(match stored {
                Some(sid) if live.contains(&sid) => vec![sid],
                _ => live,
            })
        }
    }
}
```

  `send_in` becomes `send_in(tx, thread_id, batch_id, target: SendTarget<'_>, touched)` and takes its targets from `targets_for` in place of `live_targets`; the rest (one row per viewer comment without a row, untargeted when the list is empty) is unchanged. Add `pub fn send_to(&self, thread_id: &str, target: SendTarget<'_>) -> Result<(Thread, Touched)>`, and keep `send_to_agent(tid)` as `send_to(tid, SendTarget::Thread)`. Its callers keep it: the thread create route's `@agent` (a new thread has no target, so it fans out) and the comment route's forwarding of later comments (`routes/threads.rs`, where `t.sent_to_agent || mentions_agent(..)` calls it), which now follow the thread's target.
- `store/batches.rs`: `SendBatch` gains `pub to: Option<String>` (a session ID). `send_batch` checks it before any thread, as the first validation after the bounds (`unknown_agent` when it is not among the artifact's live targets), and passes `to.as_deref().map_or(SendTarget::Everyone, SendTarget::Agent)` to every `send_in`. Existing `SendBatch { … }` literals (Task 20's tests) add `to: None`.
- Add a store test in `store/feedback.rs`, beside `send_is_idempotent_and_later_viewer_comments_are_forwarded`: a thread sent with `SendTarget::Agent(watcher)` gets its later viewer comment's row for the watcher only; after the watcher's session ends, the next one goes to the owner; `SendTarget::Everyone` clears the stored target.
- `routes/threads.rs`: `BatchBody` gains `#[serde(default)] to: Option<String>`. A single-send body is `#[derive(Deserialize, Default)] #[serde(deny_unknown_fields)] struct SendBody { #[serde(default)] to: Option<String> }`, read as `resolve` reads its optional `ResolveBody`. In both handlers, inside the store call:

```rust
            let to = match b.to.as_deref() {
                Some(h) if clax_core::is_agent_handle(h) => Some(st.live_agent(&id, h)?.ok_or_else(|| CoreError::invalid("unknown_agent", "no live agent on this artifact has that handle"))?),
                Some(_) => return Err(CoreError::invalid("unknown_agent", "not an agent handle")),
                None => None,
            };
```

  The batch passes `to` in `SendBatch`. The single send calls `st.send_to(&tid, to.as_deref().map_or(SendTarget::Everyone, SendTarget::Agent))`: a single send without `to` is an explicit send to everyone, which clears the target. The capability's `sendToClaude` posts no body, so it fans out, as before this task.
- Thread views gain nothing for the target: `target_session_id` is a session ID, and `tests/api_batch.rs` asserts it is never served.

Run: `cargo test -p clax-server --test api_batch && cargo test -p clax-core`
Expected: PASS.

- [ ] **Step 4: Gates and staging**

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add crates/clax-core/src/store/feedback.rs crates/clax-core/src/store/batches.rs crates/clax-core/src/store/migrations.rs crates/clax-core/src/lib.rs crates/clax-server/src/routes/threads.rs crates/clax-server/src/routes/mod.rs crates/clax-server/src/feedback.rs crates/clax-server/src/error.rs crates/clax-server/tests/api_batch.rs
git add -u crates/clax-server/tests
git status --short   # staged; the controller commits ("Add the batch send route, all or nothing, and an agent target that a thread's later comments follow")
```
### Task 22: One grouped delivery on every harness: goldens, skills, smoke

Every tier renders through `render_items` (Task 20), so there is nothing new to build per tier. This task proves the grouped delivery on each path an agent reads, proves on a real hook that a send naming an agent reaches only that agent and that the thread's later comments follow it (Task 21), and teaches the skills what a batch looks like and that a watch does not mean every comment.

**Files:**
- Modify: `crates/clax-server/tests/api_push.rs`, `crates/clax-hooks/tests/golden.rs`, `crates/clax-mcp/tests/comments.rs`, `plugins/pi/test/clax.test.ts`, `plugins/claude-code/skills/clax/SKILL.md`, `plugins/clax/skills/clax/SKILL.md`, `plugins/pi/skills/clax/SKILL.md`, `scripts/smoke-comment-loop.sh`

- [ ] **Step 1: Codex `codex queue` gets one message for the batch**

Append to `crates/clax-server/tests/api_push.rs`:

```rust
#[tokio::test]
async fn a_batch_reaches_codex_as_one_queued_message_led_by_its_note() {
    let d = tempfile::tempdir().unwrap();
    let ts = server(Some(fake_codex(d.path(), 0, 0)), Duration::from_secs(10)).await;
    let (_sid, aid) = codex_owner(&ts, Some("cx-batch")).await;
    let a = ts.thread(&aid, 1, "one").await["id"].as_str().unwrap().to_string();
    let b = ts.thread(&aid, 1, "two").await["id"].as_str().unwrap().to_string();
    let res = ts.client.post(format!("{}/api/artifacts/{aid}/threads:send", ts.base))
        .json(&json!({"thread_ids": [a, b], "note": "Both, please"})).send().await.unwrap();
    assert_eq!(res.status(), 200);
    let args = ran(d.path()).await;
    assert!(args.starts_with("queue\n--thread\ncx-batch\n--message\n[clax] 2 comments sent to you:\n[clax] 2 comments on \"Pushed\", sent together by Viewer. Note: \"Both, please\"\n"), "{args}");
    assert_eq!(args.matches("[clax] Comment sent to you").count(), 2, "one message holds both");
}
```

`codex_owner` titles its artifact "Pushed". A sender with no cookie is named `Viewer`, as a viewer comment's author is.

- [ ] **Step 2: The Stop hook blocks once with the whole batch**

Append to `crates/clax-hooks/tests/golden.rs`:

```rust
#[test]
fn a_batch_blocks_the_stop_once_with_every_thread() {
    let d = Daemon::start();
    let (_sid, aid) = d.session_with_artifact("claude", "cc-batch");
    let mut tids = Vec::new();
    for body in ["one", "two", "three"] {
        let form = reqwest::blocking::multipart::Form::new()
            .text("anchor", r#"{"kind":"element","selector":"body > h2","quote":"Goals"}"#)
            .text("body", body).text("version", "1");
        let v: Value = d.http().post(format!("{}/api/artifacts/{aid}/threads", d.base())).multipart(form).send().unwrap().json().unwrap();
        tids.push(v["thread"]["id"].as_str().unwrap().to_string());
    }
    let res = d.http().post(format!("{}/api/artifacts/{aid}/threads:send", d.base()))
        .json(&serde_json::json!({"thread_ids": tids, "note": "All three"})).send().unwrap();
    assert_eq!(res.status(), 200);
    let mut stop_in: Value = serde_json::from_slice(&fixture("claude-stop.json")).unwrap();
    stop_in["session_id"] = "cc-batch".into();
    let r = hook(&d.home(), "claude", "stop", stop_in.to_string().as_bytes());
    let reason = one_line_json(&r.stdout)["reason"].as_str().unwrap().to_string();
    assert!(reason.starts_with("[clax] 3 comments sent to you:\n[clax] 3 comments on \"Hooked\", sent together by Viewer. Note: \"All three\"\n"), "{reason}");
    for t in &tids {
        assert!(reason.contains(t.as_str()), "{reason}");
    }
    stop_in["stop_hook_active"] = true.into();
    assert_eq!(hook(&d.home(), "claude", "stop", stop_in.to_string().as_bytes()).stdout, "");
}

#[test]
fn a_send_to_one_agent_blocks_only_its_stop_and_later_comments_follow_it() {
    let d = Daemon::start();
    let (_owner, aid) = d.session_with_artifact("claude", "cc-owner");
    let w: Value = d.http().post(format!("{}/api/sessions", d.base())).bearer_auth(d.token())
        .json(&serde_json::json!({"harness": "claude", "harness_session_id": "cc-watcher", "cwd": "/tmp/project"}))
        .send().unwrap().json().unwrap();
    let watcher = w["session"]["id"].as_str().unwrap().to_string();
    let res = d.http().put(format!("{}/api/sessions/{watcher}/watches/{aid}", d.base())).bearer_auth(d.token()).send().unwrap();
    assert!(res.status().is_success());
    let a: Value = d.http().get(format!("{}/api/artifacts/{aid}", d.base())).send().unwrap().json().unwrap();
    // The watcher watched last, so it is the most recently active live agent and leads the list (Task 15).
    let first = &a["artifact"]["participants"]["agents"][0];
    assert_eq!(first["live"], true);
    let handle = first["handle"].as_str().unwrap().to_string();
    let form = reqwest::blocking::multipart::Form::new()
        .text("anchor", r#"{"kind":"element","selector":"body > h2","quote":"Goals"}"#)
        .text("body", "two columns").text("version", "1");
    let t: Value = d.http().post(format!("{}/api/artifacts/{aid}/threads", d.base())).multipart(form).send().unwrap().json().unwrap();
    let tid = t["thread"]["id"].as_str().unwrap().to_string();
    let res = d.http().post(format!("{}/api/artifacts/{aid}/threads/{tid}/send", d.base()))
        .json(&serde_json::json!({"to": handle})).send().unwrap();
    assert_eq!(res.status(), 200);
    let stop = |hsid: &str| {
        let mut s: Value = serde_json::from_slice(&fixture("claude-stop.json")).unwrap();
        s["session_id"] = hsid.into();
        hook(&d.home(), "claude", "stop", s.to_string().as_bytes()).stdout
    };
    assert_eq!(stop("cc-owner"), "", "the owner is not the agent the thread was sent to");
    assert!(one_line_json(&stop("cc-watcher"))["reason"].as_str().unwrap().contains("two columns"));
    let res = d.http().post(format!("{}/api/artifacts/{aid}/threads/{tid}/comments", d.base()))
        .json(&serde_json::json!({"body": "and the footer"})).send().unwrap();
    assert_eq!(res.status(), 201);
    assert_eq!(stop("cc-owner"), "", "a later comment follows the thread's target");
    assert!(one_line_json(&stop("cc-watcher"))["reason"].as_str().unwrap().contains("and the footer"));
}
```

Both sessions are Claude Code sessions, so the test tells them apart by order: the watcher's watch is its newest activity, which puts it first in `participants.agents`.

- [ ] **Step 3: MCP tier 1 and Pi tier 5**

Append to `crates/clax-mcp/tests/comments.rs`:

```rust
#[tokio::test]
async fn a_batch_piggybacks_as_one_group_on_the_next_tool_result() {
    let ts = TestServer::spawn().await;
    let (tools, _sid) = session_tools(&ts).await;
    let (p, _) = blocks(&tools.publish(Parameters(PublishArgs { html: Some("<h2>Goals</h2>".into()), title: Some("Batch".into()), ..Default::default() })).await.unwrap());
    let aid = p["artifact_id"].as_str().unwrap().to_string();
    let a = ts.thread(&aid, 1, "one").await["id"].as_str().unwrap().to_string();
    let b = ts.thread(&aid, 1, "two").await["id"].as_str().unwrap().to_string();
    ts.client.post(format!("{}/api/artifacts/{aid}/threads:send", ts.base)).json(&json!({"thread_ids": [a, b]})).send().await.unwrap();
    let (v, trailing) = blocks(&tools.list(Parameters(ListArgs::default())).await.unwrap());
    assert_eq!(v["feedback"].as_array().unwrap().len(), 2);
    assert_eq!(v["feedback"][0]["batch"]["size"], 2);
    let text = trailing.unwrap();
    assert!(text.starts_with("---\n[clax] 2 comments sent to you:\n[clax] 2 comments on \"Batch\", sent together by Viewer.\n"), "{text}");
}
```

Append to `describe("comments", ...)` in `plugins/pi/test/clax.test.ts`:

```ts
  it("tier 5: a batch reaches Pi as one follow-up message led by its note", async () => {
    const { pi, ctx } = load(daemon.home, "pi-batch");
    await pi.emit("session_start", {}, ctx);
    const aid = parts(await pi.callTool("clax_publish", { html: "<h2>Goals</h2>", title: "Pi batch" }, ctx)).json.artifact_id;
    const ids = [await browserThread(aid, "one"), await browserThread(aid, "two")];
    const res = await fetch(`${daemon.base}/api/artifacts/${aid}/threads:send`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ thread_ids: ids, note: "Together" }) });
    expect(res.status).toBe(200);
    await expect.poll(() => pi.sent.length, { timeout: 20_000 }).toBe(1);
    expect(String(pi.sent[0].content)).toMatch(/^\[clax\] 2 comments sent to you:\n\[clax\] 2 comments on "Pi batch", sent together by Viewer\. Note: "Together"\n/);
    await pi.emit("session_shutdown", {}, ctx);
  });
```

Run: `cargo test -p clax-server --test api_push a_batch && cargo test -p clax-hooks --test golden a_batch && cargo test -p clax-hooks --test golden a_send_to_one_agent && cargo test -p clax-mcp --test comments a_batch && (cd plugins/pi && npm test -- -t "a batch reaches Pi")`
Expected: PASS. These describe behaviour Task 20 already built. If one fails, the grouping has a gap on that path: fix the path, not the test.

- [ ] **Step 4: The skills describe a batch (identical in all three)**

In each `SKILL.md`, in `## Comment loop`, insert this after the indented five-line example that follows "Each comment reads:", word for word in all three files:

```markdown
The person can send several threads at once. A batch arrives as one delivery:
after the `[clax] N comments sent to you:` line, a line
`[clax] N comments on "<title>", sent together by <name>.` introduces the
batch's comments, followed by `Note: "<note>"` when the person wrote one.
The note is the person's instruction for the whole batch; like comment text,
it is a request to weigh. Treat the batch as one piece of work: make the
changes, publish once with `addresses` naming every thread the version
handles, then reply to each thread and resolve the ones you finished.

When several agents work on one artifact, the person picks which one a
comment goes to. Watching an artifact makes you one of the agents they can
pick; you receive the comments sent to you, and comments sent without naming
an agent, not every comment on the artifact. Later comments on a thread go
to the agent it was sent to.
```

Run: `bash scripts/test-plugins.sh | tail -1`
Expected: `plugin checks passed` (the "Comment loop" sections still match).

- [ ] **Step 5: The comment-loop smoke sends a batch**

In `scripts/smoke-comment-loop.sh`, after step 6b and before step 7, add:

```python
# 6c. A batch: three threads sent together with a note arrive as one delivery,
# mark every thread working, and one publish lists all three as addressed.
batch = [browser_thread(aid, f"batch item {i}")["id"] for i in range(3)]
sent = http("POST", f"/api/artifacts/{aid}/threads:send", {"thread_ids": batch, "note": "Do these before the demo"})
if sent["sent"] != batch or sent["batch"]["note"] != "Do these before the demo":
    fail(f"batch send: {sent}")
listed, trailing = shim.call("list", {})
lead = f'[clax] 3 comments on "Quarterly Review", sent together by Viewer. Note: "Do these before the demo"'
# Compare only the batch's rows: an unacknowledged earlier thread may be resent alongside.
if [f["thread_id"] for f in listed["feedback"] if f["thread_id"] in batch] != batch or not trailing or lead not in trailing.split("\n"):
    fail("batch delivery:\n" + str(trailing))
ok(f"batch: three threads arrived in one delivery led by the note ({lead})")
w = working(aid)
if len(w) != 1 or w[0]["thread_ids"] != batch:
    fail(f"working after the batch: {w}")
ok("batch: every thread in it is marked working")
v3, _ = shim.call("publish", {"id": aid, "html": page, "note": "Batch done"})
if v3["addressed"] != batch:
    fail(f"publish after the batch: {v3}")
ok(f"batch: publish v{v3['version']} listed all three threads as addressed")
```

`trailing.split("\n")` holds `---`, the counted header, then the lead line.

Run: `scripts/smoke-comment-loop.sh`
Expected: all `PASS`, including the three `batch:` lines, ending with `comment loop smoke passed`.

- [ ] **Step 6: Gates and staging**

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add crates/clax-server/tests/api_push.rs crates/clax-hooks/tests/golden.rs crates/clax-mcp/tests/comments.rs plugins/pi/test/clax.test.ts \
  plugins/claude-code/skills/clax/SKILL.md plugins/clax/skills/clax/SKILL.md plugins/pi/skills/clax/SKILL.md scripts/smoke-comment-loop.sh
git status --short   # staged; the controller commits ("Prove a batch arrives as one delivery on every harness, and teach the skills to read one")
```

---

### Task 23: Batch send in Echo: checkboxes, the selection bar, the agent picker, and Send N unsent

The viewer's side of batch send, in Echo:
- checkboxes on open cards, with Shift-click ranges, and X ticks the selected thread;
- a selection bar at the top of the sidebar (under the working strip, above the groups), loaded when the first box is ticked: converging dots, `3 selected` over `sent together`, Clear, `Send 3 to claude ▾`, and an optional note sent with Cmd+Enter or Ctrl+Enter or Shift+S;
- `Send N unsent to claude` at the sidebar's top;
- one agent picker shared by every Send.

Send goes to the agent you last sent to on this artifact if it is live, else to the first live agent of `participants.agents` (Task 15 orders it live first, most recently active first). With no live agent, Send goes without `to`: the daemon stores the comments untargeted for the next session that publishes or watches. The caret appears only when more than one agent is live.

**Files:**
- Create: `web/shell/src/view/batch-model.ts`, `web/shell/src/view/batch-model.test.ts`, `web/shell/src/view/send-target.ts`, `web/shell/src/view/send-target.test.ts`, `web/shell/src/ui/SelectionBar.svelte`, `web/shell/src/ui/SendButton.svelte`, `web/e2e/batch.spec.ts`
- Modify: `web/shell/src/threads.ts` (`sendBatch`, `sendToAgent(…, to)`, `Thread.sends`), `web/shell/src/view/history-model.ts`, `web/shell/src/view/keys.ts`, `web/shell/src/view/keys.test.ts`, `web/shell/src/view/artifact-controller.ts`, `web/shell/src/view/artifact-controller.test.ts`, `web/shell/src/ui/SidebarIsland.svelte`, `web/shell/src/ui/Sidebar.svelte`, `web/shell/src/ui/ThreadCard.svelte`, `web/shell/src/sidebar.test.ts`, `web/shell/src/theme.css`, `web/e2e/scenes.ts`

**Interfaces:**
- `threads.ts`:
  - `sendToAgent(aid, tid, to: string | null = null)` posts `{to}` when set;
  - `sendBatch(aid, threadIds, note, to: string | null): Promise<{ threads: Thread[]; sent: string[]; unchanged: string[] }>` (the body has no `to` key when `to` is null) calls `POST /api/artifacts/<aid>/threads:send` with no token, and throws `ApiError` with the daemon's message;
  - `Thread` gains `sends?: { batch_id: string; size: number; note: string | null; sent_by: string; sent_at: string }[]`.
- `view/batch-model.ts`:
  - `type Selection = { ids: string[]; anchor: string | null }` and `EMPTY_SELECTION`;
  - `selectable(t, deleted)`;
  - `toggle(sel, id, shift, order: string[])`;
  - `prune(sel, threads, deleted)`;
  - `unsent(threads)`;
  - `countLabel(n)`, `unsentLabel(n, agent)` and `sendLabel(n, agent)`.
- `view/send-target.ts`:
  - `liveAgents(agents: AgentView[]): AgentView[]`;
  - `defaultTarget(aid: string, agents: AgentView[]): string | null`, which picks the remembered handle if it is live, else the first live agent in `agents` (the daemon orders them live first, most recently active first), else null. Null means the shell sends without `to`;
  - `rememberTarget(aid, handle)`, kept in `localStorage` `clax.sendTo.<aid>` with every access guarded.
- `history-model.ts`: each send in `t.sends` adds `{ v: versionAt(sent_at), who: sent_by, agent: false, verb: "sent it" + (size > 1 ? ` with ${size - 1} other${size - 1 === 1 ? "" : "s"}` : "") + (note ? ` · “${note}”` : "") }`, so a batch of three reads "sent it with 2 others".
- `keys.ts`: `x` maps to `tick`. `S` (Shift+S) has mapped to `sendTicked` since Task 3. Rows `{ keys: ["X"], what: "Tick the selected thread", action: "tick" }` and `{ keys: ["⇧", "S"], what: "Send every ticked thread together", action: "sendTicked" }` go after `R`.
- `ArtifactController`:
  - `ViewState` gains `selection: Selection`, `batchNote: string`, `batchBusy: boolean` and `sendTo: string | null`;
  - new methods: `toggleSelect(t, shift)`, `clearSelection()`, `setBatchNote(v)`, `sendSelection()`, `sendUnsent()`, `chooseTarget(handle)`;
  - `sendThread(t)` passes `this.s.sendTo`;
  - `shortcut("tick")` toggles the selected thread, and `shortcut("sendTicked")` calls `sendSelection()`.
- Components:
  - `SendButton` `{ label: string; agents: AgentView[]; names: Map<string, string>; target: string | null; disabled?: boolean; onSend(): void; onChoose(handle: string): void }`. Its caret is a menu button, and it is shown only with more than one live agent.
  - `SelectionBar` (lazy) `{ count; note; busy; send: Snippet; onNote; onClear; onSend }`.
  - `ThreadCard` gains `checked?: boolean`, `onToggle?(t, shift)` and `send?: Snippet` (it renders the shared Send button).

- [ ] **Step 1: Models, tests first**

`web/shell/src/view/batch-model.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import type { Thread } from "../threads";
import { EMPTY_SELECTION, countLabel, prune, selectable, sendLabel, toggle, unsent, unsentLabel } from "./batch-model";

const T = (id: string, status: "open" | "resolved" = "open", sent = false) => ({ id, status, sent_to_agent: sent } as Thread);
const order = ["a", "b", "c", "d", "e"];

describe("batch-model", () => {
  it("selects open threads of a live artifact only", () => {
    expect([selectable(T("a"), false), selectable(T("a", "resolved"), false), selectable(T("a"), true)]).toEqual([true, false, false]);
  });
  it("toggles one, then a shift range to the clicked box's new state, in sidebar order", () => {
    let s = toggle(EMPTY_SELECTION, "b", false, order);
    expect(s).toEqual({ ids: ["b"], anchor: "b" });
    s = toggle(s, "d", true, order);
    expect(s.ids).toEqual(["b", "c", "d"]);
    s = toggle(s, "c", false, order);
    expect(s).toEqual({ ids: ["b", "d"], anchor: "c" });
  });
  it("drops threads that disappeared, were resolved, or whose artifact went", () => {
    const s = { ids: ["a", "b", "c"], anchor: "c" };
    expect(prune(s, [T("a"), T("b", "resolved")], false)).toEqual({ ids: ["a"], anchor: null });
    expect(prune(s, [T("a"), T("b"), T("c")], true)).toEqual(EMPTY_SELECTION);
    expect(prune(s, [T("a"), T("b"), T("c")], false)).toBe(s);
  });
  it("counts unsent threads and labels in Echo's words", () => {
    expect(unsent([T("a"), T("b", "open", true), T("c", "resolved")]).map(t => t.id)).toEqual(["a"]);
    expect([countLabel(3), unsentLabel(4, "claude"), sendLabel(3, "claude"), sendLabel(1, "codex")]).toEqual(["3 selected", "Send 4 unsent to claude", "Send 3 to claude", "Send to codex"]);
  });
});
```

`web/shell/src/view/send-target.test.ts`:

```ts
import { afterEach, describe, expect, it } from "vitest";
import { defaultTarget, liveAgents, rememberTarget } from "./send-target";

// As the daemon lists them: live first, most recently active first.
const agents = [{ handle: "a_cx", harness: "codex", live: true }, { handle: "a_cl", harness: "claude", live: true }, { handle: "a_old", harness: "pi", live: false }];
afterEach(() => localStorage.clear());

describe("send-target", () => {
  it("prefers the agent you last sent to while it is live, then the most recently active live agent", () => {
    expect(defaultTarget("x", agents)).toBe("a_cx");
    rememberTarget("x", "a_cl");
    expect(defaultTarget("x", agents)).toBe("a_cl");
    rememberTarget("x", "a_old");
    expect(defaultTarget("x", agents)).toBe("a_cx"); // the agent you last sent to has ended
    expect(liveAgents(agents).map(a => a.handle)).toEqual(["a_cx", "a_cl"]);
  });
  it("names no target when no agent is live, so the send goes without to", () => {
    rememberTarget("x", "a_old");
    expect(defaultTarget("x", [{ handle: "a_old", harness: "pi", live: false }])).toBeNull();
    expect(defaultTarget("x", [])).toBeNull();
  });
});
```

Run: `cd web && npx vitest run shell/src/view/batch-model.test.ts shell/src/view/send-target.test.ts`
Expected: FAIL.

`web/shell/src/view/batch-model.ts`:

```ts
// Batch send in the sidebar (spec §8): which cards can be ticked, range
// selection, pruning, and the words shown. Framework-free.
import type { Thread } from "../threads";

export type Selection = { ids: string[]; anchor: string | null };
export const EMPTY_SELECTION: Selection = { ids: [], anchor: null };
export const selectable = (t: Thread, deleted: boolean) => !deleted && t.status === "open";

/** Ticks or unticks `id`. With `shift` and an anchor, every ID in `order`
 * between the anchor and `id` takes `id`'s new state. `id` becomes the anchor. */
export function toggle(sel: Selection, id: string, shift: boolean, order: string[]): Selection {
  const on = !sel.ids.includes(id);
  const a = sel.anchor === null ? -1 : order.indexOf(sel.anchor);
  const b = order.indexOf(id);
  const range = shift && a >= 0 && b >= 0 ? order.slice(Math.min(a, b), Math.max(a, b) + 1) : [id];
  const rest = sel.ids.filter(x => !range.includes(x));
  return { ids: on ? [...rest, ...range] : rest, anchor: id };
}

/** Keeps only IDs of threads still selectable; the same object when nothing changed. */
export function prune(sel: Selection, threads: Thread[], deleted: boolean): Selection {
  const live = new Set(threads.filter(t => selectable(t, deleted)).map(t => t.id));
  const ids = sel.ids.filter(id => live.has(id));
  const anchor = sel.anchor !== null && live.has(sel.anchor) && ids.includes(sel.anchor) ? sel.anchor : null;
  return ids.length === sel.ids.length && anchor === sel.anchor ? sel : ids.length ? { ids, anchor } : EMPTY_SELECTION;
}

export const unsent = (threads: Thread[]) => threads.filter(t => t.status === "open" && !t.sent_to_agent);
export const countLabel = (n: number) => `${n} selected`;
export const unsentLabel = (n: number, agent: string) => `Send ${n} unsent to ${agent}`;
export const sendLabel = (n: number, agent: string) => (n > 1 ? `Send ${n} to ${agent}` : `Send to ${agent}`);
```

`web/shell/src/view/send-target.ts`:

```ts
// Which agent a Send goes to (spec §10, "Participants and attention"): the
// one this viewer last sent to on the artifact while it is live, else the
// most recently active live agent (the daemon's list order), else none, and
// the send goes without `to`. Remembered per browser; storage may throw.
import type { AgentView } from "./working-model";

const key = (aid: string) => `clax.sendTo.${aid}`;
export const liveAgents = (agents: AgentView[]) => agents.filter(a => a.live);

export function rememberTarget(aid: string, handle: string): void {
  try { localStorage.setItem(key(aid), handle); } catch { /* this page only */ }
}

export function defaultTarget(aid: string, agents: AgentView[]): string | null {
  const live = liveAgents(agents);
  let last: string | null = null;
  try { last = localStorage.getItem(key(aid)); } catch { /* none */ }
  if (last && live.some(a => a.handle === last)) return last;
  return live[0]?.handle ?? null;
}
```

Run: `cd web && npx vitest run shell/src/view/batch-model.test.ts shell/src/view/send-target.test.ts`
Expected: PASS.

- [ ] **Step 2: The controller, test first**

Use Task 3's seeded `started()` in `view/artifact-controller.test.ts`, with this seed, declared once above both tests as `const seed: Seed = { … }`:
- `threads: [thread("t1"), thread("t2")]`;
- `artifact: { participants: { people: [], agents: [{ handle: "a_cl", harness: "claude", live: true }] } }`;
- `routes`: a `POST` to a URL ending `/threads:send` answers `{ threads: [thread("t1", { sent_to_agent: true }), thread("t2", { sent_to_agent: true })], sent: ["t1", "t2"], unchanged: [] }`.

Then add the two tests below, and a third with `participants.agents` empty: `sendTo` is `null`, and `sendSelection()` posts `{ thread_ids: ["t1", "t2"], note: null }` with no `to` key.

```ts
  it("ticks a range, sends it as one batch to the default agent with the note, then clears", async () => {
    const { ctl } = await started(seed);
    await vi.waitFor(() => expect(ctl.state.get().threads.length).toBe(2));
    const [t1, t2] = ctl.state.get().threads;
    expect(ctl.state.get().sendTo).toBe("a_cl");
    ctl.toggleSelect(t1, false);
    ctl.toggleSelect(t2, true);
    ctl.setBatchNote("Before the demo");
    await ctl.sendSelection();
    const call = (fetch as unknown as ReturnType<typeof vi.fn>).mock.calls.find(([u]) => String(u).endsWith("/threads:send"))!;
    expect(JSON.parse((call[1] as RequestInit).body as string)).toEqual({ thread_ids: ["t1", "t2"], note: "Before the demo", to: "a_cl" });
    expect(ctl.state.get()).toMatchObject({ selection: { ids: [], anchor: null }, batchNote: "", batchBusy: false });
    ctl.dispose();
  });

  it("drops a thread from the selection when it disappears", async () => {
    const { ctl } = await started(seed);
    await vi.waitFor(() => expect(ctl.state.get().threads.length).toBe(2));
    const [t1, t2] = ctl.state.get().threads;
    ctl.toggleSelect(t1, false);
    ctl.toggleSelect(t2, false);
    FakeES.last!.emit("thread_deleted", { type: "thread_deleted", artifact_id: ID, thread_id: "t1" });
    await Promise.resolve();
    expect(ctl.state.get().selection.ids).toEqual(["t2"]);
    ctl.dispose();
  });
```

Implement in `view/artifact-controller.ts`:
- Fields: `selection: EMPTY_SELECTION`, `batchNote: ""`, `batchBusy: false` and `sendTo: null`.
- When `data` is first set, set `sendTo: defaultTarget(this.id, d.artifact.participants?.agents ?? [])`. Whenever `participants` change afterwards (a refetch after `working` or `version`), recompute it the same way unless `sendTo` still names a live agent, so a target whose session ended is replaced, and is replaced by none when no agent is live.
- `chooseTarget(h)`: `rememberTarget(this.id, h); this.set({ sendTo: h });`.
- `toggleSelect(t, shift)`: `this.set(s => ({ selection: toggle(s.selection, t.id, shift, this.order(s).map(x => x.id)) }))`.
- `clearSelection()` and `setBatchNote(v)`, as their names say.
- `private async sendIds(ids)`:
  - returns when `ids` is empty or a send is busy;
  - otherwise sets `batchBusy`, and calls `sendBatch(this.id, ids, note || null, this.s.sendTo)` inside `report(…, SEND_FAILED, this.noticeFor(SEND_FAILED))`;
  - on success, upserts the returned threads, clears the selection and note, and calls `rememberTarget` for `sendTo` when it is set;
  - when `sendTo` is null the request carries no `to` (`sendBatch` and `sendToAgent` drop a null `to`), and the daemon stores the comments untargeted;
  - always ends with `batchBusy: false`.
- `sendSelection()` sends `this.s.selection.ids`. `sendUnsent()` sends `unsent(this.s.threads)`.
- `sendThread(t)` becomes `this.saveThread(sendToAgent(this.id, t.id, this.s.sendTo), SEND_FAILED)`, and remembers the target when there is one.
- An `unknown_agent` answer (the target ended between the last refetch and the send) refetches the artifact, recomputes `sendTo`, and shows the send failure; it never retries without `to` on its own.
- In `react()`, when `prev.threads !== s.threads || prev.deleted !== s.deleted`, prune the selection, and set it only if it changed.
- `shortcut("tick")`: `if (sel && selectable(sel, s.deleted)) this.toggleSelect(sel, false)`. `shortcut("sendTicked")`: `void this.sendSelection()`.

Run: `cd web && npx vitest run shell/src/view/artifact-controller.test.ts`
Expected: PASS.

- [ ] **Step 3: Components**

`web/shell/src/ui/SendButton.svelte`:

```svelte
<script lang="ts">
  // A Send that names its agent (spec §10). The caret, shown only when more
  // than one agent is live, picks another; the choice is remembered.
  import type { AgentView } from "../view/working-model";

  let { label, agents, names, target, disabled = false, onSend, onChoose }: {
    label: string; agents: AgentView[]; names: Map<string, string>; target: string | null; disabled?: boolean; onSend(): void; onChoose(h: string): void;
  } = $props();
  let open = $state(false);
  const live = $derived(agents.filter(a => a.live));
</script>

<span class="send">
  <button type="button" class="primary" {disabled} onclick={onSend}>{label}</button>
  {#if live.length > 1}
    <button type="button" class="primary caret" aria-label="Choose the agent" aria-haspopup="menu" aria-expanded={open} onclick={() => { open = !open; }}>▾</button>
    {#if open}
      <!-- Escape closes the menu. -->
      <!-- svelte-ignore a11y_interactive_supports_focus -->
      <div class="send-menu" role="menu" onkeydown={e => { if (e.key === "Escape") { e.stopPropagation(); open = false; } }}>
        {#each live as a (a.handle)}
          <button type="button" role="menuitemradio" aria-checked={a.handle === target} class="ghost" onclick={() => { onChoose(a.handle); open = false; }}>{names.get(a.handle) ?? a.harness}</button>
        {/each}
      </div>
    {/if}
  {/if}
</span>
```

`web/shell/src/ui/SelectionBar.svelte`:

```svelte
<script lang="ts">
  // The selection bar (spec §8): the ticked threads, sent together to one agent.
  import type { Snippet } from "svelte";
  import { countLabel } from "../view/batch-model";
  import { isSubmitKey } from "../view/composer-model";

  let { count, note, busy, send, onNote, onClear, onSend }: { count: number; note: string; busy: boolean; send: Snippet; onNote(v: string): void; onClear(): void; onSend(): void } = $props();
</script>

<div class="selbar" role="region" aria-label="Selected comments">
  <div class="selbar-row">
    <span class="converge" aria-hidden="true"><i></i><i></i><i></i><i></i></span>
    <span class="txt"><b role="status">{countLabel(count)}</b>sent together</span>
    <button type="button" class="ghost" onclick={onClear}>Clear</button>
    {@render send()}
  </div>
  <input class="selbar-note" aria-label="Note for the agent (optional)" placeholder="Note for the agent (optional)" maxlength="280" value={note} disabled={busy}
    oninput={e => onNote(e.currentTarget.value)} onkeydown={e => { if (isSubmitKey(e)) { e.preventDefault(); onSend(); } }} />
</div>
```

`isSubmitKey` is the function the port keeps in `view/composer-model.ts`.

`ui/ThreadCard.svelte`:
- Add `checked?: boolean`, `onToggle?(t: Thread, shift: boolean): void` and `send?: Snippet`.
- As the header's first child, when `onToggle` is given: `<input type="checkbox" class="thread-check" checked={checked ?? false} aria-label={`Select thread ${n ?? ""} ${anchorLabel(t.anchor)}`.replace("  ", " ")} onclick={e => { e.stopPropagation(); onToggle(t, e.shiftKey); }} />`. The click handler reads `shiftKey`, so Shift-click makes a range.
- In the actions, replace the Send button with `{#if !t.sent_to_agent}{@render send?.()}{/if}`.

`ui/Sidebar.svelte`:
- Add `selection`, `batchNote`, `batchBusy`, `sendTo`, `onToggle`, `onClear`, `onNote`, `onSendSelection`, `onSendUnsent` and `onChoose` to `Props`.
- Every open card gets `checked`, `onToggle` and a `send` snippet rendering `SendButton` with `label={`Send to ${names.get(p.sendTo ?? "") ?? p.agent}`}` and `onSend={() => p.onSend(t)}`.
- In the script, name the target once: `const target = $derived(names.get(p.sendTo ?? "") ?? p.agent);` (the same expression the card's Send uses), and pass it to every label below.
- At the top of the aside, after the strip: `{#if unsent(p.threads).length}<button class="send-unsent" onclick={p.onSendUnsent}>{unsentLabel(unsent(p.threads).length, target)}</button>{/if}`.
- Directly after it, still above the groups, when `p.selection.ids.length`, the lazy bar:

```svelte
    {#await import("./SelectionBar.svelte") then { default: SelectionBar }}
      <SelectionBar count={p.selection.ids.length} note={p.batchNote} busy={p.batchBusy} onNote={p.onNote} onClear={p.onClear} onSend={p.onSendSelection}>
        {#snippet send()}<SendButton label={sendLabel(p.selection.ids.length, target)} agents={p.agents ?? []} {names} target={p.sendTo} disabled={p.batchBusy} onSend={p.onSendSelection} onChoose={p.onChoose} />{/snippet}
      </SelectionBar>
    {/await}
```

  The bar sits at the top, where the viewer's eyes are when they start ticking, and it stays in view while they scroll the list (`position: sticky; top: 0`).

`ui/SidebarIsland.svelte` wires all of these to the controller.

`history-model.ts` adds the send events. `keys.ts` adds `x` and the two rows. Update the row list in `keys.test.ts`.

In `sidebar.test.ts`, add tests that:
- a ticked card's checkbox reads `Select thread 1 …`;
- Shift-click passes `shift: true`;
- with two live agents the caret menu lists both and checks the target;
- with one live agent there is no caret.

- [ ] **Step 4: Styles**

Append to `web/shell/src/theme.css`:

```css
/* Batch send (spec §8). */
.thread-check { width: 18px; height: 18px; margin: 0; accent-color: var(--accent); flex: none; }
.thread-card:has(.thread-check:checked) { box-shadow: inset 0 0 0 1px var(--accent); }
.send { display: inline-flex; position: relative; }
.send .caret { min-width: 28px; padding: 0 6px; border-left: 1px solid color-mix(in srgb, var(--on-accent) 35%, transparent); font-family: var(--mono); }
.send-menu { position: absolute; right: 0; top: calc(100% + 6px); z-index: 20; min-width: 160px; background: var(--raised); border: 1px solid var(--border-strong); box-shadow: 0 14px 40px var(--shadow); display: flex; flex-direction: column; padding: 4px 0; }
.send-menu button { justify-content: flex-start; border: 0; min-height: 32px; }
.send-menu button[aria-checked="true"]::before { content: "✓"; margin-right: 6px; }
.send-unsent { width: 100%; }
.selbar { position: sticky; top: 0; z-index: 5; margin: 0 -14px 8px; display: flex; flex-direction: column; gap: 8px; padding: 12px 14px; background: var(--raised); border-bottom: 1px solid var(--border-strong); box-shadow: 0 8px 24px var(--shadow); }
.selbar-row { display: flex; align-items: center; gap: 12px; }
.selbar .txt { flex: 1; font-size: 12px; color: var(--muted); line-height: 1.35; }
.selbar .txt b { display: block; color: var(--fg); font: 600 16px/1.1 var(--grot); }
.converge { position: relative; width: 46px; height: 24px; flex: none; }
.converge i { position: absolute; top: 5px; width: 14px; height: 14px; border-radius: 50%; background: var(--you); border: 1.5px solid var(--raised); transition: left .4s cubic-bezier(.4,0,.2,1); }
.converge i:nth-child(1) { left: 0; } .converge i:nth-child(2) { left: 8px; } .converge i:nth-child(3) { left: 16px; }
.converge i:nth-child(4) { left: 28px; top: 2px; width: 20px; height: 20px; background: var(--agent); }
@media (max-width: 700px) { .selbar-row { flex-wrap: wrap; } .selbar button { min-height: 40px; } }
@media (prefers-reduced-motion: reduce) { .converge i { transition: none; } }
```

- [ ] **Step 5: Browser tests**

`web/e2e/batch.spec.ts`:

```ts
import { test, expect } from "@playwright/test";
import { api, openArtifact, postThread, publishAs, registerSession, startDaemon } from "./fixtures";

let d: Awaited<ReturnType<typeof startDaemon>>;
test.beforeAll(async () => { test.setTimeout(180_000); d = await startDaemon(); });
test.afterAll(async () => { await d?.stop(); });

async function fresh(title: string, n: number) {
  const s = await registerSession(d.base, d.token, "claude", `batch-${title}`);
  const { artifact } = await publishAs(d.base, d.token, s.id, title, { "index.html": "<main><h2>Quarterly goals</h2></main>" });
  const ids: string[] = [];
  for (let i = 0; i < n; i++) ids.push((await postThread(d.base, artifact.id, `item ${i}`)).id);
  return { sid: s.id, aid: artifact.id, ids };
}
const panel = async (page: import("@playwright/test").Page) => {
  if (!(await page.locator("aside.sidebar").isVisible())) await page.getByRole("button", { name: /Threads/ }).first().click();
};

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: tick a shift range, send with a note, and the agent gets one delivery`, async ({ page }) => {
    const { sid, aid, ids } = await fresh(`Batch ${mode}`, 4);
    await openArtifact(page, d.base, aid, 1, mode);
    await panel(page);
    const box = (id: string) => page.locator(`.thread-card[data-thread="${id}"] .thread-check`);
    await box(ids[0]).click();
    await box(ids[2]).click({ modifiers: ["Shift"] });
    const bar = page.getByRole("region", { name: "Selected comments" });
    await expect(bar.getByRole("status")).toHaveText("3 selected");
    await expect(bar).toContainText("sent together");
    const [barTop, firstCardTop] = await Promise.all([bar.boundingBox(), page.locator(".thread-card").first().boundingBox()]);
    expect(barTop!.y, "the selection bar sits at the top of the sidebar").toBeLessThan(firstCardTop!.y);
    await expect(bar.getByRole("button", { name: "Send 3 to claude" })).toBeVisible();
    await expect(bar.getByRole("button", { name: "Choose the agent" })).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Send 4 unsent to claude" })).toBeVisible();
    await bar.getByLabel("Note for the agent (optional)").fill("Before the demo");
    await bar.getByLabel("Note for the agent (optional)").press("ControlOrMeta+Enter");
    await expect(bar).toHaveCount(0);
    const got = await api(d.base, d.token, `/api/sessions/${sid}/feedback?tier=piggyback`);
    expect(got.feedback.map((f: { thread_id: string }) => f.thread_id)).toEqual(ids.slice(0, 3));
    expect(got.text.split("\n")[1]).toBe(`[clax] 3 comments on "Batch ${mode}", sent together by Viewer. Note: "Before the demo"`);
    await expect(page.locator(`.thread-card[data-thread="${ids[1]}"] .hist`)).toContainText("sent it with 2 others · “Before the demo”");
    await page.getByRole("button", { name: "Send 1 unsent to claude" }).click();
    await expect(page.getByRole("button", { name: /unsent to/ })).toHaveCount(0);
  });

  test(`${mode}: with two live agents the caret picks one, and only that agent gets the rows`, async ({ page }) => {
    const { sid, aid, ids } = await fresh(`Pick ${mode}`, 1);
    const other = await registerSession(d.base, d.token, "codex", `pick-${mode}`);
    await api(d.base, d.token, `/api/sessions/${other.id}/watches/${aid}`, { method: "PUT" });
    await openArtifact(page, d.base, aid, 1, mode);
    await panel(page);
    const card = page.locator(`.thread-card[data-thread="${ids[0]}"]`);
    await card.getByRole("button", { name: "Choose the agent" }).click();
    await card.getByRole("menuitemradio", { name: "codex" }).click();
    await card.getByRole("button", { name: "Send to codex" }).click();
    expect((await api(d.base, d.token, `/api/sessions/${other.id}/feedback?tier=piggyback`)).feedback).toHaveLength(1);
    expect((await api(d.base, d.token, `/api/sessions/${sid}/feedback?tier=piggyback`)).feedback).toHaveLength(0);
    await page.reload();
    await panel(page);
    expect(await page.evaluate(id => localStorage.getItem(`clax.sendTo.${id}`), aid)).toMatch(/^a_/);
  });

  test(`${mode}: with no live agent Send goes without to, and the comment waits for the next session`, async ({ page }) => {
    const { sid, aid, ids } = await fresh(`Nobody ${mode}`, 1);
    await api(d.base, d.token, `/api/sessions/${sid}`, { method: "PATCH", body: JSON.stringify({ ended: true }) });
    await openArtifact(page, d.base, aid, 1, mode);
    await panel(page);
    const card = page.locator(`.thread-card[data-thread="${ids[0]}"]`);
    await expect(card.getByRole("button", { name: "Choose the agent" })).toHaveCount(0);
    const sent = page.waitForRequest(r => r.url().endsWith(`/threads/${ids[0]}/send`));
    await card.getByRole("button", { name: /^Send to / }).click();
    expect((await sent).postData() ?? "").not.toContain("\"to\"");
    const next = await registerSession(d.base, d.token, "codex", `nobody-next-${mode}`);
    await api(d.base, d.token, `/api/sessions/${next.id}/watches/${aid}`, { method: "PUT" });
    expect((await api(d.base, d.token, `/api/sessions/${next.id}/feedback?tier=piggyback`)).feedback).toHaveLength(1);
  });

  test(`${mode}: a ticked thread that disappears leaves the selection; X and Shift+S work from the keyboard`, async ({ page }) => {
    const { aid, ids } = await fresh(`Prune ${mode}`, 2);
    await openArtifact(page, d.base, aid, 1, mode);
    await panel(page);
    await page.locator("body").press("j");
    await page.locator("body").press("x");
    await page.locator("body").press("j");
    await page.locator("body").press("x");
    const count = page.getByRole("region", { name: "Selected comments" }).getByRole("status");
    await expect(count).toHaveText("2 selected");
    await page.request.post(`${d.base}/api/artifacts/${aid}/threads/${ids[0]}/resolve`, { headers: { origin: d.base } });
    await expect(count).toHaveText("1 selected");
    await page.locator("body").press("Shift+S");
    await expect(page.getByRole("region", { name: "Selected comments" })).toHaveCount(0);
  });
}
```

Run: `cd web && npx vitest run && npm run lint && npm run typecheck && npm run build && node scripts/bundle-size.mjs && npx playwright test e2e/batch.spec.ts; echo "exit=$?"`
Expected: `exit=0`. `SelectionBar` is outside `artifact.html`'s closure. If the `artifact` budget fails, lazy-load `SendButton`'s menu next. Do not raise the budget.

- [ ] **Step 6: Screenshots and a look**

Append a `bulk` scene to `web/e2e/scenes.ts`: open the panel, tick the first and third card (Shift-click the third), and type a note.

Run: `cd web && CLAX_SHOTS=task-23 CLAX_SCENES=bulk,threads npx playwright test e2e/shots.spec.ts`
Expected: PASS.

The selection bar sits at the top of the sidebar, as the batch decision records (spec §8); the Echo mockup's bar at the foot is superseded. Compare everything else with the mockup.

Report, against `concept-3-echo/shots/*-bulk.png`:
- the checkboxes beside each card head;
- the selection bar at the top, under `Send N unsent`, with its dots, `2 selected` over `sent together`, Clear, and `Send 2 to claude`, staying in view while the list scrolls;
- the note field;
- `Send N unsent to claude` at the top;
- the phone layout, with buttons at least 40px and no sideways scroll.

With VoiceOver (Cmd+F5) in a headed browser:
- check that the count is announced as it changes;
- check that each checkbox reads its label.


Run: `cd web && npm run perf; echo "exit=$?"`
Expected: `exit=0`. Stop rule: if any of the five measures in `web/perf/budget.json` is over budget (link to first paint `firstPaint`, link to comment ready `commentReady`, frame paint `framePaint`, ready latency `readyLatency`, cold ready latency `coldLatency`, in either frame mode), stop and report all five numbers against their budgets. Lazy-load first; never raise a budget.

- [ ] **Step 7: Gates and staging**

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add web/shell/src/view/batch-model.ts web/shell/src/view/batch-model.test.ts web/shell/src/view/send-target.ts web/shell/src/view/send-target.test.ts \
  web/shell/src/ui/SelectionBar.svelte web/shell/src/ui/SendButton.svelte web/e2e/batch.spec.ts web/e2e/scenes.ts
git add -u web/shell/src
git status --short   # staged; the controller commits ("Send several threads at once in Echo: checkboxes, the selection bar with a note, and an agent picker")
```

---

### Task 24: Presence and the people panel (decided: Q5, Q7)

This task adds:
- here or away in the roster;
- the location, in the people panel only;
- the people panel itself, opened from the roster or with P, which also holds the viewer's name.

Presence lives in memory in the daemon, like working: a restart starts with none, and reports lapse 90 s after the last one. Multiplayer is coming later, so this is the light layer the brief describes, and it stays out of the way: it never moves or covers the page.

**Files:**
- Create: `crates/clax-core/src/presence.rs`, `crates/clax-server/tests/api_presence.rs`, `web/shell/src/view/presence-model.ts`, `web/shell/src/view/presence-model.test.ts`, `web/shell/src/ui/PeoplePanel.svelte`, `web/e2e/presence.spec.ts`
- Modify: `crates/clax-core/src/lib.rs`, `crates/clax-core/src/events.rs` (`Event::Presence`), `crates/clax-server/src/state.rs`, `crates/clax-server/src/daemon.rs` (sweeper), `crates/clax-server/src/testing.rs`, `crates/clax-server/src/routes/viewers.rs`, `crates/clax-server/src/routes/artifacts.rs`, `crates/clax-server/src/routes/mod.rs`, `web/shell/src/api.ts`, `web/shell/src/events.ts`, `web/shell/src/view/keys.ts`, `web/shell/src/view/keys.test.ts`, `web/shell/src/view/artifact-controller.ts`, `web/shell/src/view/artifact-controller.test.ts`, `web/shell/src/ui/TopbarIsland.svelte`, `web/shell/src/ui/SidebarIsland.svelte`, `web/shell/src/ui/Roster.svelte`, `web/shell/src/theme.css`, `web/e2e/fixtures.ts`, `web/e2e/scenes.ts`, and every e2e spec that fills `Your name` (`grep -rln "Your name" web/e2e`)

**Interfaces:**
- `clax_core::presence`:
  - `PRESENCE_TTL_SECS: i64 = 90`, `GONE_KEEP_SECS: i64 = 600`, `MAX_WHERE_CHARS: usize = 80`;
  - `enum State { Here, Away, Gone }`;
  - `PresenceView { public_id, display_name, state, r#where: Option<String>, since: String }`;
  - `Presence::new(clock: Arc<dyn Clock>)`, `report(aid, public_id, display_name, state, where_) -> bool` (whether the visible view changed), `sweep() -> Vec<String>` (the artifacts that changed), and `for_artifact(aid) -> Vec<PresenceView>`.
  - `Gone` means a report has lapsed: `since` is the last report, and the shell shows "last here <time>".
- `Event::Presence { artifact_id: String, people: Vec<PresenceView> }`, with the SSE name `presence`.
- The panel reads each person's last viewed version from `participants.people[].seen`, which Task 15 serves publicly (decided: Q7). This task adds no field for it, and serves no looked-at mark of anyone's.
- HTTP:
  - `PUT /api/viewers/me/presence` (`SameOrigin`, a cookie is required) takes `{artifact_id, state: "here" | "away", where?}` and answers `{people}`;
  - `GET /api/artifacts/<aid>/presence` answers `{people}`;
  - the artifact view's bootstrap block does not carry presence, since it is fetched after load.
- Shell:
  - `ViewState.presence: PresenceView[]` and `ViewState.shareWhere: boolean` (from `localStorage` `clax.shareWhere`, default true);
  - `ctl.setShareWhere(on)`;
  - a private reporter: a report on start, on `visibilitychange`, when the selection or the composer's anchor changes, on input after an away period, and every 30 s;
  - `shortcut("people")` calls `openMenu("people")`;
  - the `.who` block becomes a button that does the same;
  - `presence-model.ts`: `stateFor(visible: boolean, idleMs: number): "here" | "away"` (away after 5 minutes idle), `whereLabel(s: ViewState): string | null`, and `personLine(p, now): string`.

- [ ] **Step 1: The registry, test first**

`crates/clax-core/src/presence.rs`, tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::working::ManualClock;
    use std::sync::Arc;

    fn reg() -> (Arc<ManualClock>, Presence) {
        let c = Arc::new(ManualClock::at("2026-09-30T10:00:00Z"));
        (c.clone(), Presence::new(c))
    }

    #[test]
    fn a_report_is_here_until_it_lapses_then_gone_then_dropped() {
        let (c, p) = reg();
        p.report("a1", "u_a", Some("Alex"), State::Here, Some("«Quarterly goals»"));
        assert_eq!(p.for_artifact("a1")[0].state, State::Here);
        assert_eq!(p.for_artifact("a1")[0].r#where.as_deref(), Some("«Quarterly goals»"));
        c.advance(91);
        assert_eq!(p.sweep(), vec!["a1".to_string()]);
        let v = &p.for_artifact("a1")[0];
        assert_eq!((v.state, v.r#where.is_none()), (State::Gone, true), "a lapsed report keeps no location");
        c.advance(600);
        p.sweep();
        assert!(p.for_artifact("a1").is_empty());
    }

    #[test]
    fn where_is_cleaned_and_bounded_and_away_keeps_no_location() {
        let (_c, p) = reg();
        p.report("a1", "u_a", None, State::Here, Some(&format!("  {}\n", "x".repeat(100))));
        assert_eq!(p.for_artifact("a1")[0].r#where.as_ref().unwrap().chars().count(), 80);
        p.report("a1", "u_a", None, State::Away, Some("chart"));
        assert!(p.for_artifact("a1")[0].r#where.is_none());
    }
}
```

`ManualClock::advance(secs)` is Task 7's. The registry takes the same `Clock` trait (`crate::working::Clock`).

Above the tests, the registry has these parts:
- a `Mutex<BTreeMap<(String, String), Entry>>`, where `Entry { display_name, state, where_, last_report: DateTime<Utc> }`, and an `Arc<dyn Clock>`;
- `report`, which replaces the entry and sets `last_report = now`. `where_` goes through `crate::working::clean_line(w, MAX_WHERE_CHARS)`, and is dropped unless the state is `Here`. It returns whether the visible view changed;
- `for_artifact`, which sorts by state (here, away, gone), then by name. An entry past `last_report + 90 s` reads as `Gone` with no `where`, at once;
- `sweep`, which turns lapsed entries into `Gone`, drops those past `last_report + 90 + 600 s`, and returns the artifacts whose view changed.

Register `pub mod presence;` in `lib.rs`. Add `Event::Presence` to `events.rs` the way Task 7 added `Event::Working`: its variant, `artifact_id()`, `name()` (`"presence"`), and the names test.

Run: `cargo test -p clax-core presence`
Expected: PASS.

- [ ] **Step 2: The routes, test first**

`crates/clax-server/tests/api_presence.rs`:

```rust
mod common;
use clax_core::presence::Presence;
use clax_core::working::ManualClock;
use common::TestServer;
use serde_json::{Value, json};
use std::sync::Arc;

#[tokio::test]
async fn presence_is_reported_by_viewers_announced_and_lapses() {
    let c = Arc::new(ManualClock::at("2026-09-30T10:00:00Z"));
    let pc = c.clone();
    let ts = TestServer::spawn_with(move |s| s.presence = Arc::new(Presence::new(pc))).await;
    let a = ts.publish("T", &[("index.html", "<p>")]).await;
    let aid = a["artifact"]["id"].as_str().unwrap().to_string();
    let alex = ts.viewer(Some("Alex")).await;
    let mut ev = ts.events(&format!("?artifact={aid}&types=presence")).await;
    let put = |state: &'static str, origin: Option<&'static str>| {
        let (ts, aid, cookie) = (&ts, aid.clone(), alex.cookie.clone());
        async move {
            let mut r = ts.client.put(format!("{}/api/viewers/me/presence", ts.base)).header("cookie", format!("clax_viewer={cookie}"))
                .json(&json!({"artifact_id": aid, "state": state, "where": "«Quarterly goals»"}));
            if let Some(o) = origin { r = r.header("origin", o); }
            r.send().await.unwrap()
        }
    };
    assert_eq!(put("here", None).await.status(), 200);
    let e = ev.next_named("presence").await;
    assert_eq!(e["people"][0]["display_name"], "Alex");
    assert_eq!(e["people"][0]["state"], "here");
    assert_eq!(e["people"][0]["where"], "«Quarterly goals»");
    assert!(!e.to_string().contains(&alex.cookie), "never the cookie");
    assert_eq!(put("here", Some("http://evil.example")).await.status(), 403);
    let anon = ts.client.put(format!("{}/api/viewers/me/presence", ts.base)).json(&json!({"artifact_id": aid, "state": "here"})).send().await.unwrap();
    assert_eq!(anon.status(), 400);
    c.advance(91);
    clax_server::presence::sweep_and_announce(&ts.presence, &ts.events);
    assert_eq!(ev.next_named("presence").await["people"][0]["state"], "gone");
    let g: Value = ts.get(&format!("/api/artifacts/{aid}/presence")).await.json().await.unwrap();
    assert_eq!(g["people"][0]["state"], "gone");
}
```

Run: `cargo test -p clax-server --test api_presence`
Expected: FAIL.

Implement, following Task 8's working wiring:
- `AppState.presence: Arc<Presence>`, initialised with `SystemClock` in `daemon.rs` and `testing.rs`;
- `TestServer.presence`;
- `crates/clax-server/src/presence.rs` with `sweep_and_announce`, which publishes `Event::Presence` for each changed artifact;
- the 5 s sweeper (Task 8) also calls `crate::presence::sweep_and_announce`;
- `routes/viewers.rs::set_presence`, which reads the viewer (400 `no_viewer` without one), checks that the artifact exists (404), takes the viewer's `public_id` and `display_name`, reports, and announces when the report changed something;
- `routes/artifacts.rs::presence` for the GET;
- in `routes/mod.rs`, the PUT before `/api/viewers/me`, and `.route("/api/artifacts/{aid}/presence", get(artifacts::presence))`.

Run: `cargo test --workspace`
Expected: PASS.

- [ ] **Step 3: The shell model, test first**

`web/shell/src/view/presence-model.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { personLine, stateFor } from "./presence-model";

describe("presence-model", () => {
  it("is here while visible and active, away when hidden or idle 5 minutes", () => {
    expect([stateFor(true, 1000), stateFor(false, 0), stateFor(true, 5 * 60_000)]).toEqual(["here", "away", "away"]);
  });
  it("reads a person's line", () => {
    const now = new Date("2026-09-30T10:03:00Z");
    expect(personLine({ public_id: "u", display_name: "Mia", state: "here", where: "«p95 chart»", since: "x" }, now)).toBe("here, looking at «p95 chart»");
    expect(personLine({ public_id: "u", display_name: "Mia", state: "here", where: null, since: "x" }, now)).toBe("here");
    expect(personLine({ public_id: "u", display_name: "Jun", state: "gone", where: null, since: "2026-09-30T10:00:00Z" }, now)).toBe("last here 3 min ago");
  });
});
```

`web/shell/src/view/presence-model.ts`:

```ts
// Presence (spec §10, "Presence"): here or away, and where, if shared.
import { relativeTime } from "../format";
import { anchorLabel } from "../threads";
import type { ViewState } from "./artifact-controller";

export type PresenceView = { public_id: string; display_name: string | null; state: "here" | "away" | "gone"; where: string | null; since: string };
export const AWAY_AFTER_MS = 5 * 60_000;
export const stateFor = (visible: boolean, idleMs: number): "here" | "away" => (visible && idleMs < AWAY_AFTER_MS ? "here" : "away");

/** Where this viewer is looking: the thread they selected, else the anchor they are writing on. */
export function whereLabel(s: ViewState): string | null {
  const t = s.threads.find(x => x.id === s.selected);
  if (t) return anchorLabel(t.anchor);
  return s.draft?.anchor ? anchorLabel(s.draft.anchor) : null;
}

export function personLine(p: PresenceView, now: Date): string {
  if (p.state === "gone") return `last here ${relativeTime(p.since, now)}`;
  if (p.state === "away") return "away";
  return p.where ? `here, looking at ${p.where}` : "here";
}
```

If `Draft` names its anchor differently, read it from where `Composer.svelte` reads it.

Run: `cd web && npx vitest run shell/src/view/presence-model.test.ts`
Expected: PASS.

- [ ] **Step 4: The controller, the panel, the roster**

`view/artifact-controller.ts`:
- `ViewState.presence: []` and `shareWhere`, read guarded from `localStorage` (default `true`).
- `onEvent` handles `presence` with `this.set({ presence: e.people })`. `events.ts` adds the event and its listener name.
- After load, fetch `GET /api/artifacts/<id>/presence` once, after paint.
- A private reporter:
  - `report()` sends `PUT /api/viewers/me/presence` with `stateFor(document.visibilityState === "visible", Date.now() - lastInput)`, plus `where: this.s.shareWhere ? whereLabel(this.s) : null`;
  - it runs at most once per 2 s, unless the state flips;
  - it runs on start, on `visibilitychange`, on a selection or draft change (in `react()`), on the first input after away, and every 30 s (an interval cleared in `dispose`);
  - it reports only when `this.s.me` is set (a viewer cookie exists).
- `setShareWhere(on)` stores the choice, sets it, and reports.
- `shortcut("people")` calls `this.openMenu("people")`.

`view/keys.ts`: add `p: "people"`, and the row `{ keys: ["P"], what: "People and agents here", action: "people" }` last. Update `keys.test.ts`.

`ui/Roster.svelte` gains `presence` (public ID → `"here" | "away" | "gone"`). The `here` and `away` classes come from it, and `gone` renders as `away`.

The top bar's roster shows everyone present, not only comment authors. In `TopbarIsland.svelte`, pass `people` as the union of `parts.people` and the non-gone entries of `s.presence`, keyed by `public_id` (presence supplies `display_name` for viewers who have not commented), the same union the people panel lists. Put it in the model as `presence-model.ts` `roster(people, presence): Participants["people"]`, with a test: two viewers present who never commented give two people, and a gone entry adds no one.

`ui/TopbarIsland.svelte`:
- the `.who` `div` becomes `<button type="button" class="who" aria-haspopup="dialog" aria-expanded={s.menu === "people"} aria-label="People and agents" onclick={() => ctl.openMenu("people")}>`;
- its `Roster` gets the presence map;
- remove `{#if !s.narrow}<ViewerName …/>{/if}`;
- after the button, while `s.menu === "people"`:

```svelte
    {#await import("./PeoplePanel.svelte") then { default: PeoplePanel }}
      <PeoplePanel {ctl} {s} onClose={() => ctl.closeMenu()} />
    {/await}
```

`ui/SidebarIsland.svelte`: remove the narrow `nameField` header, because the name now lives in the panel.

`web/shell/src/ui/PeoplePanel.svelte` (lazy) is a `role="dialog" aria-label="People and agents"` panel, 420px wide, a full sheet at phone width. Escape and an outside click close it and return focus to the `.who` button. It holds:
- **People · N**: one row per participant and present viewer (the union of `participants.people` and `presence`). Each row has its token, the name (with `you` for this viewer), `personLine`, and a muted line: `In N threads.` (open threads whose comments carry their `author_public_id`), then `Seen vK.` from `participants.people[].seen` (Task 15; public, decided: Q7), or nothing when it is null. A viewer who is present but never commented has no participants row, so no `Seen` line.
- **Agents · N**: one row per agent. Each row has its token, the name, and one of these:
  - the threads it works on (`On #1 (yours) and #3`, from `stripText`) with the clock, and a `HaikuLine` seeded by the record key, hidden in comment mode;
  - or `Idle`, plus `Addressed #2 in v5` for the newest version it published that addressed threads.
- **The name row**: `You are <name>` with the port's `ViewerName` field. Under it, a checkbox `Share where I'm looking` bound to `ctl.setShareWhere` (on by default, stored per browser).
- Agent names follow Task 16's rule: the harness, with the handle's first four hex digits when two agents on the artifact share one (`claude 7f3a`).

Styles, appended to `theme.css` (the mockup's `.pop` and `.prow`):

```css
/* The people panel (spec §8). */
.people { position: absolute; top: 56px; left: 16px; z-index: 20; width: 420px; max-height: 70vh; overflow: auto; background: var(--raised); border: 1px solid var(--border-strong); box-shadow: 0 14px 40px var(--shadow); padding: 6px 0 8px; }
.people h3 { margin: 10px 16px 6px; font: 600 14px var(--grot); color: var(--muted); display: flex; align-items: center; gap: 8px; }
.people h3 .sw { width: 7px; height: 14px; } .people .ph .sw { border-radius: 0 7px 7px 0; background: var(--you); } .people .ah .sw { border-radius: 7px 0 0 7px; background: var(--agent); }
.prow { display: grid; grid-template-columns: 52px 1fr; gap: 2px 10px; padding: 7px 16px; align-items: start; }
.prow b { font: 600 15px/1.2 var(--grot); } .prow b small { font: 400 11.5px var(--mono); color: var(--muted); margin-left: 6px; }
.prow p { grid-column: 2; margin: 0; font-size: 12.5px; line-height: 1.45; } .prow .tok { grid-row: span 2; justify-self: start; }
.prow .hk { grid-column: 2; }
.prow.edit { border-top: 1px solid var(--border); margin-top: 6px; padding-top: 10px; display: flex; flex-wrap: wrap; gap: 8px; align-items: center; font-size: 12px; color: var(--muted); }
button.who { cursor: pointer; font: inherit; }
@media (max-width: 700px) { .people { position: fixed; left: 0; right: 0; top: 56px; bottom: 52px; width: auto; max-height: none; box-shadow: none; border-width: 1px 0 0; } }
```

- [ ] **Step 5: The name field moves: update the e2e specs**

Add to `web/e2e/fixtures.ts`:

```ts
/** Sets this page's viewer name through the people panel, as a person does. */
export async function setName(page: Page, name: string) {
  await page.getByRole("button", { name: "People and agents" }).click();
  const field = page.getByRole("dialog", { name: "People and agents" }).getByLabel("Your name");
  await field.fill(name);
  await field.press("Enter");
  await page.keyboard.press("Escape");
}
```

Every use of the field moves with it. Add a second helper, and a Close button at the panel's foot (`<button type="button" class="ghost">Close</button>`, which closes it and returns focus to `.who`):

```ts
/** The name field, in the open people panel: the shell text control the gesture tests click. */
export async function nameField(page: Page) {
  if (!(await page.getByRole("dialog", { name: "People and agents" }).isVisible())) await page.getByRole("button", { name: "People and agents" }).click();
  return page.getByRole("dialog", { name: "People and agents" }).getByRole("textbox", { name: "Your name" });
}
```

Then, use by use:
- `comments-capability.spec.ts:76`, `contract.spec.ts:209`, `db.spec.ts:60`, `user-assets.spec.ts:20` and `:46`, `comment-flows.spec.ts:225` and `:348` (each `const name = page.getByRole("textbox", { name: "Your name" })` followed by a fill and Enter): `await setName(page, …)` in place of the pair; where the test keeps using `name` (to read it back), `const name = await nameField(page)`.
- `db.spec.ts:146-147` (the fill and press pair): `await setName(page, "Ada")`.
- `contract.spec.ts:130` (a click on the field): `await (await nameField(page)).click()`, then close the panel with its Close button.
- `comment-flows.spec.ts:282`, `:298` and `:325`, the gesture tests that use the field as the shell input (`clickShell(page, textbox)`): `await clickShell(page, await nameField(page))`, then `await clickShell(page, page.getByRole("dialog", { name: "People and agents" }).getByRole("button", { name: "Close" }))`. The last shell input is still a click in the shell, as each test needs, and the panel no longer covers the page.
- Task 17's capability test in `working.spec.ts`: `await setName(page, "Alex")`.

Afterwards `grep -rn '"Your name"' web/e2e` prints only `fixtures.ts`. Run the whole suite, as Step 6 does.

- [ ] **Step 6: Browser tests**

`web/e2e/presence.spec.ts`:

```ts
import { test, expect } from "@playwright/test";
import { openArtifact, postThread, publishAs, registerSession, setName, startDaemon } from "./fixtures";

let d: Awaited<ReturnType<typeof startDaemon>>;
test.beforeAll(async () => { test.setTimeout(180_000); d = await startDaemon(); });
test.afterAll(async () => { await d?.stop(); });

for (const mode of ["subdomain", "sandbox"] as const) {
  test(`${mode}: two viewers see each other here, the location only in the panel, and away when hidden`, async ({ browser }) => {
    const s = await registerSession(d.base, d.token, "claude", `pres-${mode}`);
    const { artifact } = await publishAs(d.base, d.token, s.id, `Presence ${mode}`, { "index.html": "<main><h2>Quarterly goals</h2></main>" });
    const t = await postThread(d.base, artifact.id, "Two columns");
    const alex = await (await browser.newContext()).newPage();
    const mia = await (await browser.newContext()).newPage();
    await openArtifact(alex, d.base, artifact.id, 1, mode);
    await openArtifact(mia, d.base, artifact.id, 1, mode);
    await setName(alex, "alex");
    await setName(mia, "Mia");
    if (!(await mia.locator("aside.sidebar").isVisible())) await mia.getByRole("button", { name: /Threads/ }).first().click();
    await mia.locator(`.thread-card[data-thread="${t.id}"] .card-head`).click();
    // Mia replies, so she is a participant whose last viewed version (v1, public) Alex can read.
    await mia.request.post(`${d.base}/api/artifacts/${artifact.id}/threads/${t.id}/comments`, { headers: { origin: d.base }, data: { body: "Agreed" } });
    await alex.reload();
    await expect(alex.locator(".who .ppl .tok.here")).toHaveCount(2);
    await expect(alex.locator(".who")).not.toContainText("looking at");
    await alex.locator("body").press("p");
    const panel = alex.getByRole("dialog", { name: "People and agents" });
    await expect(panel.locator(".prow", { hasText: "Mia" })).toContainText("here, looking at");
    await expect(panel.locator(".prow", { hasText: "Mia" })).toContainText("Seen v1.");
    await mia.evaluate(() => { Object.defineProperty(document, "visibilityState", { value: "hidden", configurable: true }); document.dispatchEvent(new Event("visibilitychange")); });
    await expect(panel.locator(".prow", { hasText: "Mia" })).toContainText("away");
    await alex.context().close();
    await mia.context().close();
  });
}
```

Run: `cd web && npx vitest run && npm run lint && npm run typecheck && npm run build && node scripts/bundle-size.mjs && npx playwright test; echo "exit=$?"`
Expected: `exit=0`. The full suite runs, because the name field moved. `PeoplePanel` is outside `artifact.html`'s closure.

- [ ] **Step 7: Screenshots and a look**

Append a `multiplayer` scene to `web/e2e/scenes.ts`:
1. a second browser context names itself "Mia" and selects a thread;
2. a working record runs on one thread;
3. the main page opens the panel with P.

Run: `cd web && CLAX_SHOTS=task-24 CLAX_SCENES=multiplayer,view npx playwright test e2e/shots.spec.ts`
Expected: PASS.

Report, against `concept-3-echo/shots/*-multiplayer.png`:
- the roster's here dots;
- the panel's two sections;
- Mia's location shown in the panel and nowhere else;
- the agent row with its clock and haiku;
- the name row and the share switch;
- the phone sheet.

Run: `cd web && npm run perf; echo "exit=$?"`
Expected: `exit=0`. Stop rule: if any of the five measures in `web/perf/budget.json` is over budget (link to first paint `firstPaint`, link to comment ready `commentReady`, frame paint `framePaint`, ready latency `readyLatency`, cold ready latency `coldLatency`, in either frame mode), stop and report all five numbers against their budgets. Lazy-load first; never raise a budget.

- [ ] **Step 8: Gates and staging**

```bash
bash scripts/quality_gates.sh; echo "exit=$?"
git add crates/clax-core/src/presence.rs crates/clax-server/tests/api_presence.rs crates/clax-server/src/presence.rs web/shell/src/view/presence-model.ts \
  web/shell/src/view/presence-model.test.ts web/shell/src/ui/PeoplePanel.svelte web/e2e/presence.spec.ts web/e2e/fixtures.ts web/e2e/scenes.ts
git add -u crates web/shell/src web/e2e
git status --short   # staged; the controller commits ("Show who is here and where they look, in a people panel that also holds your name")
```

---

### Task 25: The Echo pass: rally of 10 in the top bar, the full screenshot sweep, and every gate

This task finishes Echo's last easter egg, then checks the whole redesign against the mockup in one sweep, in light and dark, at desktop and phone width. It runs every gate. Nothing new is built beyond the rally line. A defect the sweep finds is fixed here when it is a styling slip. When it is a design question, it is reported to the controller.

**Files:**
- Create: `web/shell/src/view/rally.ts`, `web/shell/src/view/rally.test.ts`
- Modify: `web/shell/src/view/working-model.ts` (`summary` takes `rally`), `web/shell/src/view/working-model.test.ts`, `web/shell/src/view/artifact-controller.ts` (`ViewState.rally`), `web/shell/src/ui/TopbarIsland.svelte`, `web/e2e/echo.spec.ts`, `web/e2e/scenes.ts`, plus whatever styling slips the sweep finds in `web/shell/src/theme.css` and the components

**Interfaces:**
- `view/rally.ts`: `rallyOnce(aid: string, version: number): boolean`. It is true once per browser per artifact, the first time the viewer views v10 (decided: Q9). It keeps `clax.rally.<aid>` in `localStorage`, guarded.
- `summary(...)` takes `rally: boolean`. With nobody working and nothing addressed, it appends ` · rally of 10` to `line2`.

- [ ] **Step 1: Rally of 10, test first**

`web/shell/src/view/rally.test.ts`:

```ts
import { afterEach, describe, expect, it } from "vitest";
import { rallyOnce } from "./rally";

afterEach(() => localStorage.clear());

describe("rally", () => {
  it("fires once per artifact, on v10 only", () => {
    expect(rallyOnce("a", 9)).toBe(false);
    expect(rallyOnce("a", 10)).toBe(true);
    expect(rallyOnce("a", 10)).toBe(false);
    expect(rallyOnce("b", 10)).toBe(true);
  });
});
```

`web/shell/src/view/rally.ts`:

```ts
// "Rally of 10" (spec §8, "Look"): an easter egg, shown once per browser per
// artifact when its tenth version is first viewed. Storage may throw.
export function rallyOnce(aid: string, version: number): boolean {
  if (version !== 10) return false;
  try {
    const k = `clax.rally.${aid}`;
    if (localStorage.getItem(k)) return false;
    localStorage.setItem(k, "1");
    return true;
  } catch { return false; }
}
```

The controller computes it, not the island: `ViewState` gains `rally: boolean` (initially `false`), and `decideChangelog()` sets `rally: rallyOnce(this.id, this.shown(s))` when it first decides. `TopbarIsland.svelte` passes `s.rally` to `summary`. In `working-model.ts`, the `Nobody working` branch appends ` · rally of 10` when `rally`. Add that case to `working-model.test.ts`, and pass `rally: false` in the others.

Run: `cd web && npx vitest run shell/src/view/rally.test.ts shell/src/view/working-model.test.ts`
Expected: PASS.

Add to `web/e2e/echo.spec.ts` a test that publishes ten versions as one session, opens v10, and expects `.who .sum .l2` to contain `rally of 10`. After a reload it no longer does. In the gallery, the card's `.chip.rally` reads `rally of 10`.

- [ ] **Step 2: The sweep**

Run every scene: `cd web && CLAX_SHOTS=task-25 npx playwright test e2e/shots.spec.ts && CLAX_SHOTS=task-25 CLAX_SHOTS_EMPTY=1 npx playwright test e2e/shots.spec.ts`
Expected: PASS. Every scene (`gallery`, `gallery-empty`, `view`, `comment`, `threads`, `keys`, `working`, `changelog`, `versions`, `bulk`, `multiplayer`) is shot in light and dark, at 1440×900 and 390×844.

Put each against its counterpart in `.superpowers/sdd/2026-09-30-redesign/concept-3-echo/shots/`, which uses the same theme, size and scene names. Write a table in the task report with one row per scene. For each scene and each of the four variants, it says whether these match the mockup, with a short note where they differ:
- the type (condensed for structure, mono for words);
- the colours (people red-orange, agents green, pink only as an accent);
- sentence case;
- spacing;
- nothing over the page;
- the phone layout.

Fix every styling slip in `theme.css` or the component, and shoot that scene again. Report every design difference, such as a missing thumbnail or a different word, as a question for the controller, and do not change the design. Thumbnails are expected to be missing (decided: Q1).

Then check by hand in a headed browser:
- in comment mode, no haiku shows anywhere: the sidebar strip hides its haiku, and the gallery is not in view;
- under reduced motion, nothing moves: the sweep, the breathing dot, the mark and the converging dots;
- the theme switch and the system scheme work together as in Task 3;
- the `?` sheet lists C, Esc, T, J and K, Enter, S, R, V, X, Shift+S and P, and each key works;
- the tab's favicon is the Echo mark in light and dark tab strips.

- [ ] **Step 3: Every gate**

```bash
grep -rniE "your move|whose move|agents' move|'s move|\bround [0-9]|(your|their|whose|next) turn|nothing waits on you|settled\." web/shell/src --include=*.svelte --include=*.ts --include=*.json | grep -v '\.test\.ts:'
grep -niE "\bturns?\b" web/shell/src/view/haiku.json
grep -rn "text-transform: *uppercase" web/shell/src
grep -rlE "from \"svelte" web/bridge web/shell/src/caps web/shell/src/view
grep -rn "Send to agent\|changelog-banner\|Show changes" web/shell/src web/e2e
```

Expected: no output from any of the five.

```bash
cd web && npm run lint && npm run typecheck && npx vitest run && npm run build && node scripts/bundle-size.mjs && npx playwright test && npm run perf; echo "exit=$?"
```

Expected: `exit=0`, every budget held. Put the printed sizes (`gallery`, `artifact`, `bridge`, `fonts`) and the perf medians into the task report, beside the port's numbers in `web/perf/budget.json` and `web/perf/bundle-budget.json`.

```bash
cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && bash scripts/test-plugins.sh && scripts/smoke-comment-loop.sh
bash scripts/quality_gates.sh; echo "exit=$?"
```

Expected: every command passes, ending with `exit=0`.


Run: `cd web && npm run perf; echo "exit=$?"`
Expected: `exit=0`. Stop rule: if any of the five measures in `web/perf/budget.json` is over budget (link to first paint `firstPaint`, link to comment ready `commentReady`, frame paint `framePaint`, ready latency `readyLatency`, cold ready latency `coldLatency`, in either frame mode), stop and report all five numbers against their budgets. Lazy-load first; never raise a budget.

- [ ] **Step 4: Staging**

```bash
git add web/shell/src/view/rally.ts web/shell/src/view/rally.test.ts web/e2e/echo.spec.ts web/e2e/scenes.ts
git add -u web/shell/src web/e2e
git status --short   # staged; the controller commits ("Finish Echo: rally of 10 in the top bar, and the redesign checked against the mockup in both themes")
```
