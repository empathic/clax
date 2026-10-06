# Clax verification

This document is the evidence that Clax at `main` = `f680986` does what its
two specs say, and a plain list of everything that evidence does not cover.
It is written for a reader who believes none of it. Every claim names the
command that produced it and its output, or a file and line.

Clax is a local daemon plus a web shell. Coding agents (Claude Code, Codex,
Pi, Grok Build) publish HTML artifacts to the daemon through an MCP shim or
the Pi extension; people open them in a browser, comment on elements, text
ranges or areas, and send the comments to the agent; the comments reach that
agent's session, and the agent replies and resolves.

## How this evidence was produced

- Commit under test: `git log -1 --format='%H %cd' main` →
  `f68098618b5ebf97e3e7e9d84e72681e4d505e62 Fri Oct 2 22:22:00 2026 -0400`.
- Every run happened in a detached scratch worktree, created with
  `git worktree add --detach $SCRATCH/verify-wt main`, and removed afterwards.
  `$SCRATCH` stands for the session scratch directory under
  `/private/tmp/claude-501/`; it is abbreviated in every quoted output below.
- Machine: macOS 26.6.2, arm64, 12 cores (`sw_vers -productVersion`,
  `uname -m`, `sysctl -n hw.ncpu`). Toolchain: `rustc 1.94.0`, `node v26.8.2`,
  `just 1.58.0`. The README asks for Node 22 (`README.md:7`); these gates ran
  on Node 26.
- Every daemon in this document ran in a scratch `CLAX_HOME` on a port the
  kernel picked (`--port 0`). Ports 7480, 7481 and 7490, `~/.clax` and
  `~/.clax-dev` were not used. No real harness CLI (`claude`, `codex`, `grok`,
  `pi`) was run; every harness in these runs is a fake or a scripted MCP
  client.
- Nothing in this document comes from an earlier report.

## 1. Quality gates on main

Command, in the scratch worktree:

```
cd $SCRATCH/verify-wt && (time bash scripts/quality_gates.sh); echo "exit=$?"
```

Full output (started 2026-10-03T02:22:41Z, ended 02:46:33Z):

```
justfile                    ok
plugin wrapper              ok
release scripts             ok
release installer           ok
tool hook gate              ok
dev scripts                 ok
plugins                     ok
web lint                    ok
web typecheck + unit        ok
web build                   ok
web bundle size             ok
cargo fmt --check           ok
cargo clippy                ok
cargo check (no test features)ok
cargo test                  ok
comment loop                ok
pi extension                ok
web e2e                     ok
time to usable              ok
all gates passed
bash scripts/quality_gates.sh  782.69s user 173.48s system 66% cpu 23:52.25 total
exit=0
```

**Exit status 0, every gate passed on the first run.** No parked flaky test
failed, so none needed a rerun. No gate was edited, skipped or loosened; the
worktree had no local changes before or after (`git status --short` printed
nothing).

The gate script prints only `ok` for a passing gate and hides the gate's own
output (`scripts/quality_gates.sh:28-39`), so a `SKIP` line inside a passing
gate does not show above. Two are known; see section 6.

What the gates contain, counted separately in the same worktree after the run:

| Suite | Command | Result |
|---|---|---|
| Rust (unit + integration, 55 test binaries) | `cargo test --workspace \| grep '^test result'`, summed | 871 passed, 0 failed, 0 ignored |
| Shell unit tests (Vitest) | `cd web && npx vitest run --reporter=dot` | 92 files, 797 tests passed |
| Pi extension | `cd plugins/pi && npx vitest run --reporter=dot` | 1 file, 48 tests passed |
| Playwright e2e | `cd web && npx playwright test --list` | 443 tests in 31 files, of which 48 are `shots.spec.ts` screenshot tests that skip unless `CLAX_SHOTS` is set (`web/e2e/shots.spec.ts:16`); 395 run |
| Comment loop | `scripts/smoke-comment-loop.sh` | section 2.1 |
| Time to usable | `cd web && npm run perf` | section 7 |

No Rust test carries `#[ignore]` (`grep -rn '#\[ignore' crates` prints
nothing), and no web or Pi test uses `.only`, `.fixme` or `.skip` outside
`shots.spec.ts` (`grep -rnE 'test\.(skip|fixme|fail)|it\.skip|describe\.skip|\.only\('`
over `web/e2e`, `web/shell/src`, `web/bridge` and `plugins/pi`).

## 2. The comment loop, end to end

Three runs, each against a scratch daemon. Together they cover publish, a
comment made in a real browser, its arrival in the publishing agent's
session, and the reply and resolve. **None of them uses a real model
session**; section 2.4 says what that leaves open.

### 2.1 `scripts/smoke-comment-loop.sh` (real stdio shim, HTTP as the browser)

The agent side is the real `clax mcp --agent claude` shim, driven over
stdio by a scripted MCP client exactly as Claude Code would drive it
(`scripts/smoke-comment-loop.sh:58-101`). The browser side is the same
multipart `POST /api/artifacts/<id>/threads` and `.../send` calls the shell
makes. Tier 5 uses a fake `codex` that records its arguments
(`scripts/smoke-comment-loop.sh:26-35`).

```
TMPDIR=$SCRATCH/tmp bash scripts/smoke-comment-loop.sh; echo "exit=$?"
```

```
smoke: building clax
PASS: published through the shim: http://localhost:55091/a/nh1xhqb0c6pd (v1)
PASS: browser thread 01M3ZTGSAVWKFSG1VHC0QJKA4W created with a clip and sent; waiting on stop_hook
---
[clax] 1 comment sent to you:
[clax] Comment sent to you on "Quarterly Review" (http://localhost:55091/a/nh1xhqb0c6pd), thread 01M3ZTGSAVWKFSG1VHC0QJKA4W
Anchored on: body > main > h2  «Quarterly goals»  (v1)
Clip: $SCRATCH/tmp/clax-loop.ZEwYg3/home/artifacts/nh1xhqb0c6pd/clips/01M3ZTGSAVWKFSG1VHC0QJKA4W.png
Viewer: "Make this a two-column layout and drop the third bullet."
Reply with comments_reply, then comments_resolve when done.
PASS: tier 1: the next tool result carried the payload above, and the clip is a readable PNG
PASS: tier 1: delivered once
PASS: working: a record naming the thread appeared when the comment was delivered (key 01M3ZTGSAZCAJCJT8D90EQRAKT)
PASS: tier 2: the Stop hook blocked with thread 01M3ZTGSB2JT6R97DR64BA9GQP, then allowed the stop
PASS: working: cleared when the Stop hook allowed the stop (the turn ended)
PASS: comments_read returned the thread and acknowledged it (delivered via stop_hook -> acknowledged)
PASS: tier 4: wait_for_feedback returned 8 ms after the @agent comment was posted
PASS: working: wait_for_feedback returning a comment marked its thread
PASS: tier 4: an idle wait returns call_again after timeout_s
PASS: working tool: the top bar now reads 'claude: Two columns'
PASS: agent reply shown as 'claude', thread resolved, feedback acknowledged
PASS: working: the agent's reply to thread 1 took it out of the working record
PASS: a reply on a plain thread returns guidance and writes nothing
PASS: publish v2 carried its note and listed the named thread and the one it was working on, both still open
PASS: working: the publish cleared the session's working record
PASS: a thread on about.html (v2) reached the agent as 'Anchored on: about.html › body > main > h2  «Our team»  (v2)'
PASS: batch: three threads arrived in one delivery led by the note ([clax] 3 comments on "Quarterly Review", sent together by Viewer. Note: "Do these before the demo")
PASS: batch: every thread in it is marked working
PASS: batch: publish v3 listed all three threads as addressed
PASS: tier 5: the daemon ran `codex queue --thread cx-smoke --message <payload>` and marked the row delivered by queue
PASS: the shim exited when its stdin closed
PASS: no daemon left running (pid 81303 gone)
comment loop smoke passed
exit=0
```

### 2.2 Real browser and real shim in one run

Neither 2.1 nor the Playwright suite alone puts a real browser click and the
real stdio shim in the same loop: 2.1 plays the browser with HTTP, and
`web/e2e/comment-loop.spec.ts:41` plays the agent with
`GET /api/sessions/<id>/feedback?tier=piggyback`. So this run joined them.
A throwaway Node script (Appendix A; it is not in the repository) spawned
`target/debug/clax --port 0 mcp --agent claude` with
`CLAUDE_CODE_SESSION_ID=verify-loop-1` in a fresh scratch `CLAX_HOME`,
published through it, then drove Playwright's Chromium through the shell's
own controls: the People panel name field, **Comment**, a hover and click on
the page's `<h2>`, the composer, **Post comment** and **Send to claude**.
The agent side then made ordinary MCP tool calls.

```
cd $SCRATCH/verify-wt/web && node loop-verify.mjs $SCRATCH/verify-wt/target/debug/clax $SCRATCH/tmp; echo "exit=$?"
```

```
PASS: published through the stdio shim: http://localhost:55247/a/qre84y8xztw6 (v1); daemon port 55247, CLAX_HOME $SCRATCH/tmp/clax-verify-ioJByJ/home
PASS: status names the session: harness=claude
PASS: browser loaded the shell and the content frame at http://qre84y8xztw6.localhost:55247/v/1/
PASS: browser: picked the h2 in comment mode, posted with a clip, pressed "Send to claude"; card reads "sent, waiting for the agent · 0 s · waiting on the agent finishing its current work"
  | ---
  | [clax] 1 comment sent to you:
  | [clax] Comment sent to you on "Quarterly Review" (http://localhost:55247/a/qre84y8xztw6), thread 01M3ZTJSCCDS8C6M6035FN72QH
  | Anchored on: body > main > h2  «Quarterly goals»  (v1)
  | Clip: $SCRATCH/tmp/clax-verify-ioJByJ/home/artifacts/qre84y8xztw6/clips/01M3ZTJSCCDS8C6M6035FN72QH.png
  | Alex: "Make this a two-column layout and drop the third bullet."
  | Reply with comments_reply, then comments_resolve when done.
PASS: agent session: the next tool result (list) carried thread 01M3ZTJSCCDS8C6M6035FN72QH with author Alex and a PNG clip from the browser's capture
PASS: browser: the card shows "claude is working on it
0:00"
PASS: comments_read returned the thread (1167 bytes)
PASS: browser: the agent's reply appeared live, authored "claude"
PASS: browser: the thread moved to Resolved; history reads "v1 claude addressed it · v1 Alex commented · claude replied · claude resolved"
PASS: daemon: thread status=resolved, feedback_state={"exhausted":false,"resends":0,"since":"2026-10-03T02:48:06.872Z","state":"acknowledged","thread_id":"01M3ZTJSCCDS8C6M6035FN72QH","tier":"piggyback"}
PASS: shim exited, daemon pid 82688 stopped
loop verify passed
exit=0
```

The first attempt of this script failed after the resolve step, on the
script's own bug: it listed threads without `include_resolved=true`
(`crates/clax-server/src/routes/threads.rs:98`, `:117`), found no thread and
crashed before stopping the daemon. The daemon was stopped with
`CLAX_HOME=<that home> clax stop` → `clax daemon stopped`, the script was
corrected, and the run above is the second. The clip in this run is the
shell's own capture of the region (its first bytes are the PNG signature,
checked in the script), not a fixture.

This run covers the subdomain frame mode only. Both frame modes are covered by
`web/e2e/comment-loop.spec.ts:15`, which passed inside the gate run and again
alone (section 4.2).

### 2.3 `scripts/smoke-capabilities.sh` (runtime capabilities, room, sample)

```
TMPDIR=$SCRATCH/tmp bash scripts/smoke-capabilities.sh; echo "exit=$?"
```

Its `smoke:` lines and the Playwright summary (`grep -E '^smoke:|passed'`):

```
smoke: building clax
smoke: ok published the tracker page (yevsxyx95s7w)
smoke: ok db tools seed and read the tracker; unpinned writes, as_level, and data/users/me are refused
smoke: ok caller levels: unnamed view, named interact, token owner
smoke: ok private subtrees stay private over HTTP and SSE; doc events carry no bodies
smoke: ok PATCH capabilities replaces the declaration and the rules apply at once
smoke: ok a REST publish marked as the page's (X-Clax-Via: page) creates v2, and a stale one is a conflict naming v2
smoke: ok sample-status: available with the stub to the token, unavailable without it
smoke: ok sample-call: a streamed call ends with done "echo: hello"
smoke: ok sample-owner-only: no token is 401, no viewer cookie is 403 forbidden
smoke: ok sample-doctor: clax doctor's sample line names stub
smoke: ok room-socket: the second socket's first peers frame lists both, and it hears the first leave
smoke: running the claude.ai-page suite (web/e2e/contract.spec.ts), room.spec.ts and sample.spec.ts in both frame modes
  39 passed (42.5s)
smoke: ok the claude.ai sample pages, rooms and sample() run in both frame modes
smoke: all checks passed
exit=0
```

`sample()` ran only against the `stub` provider here. The Anthropic provider
is covered by `crates/clax-server/tests/sample_anthropic.rs` against a local
fake HTTP server, never against the real API.

### 2.4 What was not run: the loop through a real model session

**No agent has run the comment loop through a real Claude Code, Codex, Grok
Build or Pi model session.** Every run above uses a scripted MCP client, a
fake `codex`, or REST calls in place of the model. The scripts that do use a
real model and a real harness are the owner's to run. Each uses a scratch
home; the notes say what each touches.

```
# Claude Code: a real `claude -p` session publishes through the plugin
scripts/smoke-claude.sh --plugin-dir

# Claude Code tier 5: an idle interactive session wakes on a comment
#   (needs `just install` first, a logged-in claude, an interactive terminal)
scripts/smoke-claude-push.sh --channel
scripts/smoke-claude-push.sh --follow

# Codex: a real `codex exec` publishes; --hooks also checks the SessionStart
#   session ID (copies ~/.codex/auth.json into the scratch home, deletes it on exit)
scripts/smoke-codex.sh --hooks

# Grok Build: checks a, c, d, e, m and v of the live list (needs XAI_API_KEY or Grok's login)
scripts/smoke-grok.sh

# Pi: `pi -p` with only this extension loaded. It listens on the CLI's
#   default port, 7480, which must be free (scripts/smoke-pi.sh:16)
scripts/smoke-pi.sh

# clax init / uninit against the real claude, codex and pi CLIs (not grok)
scripts/verify-harnesses.sh

# Rooms and sample() by hand, in a scratch daemon; the stub provider unless
#   ANTHROPIC_API_KEY is set, in which case every call spends that key
just demo-room-sample          # runs scripts/demo-room-sample.sh
```

## 3. Install and run

From a clone (`README.md:9-47`, `justfile:75-79`):

```
just install          # npm ci + web build, cargo install --locked --root "${CARGO_HOME:-$HOME/.cargo}" --path crates/clax-cli,
                      # stops the agents' daemon if it runs that binary, then runs clax init
clax init             # register the plugins with each harness on PATH (claude, codex, pi, grok)
clax doctor --agent <claude|codex|pi|grok>
just uninstall        # clax uninit, stop that daemon, cargo uninstall clax-cli
```

Without a clone (`README.md:49-54`):

```
curl -fsSL https://github.com/empathic/clax/releases/latest/download/install.sh | bash
clax init
```

Working on Clax itself (`README.md:175-199`, `justfile:9-12`):

```
just dev claude       # also: just dev codex | just dev grok | just dev pi
just watch            # auto-reloading daemon and web UI on ~/.clax-dev, port 7481
```

What has and has not run:

| Step | Exercised by | Against |
|---|---|---|
| `just install` / `just uninstall` recipes | `scripts/test-dev.sh` (gate "dev scripts") | fake `cargo`, a fake clax and fake `claude`, `codex`, `grok`, `pi`, scratch `HOME` and `CARGO_HOME` (`scripts/test-dev.sh:2-6`) |
| `clax init` / `clax uninit` | `crates/clax-cli/tests/init.rs` (22 tests, for example `init_registers_each_harness_on_path_through_its_cli` at `:183`) | fake harness CLIs only |
| `scripts/verify-harnesses.sh` | `crates/clax-cli/tests/verify_harnesses.rs:351` `the_script_passes_against_fakes_and_never_touches_the_real_home` | fakes only; the script itself is meant for the real CLIs and has not run against them (`docs/follow-ups.md:17-20`) |
| `just dev <harness>` | `scripts/test-dev.sh` | fake harness commands only (`docs/follow-ups.md:14-16`) |
| `install.sh` | `scripts/test-install.sh` (gate "release installer") | a local fake release server on a kernel-picked port (`scripts/test-install.sh:2-3`); never against GitHub, and it returns 404 while the repository is private (`README.md:56-61`) |
| Release workflow | `scripts/test-release.sh` (version, bump, packaging) | never run on GitHub (`docs/follow-ups.md:31-39`) |
| The built binary, daemon, shell, shim, hooks | every gate in section 1, and section 2 | real, in scratch homes |

**Not one of `just install`, `clax init`, `just dev <harness>` or `install.sh`
has run against a real harness CLI or a real home in this verification, or
(per `docs/follow-ups.md:9-39`) by any agent before it.** This run did not
execute them at all, since each touches `~/.cargo/bin`, `~/.clax` or a real
harness's configuration.

## 4. Spec trace

Each row names the tests that pin a section. Paths are relative to the
repository root; `name` is the Rust function or the Playwright and Vitest
title, `:N` its line.

### 4.1 `docs/superpowers/specs/2026-09-28-clax-design.md`

| § | Section | Tests that pin it |
|---|---|---|
| 1 | Purpose and success criteria | the loop in section 2; `web/e2e/comments.spec.ts:26` "element thread: pick, compose, pin, send, agent reply, resolve"; `:52` "range thread quotes the selected text"; `web/e2e/contract.spec.ts:206` "`${mode}: ${file} runs unchanged`" (claude.ai pages in both frame modes) |
| 5 | Storage and data model | `crates/clax-core/src/store/*.rs` unit tests (artifacts 19, docs 19, feedback 27, sessions 18, threads 18, assets 10, changelog 7, viewers 6, batches 4, watches 3, migrations 3); `crates/clax-server/tests/daemon.rs:8` `daemon_info_roundtrips_with_0600` |
| 6 | HTTP API | `crates/clax-server/tests/api_artifacts.rs:6` `publish_get_list_roundtrip`, `:35` `republish_requires_matching_if_version`, `:65` `validation_errors_are_400_with_codes`; `api_assets.rs:5` `upload_serve_list_delete`; `api_content.rs:7` `serves_wrapped_index_and_files_with_caching_headers`; `api_events.rs:68` `publish_emits_version_event_filtered_by_artifact` |
| 7 | Daemon discovery and lifecycle | `crates/clax-cli/tests/cli.rs:63` `serve_starts_a_background_daemon_and_stop_ends_it`, `:129` `concurrent_auto_starts_yield_one_daemon`; `switch.rs:109` `serve_replaces_an_older_daemon_on_its_port` |
| 8 | Shell UI and viewer | `web/e2e/viewer.spec.ts:16` "gallery shows an empty state then a card", `:25` "viewer renders content with the bridge, and offers Reload on republish", `:47` "fallback mode uses the sandboxed path-based frame"; `web/e2e/boot.spec.ts:15` "a second visit gets the frame in the HTML …"; `crates/clax-server/tests/shell_boot.rs:71` `embeds_what_the_api_answers_and_never_the_token` |
| 8 | Time to usable | the "time to usable" gate (`web/perf/usable.perf.ts`, budgets in `web/perf/budget.json`) and the "web bundle size" gate (`web/scripts/bundle-size.mjs`, `web/perf/bundle-budget.json`); numbers in section 7 |
| 9 | Runtime bridge and capabilities | `web/e2e/capabilities.spec.ts:35` "use() resolves declared names asynchronously, frozen, and null for the rest", `:50` "permissions.request shows one dialog …"; `web/e2e/db.spec.ts:30` "a write in one tab reaches the other tab's onSnapshot …"; `crates/clax-server/tests/api_docs.rs:43` `put_get_patch_delete_round_trip_with_pins`, `:115` `levels_follow_the_token_the_viewer_name_and_as_level`; `crates/clax-mcp/tests/db.rs:63` `set_get_update_delete_with_version_pins` |
| 9 | Anchors and clips | `crates/clax-core/src/anchor.rs:305` `validation_rejects_bad_anchors`, `:461` `area_anchors_validate_their_fractions_and_need_a_selector`; `web/e2e/area.spec.ts:70` "a drag over an image draws an area, clipped to exactly the rectangle …"; `web/e2e/comments.spec.ts:67` "republish re-anchors kept elements and detaches removed ones"; `crates/clax-server/tests/api_threads.rs:20` `create_stores_the_clip_and_serves_it_sandboxed` |
| 10 | Comments and the feedback loop; delivery tiers | `crates/clax-server/tests/api_feedback.rs:29` `long_poll_wakes_on_a_send_not_at_its_deadline`, `:102` `tiers_gate_by_arming_and_resends_can_be_excluded`; `crates/clax-mcp/tests/comments.rs:136` `every_result_carries_pending_feedback_once`, `:159` `comments_read_summarises_threads_and_acknowledges_them`, `:226` `reply_and_resolve_follow_the_sent_rule`; `crates/clax-hooks/tests/golden.rs:292` `claude_session_lifecycle`, `:297` `codex_session_lifecycle`; `scripts/smoke-comment-loop.sh` (tiers 1, 2, 4, 5) |
| 10 | Codex wake path (tier 5) | `crates/clax-server/tests/api_push.rs:109` `exit_0_delivers_by_queue_with_the_payload_and_codex_home` (fake `codex`) |
| 10 | Notices (Grok's monitor) | `crates/clax-server/tests/api_notices.rs:49` `a_notice_points_at_the_comment_once_and_delivers_nothing`, `:125` `grok_push_reports_whether_a_follower_is_connected`; `crates/clax-cli/tests/follow.rs:263` `it_prints_one_line_per_new_comment_and_nothing_else` |
| 10 | Working, changelog, batch, attention, presence | see 4.2 |
| 11 | Sessions and identity | `crates/clax-server/tests/api_sessions.rs:26` `register_heartbeat_end_roundtrip`, `:121` `join_by_parent_pid_meets_the_shim_in_either_order`; `crates/clax-mcp/tests/shim.rs:224` `serves_the_tools_and_registers_the_harness_session` |
| 12 | MCP tool surface | `crates/clax-server/tests/api_mcp.rs:70` `mcp_lists_the_twenty_three_tools`; `crates/clax-mcp/tests/tools.rs:51` `publish_then_read_round_trips_the_page_and_files` (22 tests in the file) |
| 13 | Plugins | `scripts/test-plugins.sh` and `scripts/test-ensure-clax.sh` (gates); `crates/clax-cli/tests/init.rs:183`; Grok: `crates/clax-cli/tests/grok_dedupe.rs:364` `both_copies_enabled_one_acts_in_either_discovery_order`, `crates/clax-hooks/tests/golden.rs:839` `grok_stop_blocks_once_at_the_end_of_a_turn`; Claude Code channel: `crates/clax-mcp/tests/channel.rs:379` `forwards_one_channel_event_per_comment_when_launched_with_the_channel`; Pi: `plugins/pi/test/clax.test.ts` (48 tests, against a real daemon) |
| 14 | Security model | `crates/clax-server/tests/api_auth.rs:15` `token_is_refused_to_non_loopback_peers`; `api_host.rs:30` `api_routes_refuse_a_host_that_is_not_this_machine`; `shell_boot.rs:212` `hostile_titles_and_comments_stay_data`; `api_content.rs:113` `main_origin_content_is_sandboxed_but_artifact_origin_is_not`; `web/e2e/gesture.spec.ts` (27 test declarations, many parameterised), for example `:133` "a page that pulls focus cannot send to the agent …"; `web/e2e/echo-chrome.spec.ts:398` "a viewer who Tabs out of the page sends only with a click: no key clears the trail" |
| 15 | Error handling | `crates/clax-server/tests/api_timeout.rs:5` `slow_api_requests_time_out_with_json_408`; `api_artifacts.rs:65` |
| 17 | Phase 5: room and sample | `crates/clax-server/tests/api_room.rs:135` `presence_and_messages_reach_every_peer_with_sender_fields`; `api_sample.rs:126` `streams_text_then_done_under_the_framing_and_the_tier_model`; `web/e2e/room.spec.ts:13`; `web/e2e/sample.spec.ts:29` "consent once, progressive text, and the call count", `:96` "use("sample") resolves null …"; section 2.3 |

### 4.2 `docs/superpowers/specs/2026-10-01-echo-design.md`

| § | Section | Tests that pin it |
|---|---|---|
| 2 | The look: type, theme, keys, comment mode | `web/shell/src/echo-theme.test.ts:12` "self-hosts exactly three faces …"; `web/shell/src/view/theme-model.test.ts:7` "follows the system until flipped, and flipping back to the system's scheme follows it again" (Q2); `web/e2e/echo-chrome.spec.ts:31` "the first paint already has the theme …", `:49` "keys pressed in the page are the page's: C and ? do nothing in the shell" (Q6), `:65` "C and ? are the shell's only letter keys …"; `web/e2e/echo.spec.ts:9` "the top bar reads Echo, and comment mode shows the red-orange rule" |
| 2 | Easter eggs (Q9) | `web/e2e/echo.spec.ts:154` "the tenth version, viewed first in this browser, reads rally of 10 …"; `web/shell/src/echo-chrome.test.ts:10` "a playful mark's halves meet on one click …" |
| 3 | Screens: top bar, thread cards, gallery, people panel | `web/shell/src/topbar.test.ts:7`; `web/shell/src/sidebar.test.ts:34` "leads with the threads the latest version addressed …"; `web/shell/src/gallery.test.ts:70` "renders cards led by the version numeral, with title and link, and no description" (Q1); `web/e2e/echo.spec.ts:30` phone layout |
| 3 | Participants and attention (Q3, Q4, Q7) | `crates/clax-server/tests/api_attention.rs:66` `an_address_after_your_last_look_needs_your_eyes_until_you_look`; `crates/clax-core/src/mentions.rs:48` `mentions_match_whole_names_in_any_case_at_a_boundary`; `web/e2e/attention.spec.ts:23` "attention across viewers: addressed, new version, a mention, and looking clears" |
| 4 | The agent working signal | `crates/clax-server/tests/api_working.rs:38` `setting_needs_the_token_and_reading_does_not`; `api_working_auto.rs:48` `each_delivering_tier_marks_the_session_working_on_the_thread`; `crates/clax-hooks/tests/golden.rs:886` `grok_working_through_a_turn`; `scripts/test-tool-hook.sh` (the once-a-minute `PostToolUse` stamp); `web/e2e/working.spec.ts:33` "the summary, roster, marker, pin and gallery chip follow the working record", `:61` "a record lapses 2 minutes after its last renewal"; `web/shell/src/caps/comments.test.ts:74` "working() names only this document's own threads, by handle" |
| 5 | The version changelog | `crates/clax-server/tests/api_changelog.rs:33` `a_publish_links_the_threads_the_session_was_working_on_then_clears`; `crates/clax-core/src/store/changelog.rs:303` `a_seen_mark_never_passes_the_latest_version`; `web/e2e/changelog.spec.ts:21`, `:45`, `:64` |
| 6 | Batch send | `crates/clax-server/tests/api_batch.rs:44` `a_batch_is_delivered_as_one_group_led_by_its_note_through_every_tier`; `web/e2e/batch.spec.ts:20` "tick a shift range, send with a note, and the agent gets one delivery" |
| 7 | Where a send goes | `web/e2e/batch.spec.ts:46` "with two live agents the caret picks one, and only that agent gets the rows", `:63` "with no live agent Send goes without to, and the comment waits for the next session"; `crates/clax-server/tests/api_watches.rs:88` `untargeted_feedback_goes_to_the_next_publisher_and_session_end_releases` |
| 8 | Presence (Q5) | `crates/clax-server/tests/api_presence.rs:9` `presence_is_reported_by_viewers_announced_and_lapses`; `web/e2e/presence.spec.ts:9` |

### 4.3 Spot checks, run alone after the gate run

Rust, in the worktree:

```
for t in "clax-server api_batch" "clax-server api_working_auto" "clax-server api_changelog" \
         "clax-server api_presence" "clax-hooks golden" "clax-mcp channel" "clax-cli grok_dedupe"; do
  set -- $t; cargo test -q -p $1 --test $2 | grep '^test result'; done
```

```
## cargo test -p clax-server --test api_batch
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.14s
## cargo test -p clax-server --test api_working_auto
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.14s
## cargo test -p clax-server --test api_changelog
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s
## cargo test -p clax-server --test api_presence
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s
## cargo test -p clax-hooks --test golden
test result: ok. 20 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 13.15s
## cargo test -p clax-mcp --test channel
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 21.72s
## cargo test -p clax-cli --test grok_dedupe
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 6.78s
```

Playwright, in `web/`:

```
npx playwright test comment-loop.spec.ts working.spec.ts changelog.spec.ts \
  batch.spec.ts presence.spec.ts attention.spec.ts --reporter=line
```

```
  27 passed (51.8s)
exit=0
```

## 5. Unsigned commits

`git log --format=%G?` reports `N` for signed and unsigned commits alike here
(`git log -1 --format=%G? dcb105c` → `error: gpg.ssh.allowedSignersFile needs
to be configured …` then `N`, though `git cat-file commit dcb105c` carries a
`gpgsig` header), so signatures were read from the raw commit objects instead:

```
for c in $(git rev-list --reverse origin/main..main); do
  git cat-file commit $c | sed '/^$/q' | grep -q '^gpgsig' || echo $c; done
```

- `origin/main` is `3114d41`. `git rev-list --count origin/main..main` → `55`.
- **All 55 commits on `main` after `origin/main` have no `gpgsig` header.**
  The first is `32267c9` "Correct the Echo plan against the merged Svelte
  shell" (2026-10-01 19:23 -0400); the last is `f680986` "Security pass:
  pointer-only consequential actions, bounded tokenless state, quoted Grok
  monitor command" (2026-10-02 22:22 -0400). This includes 5 merge commits
  (`git rev-list --count --merges origin/main..main` → `5`).
- `origin/main` itself also carries unsigned commits. The same check over
  `git rev-list --reverse origin/main` (314 commits) gives runs of 242
  signed, 36 unsigned, 22 signed, 14 unsigned: `2e08cad` through `8fdf373`
  (36), and `95ed66a` through `3114d41` (14), the tip of `origin/main`.
  Signed copies of some commits in the first run exist beside them (for
  example `7595874` and `2e08cad` share a subject).
- In all, **105 of the 369 commits reachable from `main` are unsigned**
  (same check over `git rev-list main`). `docs/follow-ups.md:54-56` records
  only the first run.

## 6. Gaps, stated plainly

### 6.1 Verified only by fakes, or not at all

- **No real model session has run the comment loop**, in any harness
  (section 2.4). Every tier that needs a live harness is unproven live.
- **Grok Build: every CLI flag and behaviour is from Grok's source, checked
  only by a fake Grok** (`crates/clax-cli/tests/grok_dedupe.rs`, the Grok
  goldens in `crates/clax-hooks/tests/golden.rs:820-915`). Unproven live:
  `grok plugin install <dir> --trust` and `uninstall --confirm`,
  `plugin list --json`, `GROK_SESSION_ID` and the hook envelope, the Stop
  block and `stopHookActive`, two plugins' servers loading together,
  `/new` and `/resume`, the `SessionEnd` timeout, and the monitor wake
  (`docs/follow-ups.md:62-83`). Grok's tier 5 is marked "from source; not run
  live" (`docs/contract.md:1531`). `scripts/verify-harnesses.sh` does not
  cover Grok (`docs/follow-ups.md:106-108`). `grok` is not on this machine's
  `PATH` (`which grok` → `grok not found`).
- **Claude Code channels: tested only with a fake `claude`** that runs the
  shim over raw JSON-RPC (`crates/clax-mcp/tests/channel.rs`). That a channel
  event from `plugin:clax@clax` starts a turn, and that a background
  `clax feedback follow --once` exit wakes an idle session, are unverified
  (`docs/follow-ups.md:48-53`). Channels are a research preview and Clax is
  not on the allowlist (`docs/contract.md:2459-2468`).
- **The Pi model session has never run end to end**: no model provider key
  exists on the build machine (`docs/contract.md:2534-2536`,
  `docs/follow-ups.md:27-28`). Pi's tier 5 is "from source; not run live"
  (`docs/contract.md:1531`). Whether a real Pi resolves `typebox` from the
  installed copy, which has no `node_modules`, is unchecked
  (`docs/follow-ups.md:21-23`).
- **VoiceOver: never tried.** Whether VoiceOver's activation click has
  `detail` 1 (acts) or 0 (falls back to "Click to <verb>") decides whether a
  screen-reader viewer can Send, Resolve, Reply or Allow at all
  (`docs/follow-ups.md:40-47`).
- **Codex `PostToolUse` renewal of the working signal is not measured**
  (`docs/contract.md:1473`; `scripts/smoke-codex.sh --hooks` measures it).
- **The Codex plugin validator has never run.** `scripts/test-plugins.sh:398-406`
  prints `SKIP: …/validate_plugin.py not found; the Codex plugin validator was not run`
  on this machine (run alone: `bash scripts/test-plugins.sh | grep SKIP`), and
  the gate hides that line because the gate passed.
- **Install paths against real harnesses**: section 3.
- **The release workflow and `install.sh` from GitHub** have never run
  (`docs/follow-ups.md:31-39`); `actionlint` was not run (`docs/follow-ups.md:37`).
- **Anthropic `sample()` provider**: tested against a fake server only
  (section 2.3).
- **Spec §18 open items**: Safari's `*.localhost` behaviour (D5) and whether a
  later Codex delivers a held message to a re-attached `codex exec` thread
  (`docs/superpowers/specs/2026-09-28-clax-design.md:2206-2210`, `:2214-2218`).

### 6.2 Owner decisions still open

- **O1, sandbox the subdomain frame.** A subdomain-mode page has no
  `sandbox`, so with the viewer's activation it can navigate the whole window
  to another site or to a fresh Clax load (`docs/follow-ups.md:112-122`,
  `docs/contract.md:2491-2494`).
- **O2, a keyboard path for the pointer-only actions.** Keyboard-only and
  screen-reader viewers cannot Send, Resolve, Reply, batch send, Post in a
  page-opened composer, or Allow, because no key clears the tainted trail and
  every load starts tainted (`docs/follow-ups.md:123-128`,
  `docs/contract.md:2485-2490`). Together with the VoiceOver gap, this may
  leave screen-reader users with no way to send a comment at all.
- **O3, LAN write rate limits.** Anyone on the LAN who reaches a
  `--bind 0.0.0.0` daemon can create viewers, threads, comments, sends and
  `db` documents without limit; room sockets and event streams have no idle
  timeout or per-caller cap (`docs/follow-ups.md:129-131`,
  `docs/contract.md:2503-2507`).
- **A Grok tool-use renewal hook.** Grok has no `PostToolUse` hook, so a Grok
  session's working mark lapses after 2 minutes of tool calls that are not
  Clax calls (`docs/superpowers/specs/2026-09-28-clax-design.md:1826-1830`).
  The proposal is one hand-over for Claude Code and Grok together
  (`docs/follow-ups.md:101-106`, Q5).

### 6.3 Parked flaky tests

From `docs/follow-ups.md:200-226`. **Every one passed in this gate run**,
so none was rerun alone; one clean run is not evidence that they are fixed.

| Test | Cause recorded |
|---|---|
| `crates/clax-mcp/tests/open_status.rs:17` `opened_follows_the_openers_exit_status` | a 1.5 s wait for the fake browser opener |
| `web/e2e/gesture.spec.ts`, N14 sandbox closed-shadow-root `sendToClaude` | 120 s timeout under load; suspected fix `e5dfa42` awaits a loaded full gates run (this run was one, and it passed) |
| `crates/clax-cli/tests/cli.rs:1388` `a_daemon_started_without_port_uses_the_homes_serve_port` | reserves a port by binding 0 and releasing it, a real race |
| `web/shell/src/artifact.test.ts`, "says so when the page of an opened thread never greets" | a 50 ms wait raced against 120 ms sleeps |
| `web/e2e/subpages.spec.ts`, "sandbox: one link inside the frame is one history entry" | about 1 run in 120 under load |
| `crates/clax-hooks/tests/golden.rs:316` `no_daemon_prints_nothing_and_starts_none` | timing under load |
| `web/e2e/artifact.spec.ts`, "sandbox: a second viewer's vote 2 s after another viewer's vote reloaded it publishes" | reloaded frame missing within 30 s once |
| `web/e2e/echo-chrome.spec.ts:420`, "subdomain: the artifact deleted while the viewer types in it leaves the keyboard free …" | once took 6.2 min |

### 6.4 Skips

- 48 `web/e2e/shots.spec.ts` screenshot tests skip in every gate run by
  design (`web/e2e/shots.spec.ts:16`). The spec asks that every UI change be
  checked in screenshots, light and dark, desktop and phone
  (`docs/superpowers/specs/2026-09-28-clax-design.md:2095-2103`); that check
  is manual and was not repeated here.
- The Codex plugin validator (6.1).
- `scripts/test-install.sh:215` skips its `shasum` fallback where `shasum`
  exists, as on this machine; `scripts/test-dev.sh:310` skips its recipe
  cases without `just` (present here, so those ran).

### 6.5 Known issues and limitations

Open defects and limits recorded in `docs/follow-ups.md:110-198` and
`docs/contract.md:2420-2536`, none fixed by this run:

- The daemon compresses no response, which costs time to usable over a LAN
  (`docs/follow-ups.md:182-187`).
- The bridge says nothing while a page's scripts hold up the parse, and a
  lazy part that loads after failing stays failed until the page's next hello
  (`docs/follow-ups.md:188-198`).
- The daemon's own `/mcp` `status` does not report `upgrade_held`; one
  canonicalization in daemon replacement has no test; `clax doctor --agent`'s
  `binary` check probes differently from the wrapper, conflates four causes
  under "not a usable clax binary", can block in `version_line`, and
  shows a stale held-upgrade reason; wrapper probes plus daemon start can
  exceed Codex's MCP startup timeout; seven usability items in
  `scripts/verify-harnesses.sh` (`docs/follow-ups.md:133-180`).
- Comment mode cannot pick inside a nested `<iframe>`, which also renders
  blank in clips; CSS counters restart in region clips; a page shares its
  realm with the bridge, so in comment mode it can report a pick the viewer
  did not make (`docs/contract.md:2422-2426`, `:2500-2502`).
- What any viewer, including a tokenless LAN viewer, may read: every
  artifact, its threads, working lists with agent messages, presence and seen
  marks (`docs/contract.md:2508-2513`). Token holders are trusted with every
  working record (`:2514-2520`).
- A Grok sandbox profile blocks a daemon the shim starts; a Grok session's
  tier 5 needs the agent to start the monitor; `/new` and `/resume` in Grok are
  unmeasured (`docs/contract.md:2442-2458`).
- `just dev claude` sessions cannot use the channel and use the background
  fallback (`docs/contract.md:2472-2474`).

## 7. Bundle and performance numbers

Bundle sizes, from the gate run's build, printed by the bundle gate's own
script run again on that build (`cd web && node scripts/bundle-size.mjs`,
exit 0):

```
gzip bytes: gallery 26683, artifact 62854, eager bridge 5449, parts: comment 10773, clip 15004, caps 8144, room 3325, sample 3340; raw bytes: fonts 50144
```

Against `web/perf/bundle-budget.json`: gallery 26683 / 26748, artifact
62854 / 62854 (exactly at budget, no headroom), eager bridge 5449 / 5491,
comment 10773 / 11821, clip 15004 / 16504, caps 8144 / 8773, room 3325 /
3657, sample 3340 / 3674, fonts 50144 / 50144 (at budget).

Time to usable, from the gate run's `web/perf/results.json` (median of 9
samples, first attempt; no Playwright retry was needed), in milliseconds,
on `darwin-arm64` where budgets are enforced (`web/perf/budget.json`,
`"enforceTargets": true`):

| Metric | subdomain | budget | sandbox | budget |
|---|---|---|---|---|
| firstPaint | 57.9 | 81 | 57.8 | 75 |
| commentReady | 73.4 | 86 | 72.9 | 86 |
| framePaint | 45.7 | 76 | 45.7 | 75 |
| readyLatency | 24.0 | 63 | 24.8 | 62 |
| coldLatency | 8.0 | 38 | 7.1 | 38 |
| control | 40 | (44) | 40 | (44) |

These are loopback numbers on one machine. No LAN measurement exists, and the
daemon does not compress responses (6.5).

## 8. Clax in Chrome

The Chrome overlay (spec `docs/superpowers/specs/2026-10-05-chrome-overlay-design.md`,
main spec D19), on branch `chrome-overlay`. Machine as above: macOS 26.6.2,
arm64, Node v26.8.2; Playwright 1.63.0 with its Chromium 153.0.8010.12
(`channel: "chromium"`, new headless).

### 8.1 How a person sets it up

```
clax init                       # writes ~/.clax/extension and the native host
                                # manifests for Chrome, Chromium, Brave and Edge
# chrome://extensions → Developer mode → Load unpacked → ~/.clax/extension
clax extension status --json    # extension_id, files, launcher, each browser's manifest
```

### 8.2 What the browser test ran

Command: `cd web && npm run build && npx playwright test e2e/chrome-overlay.spec.ts`
(it also runs inside the web e2e lane of `scripts/quality_gates.sh`). Each
test has its own daemon (scratch `CLAX_HOME`, port 0), its own Vite dev
server on a copy of `web/e2e/live-site/`, and its own Chromium profile.
`clax extension install` registers the native host into that profile's
`NativeMessagingHosts` (`CLAX_NATIVE_HOST_DIRS=chromium=<profile>/NativeMessagingHosts`),
the test build of the extension replaces the release files in
`<home>/extension`, and Chromium loads it from there. Chromium does read the
host manifest from a profile given as `--user-data-dir`, and runs the
installed `host/launch.sh` → `host/ensure-clax.sh` → `clax native-host`
(the `bin` setting names this run's binary): the fallback in the plan (pairing
through the test hook) was not needed. Seven tests, all passing:

1. **The whole loop.** Chromium's own ID for `<home>/extension` equals the
   daemon's (`GET /api/extension`) and the CLI's (`clax extension status
   --json`). An agent session watches the dev server URL. The worker's hook
   (standing in for the toolbar icon) turns comment mode on; a page script
   finds no shadow root; a click on the page's button opens the composer
   frame; the post stores the thread with its clip and a snapshot that has
   the button and no `<script>`, `onload`, password value or hidden input.
   Chrome's own side panel (opened by `chrome.sidePanel.open` under a real
   click in an extension page, then driven over CDP, as Playwright does not
   list it) shows the thread and sends it; the session's feedback carries
   `live page <url>` and `Snapshot:`. Editing `main.js` makes Vite hot-update
   the page: the pin follows the relabelled button, then the thread goes to
   Detached when the button is removed. The agent's `addressed: true` reply
   shows in the panel, and the overlay's quiet snapshot links it. The shell's
   gallery shows the Live chip, the shell's viewer is the extension's owner
   viewer, and the snapshot view shows the thread with Comment disabled.
   The Vite update is a hot one (a `window` marker set before the edit
   survives it). The comment's POST passes the gateway, which refuses a
   write without `Origin`, so Chrome sends `Origin` on it. Reported, not
   judged, on the implementer's runs: icon → comment mode on 9–33 ms; pick
   → composer 142–253 ms (a review run printed 42 ms and 354 ms).
2. **Daemon restarted on another port.** The pairing names the old port; the
   panel shows `daemon_unreachable` with Retry; Retry pairs again with the new
   daemon and the error clears.
3. **Browser restart.** With the origin enabled and a thread on the page,
   Chromium is closed and started on the same profile; the registered loader
   greets the worker with no gesture, and the overlay comes back and resolves
   the thread's anchor, comment mode off.
4. With only the dev server's origin held (the release build's state once a
   person allows a site; the test build with `host_permissions` narrowed):
   a `history.pushState` route change keeps the overlay and comment mode
   (Chrome reports it to `tabs.onUpdated` as `loading`, then `complete`); a
   page's MutationObserver sees the host hide for the screenshot and show
   again; the composer takes focus (the page sees `blur` and
   `document.activeElement` = `CLAX-OVERLAY`, and typed keys reach the
   composer); `window.frames.length` is 0; the post has no clip
   (`captureVisibleTab` needs `activeTab` or `<all_urls>`, spec L8); and the
   page's own iframe of `chrome-extension://<ID>/composer.html` is refused
   (`chrome-error://`).
5. A composer frame loaded a second time (a navigation over CDP: the page
   cannot reach the frame) is closed, its pick cancelled, nothing posted,
   and comment mode stays on.
6. Two toggles at once (the worker's hook called twice concurrently) leave
   one pins host in the page: the isolated world's flag starts the overlay
   once, whether one injection or both ran `overlay.js` (which of the two
   happened is not observed).
7. Under a page's modal `<dialog>`, a pick inside the dialog opens the
   composer, which has focus and takes the keys, and its submit shortcut
   posts; the dialog's backdrop is on top at the composer's centre
   (`elementFromPoint` → `DIALOG`), so its buttons cannot be clicked while
   the dialog is open (spec §10.4).

Found by this test and fixed, each with a regression test:

- Chrome sends no `Origin` on the worker's GETs when the extension holds a
  host permission for the daemon's origin (`<all_urls>`, or "On all sites"),
  marking them `Sec-Fetch-Site: none`; the gateway refused them all with
  `forbidden_origin`, so no page could be looked up. The gateway now counts
  a GET with the credential, no `Origin` and `Sec-Fetch-Site: none` as the
  extension's (provisional, pending the owner's confirmation; writes
  without `Origin` stay refused) (`crates/clax-server/tests/api_gateway.rs`,
  `a_credential_without_origin_is_the_extensions_only_when_the_browser_says_none`).
- Registered loaders were gone after the browser restart (Chromium dropped
  the registered content script across a restart with `--load-extension`;
  whether Chrome does the same on an update or reload was not checked),
  while the origin record stayed; the worker now registers them again at
  each start
  (`web/extension/src/sw/origins.test.ts`, and test 3 above).
- Retry after `daemon_unreachable` did not pair again, so within 10 s of the
  last pairing a daemon restart could not be recovered from the panel;
  Retry now pairs again for it (`web/extension/src/sw/panel.test.ts`, and
  test 2 above).

### 8.3 Not exercised; only a person can check

- The toolbar icon, its permission prompt and the per-origin grant (the
  test build holds `<all_urls>` or a fixed origin; the worker's test hook
  stands in for the click). `activeTab` lapsing on navigation.
- The side panel opened by the icon (the test opens Chrome's real side
  panel, but through `chrome.sidePanel.open` from an extension page).
- Alt+Shift+C and the page's context menu entry.
- Chrome stable, Brave and Edge: host registration on macOS and Linux, and
  Load unpacked of `~/.clax/extension` by a person (only Playwright's
  Chromium ran, on macOS).
- An agent in a real Claude Code or Codex session watching a dev server
  through the MCP `watch` tool (the test registers the session and its watch
  over HTTP).
- The self-reload after `clax init` installs a newer extension, and the
  overlay's re-injection beside an orphaned one after an extension reload:
  in Playwright's Chromium, `chrome.runtime.reload()` of an extension loaded
  with `--load-extension` unloads it (its pages answer
  `ERR_BLOCKED_BY_CLIENT` and no worker starts), with or without
  `--disable-extensions-except`, and with `Extensions.loadUnpacked` the
  worker never started; so neither was run.
- The shell and the side panel sharing the owner's marks by hand (read a
  thread in one; it is not new in the other).
- Pins found by hit testing, and the cursor, as spec §10.4 says a page can
  observe them (not measured).
- Stacking under a page's popover or non-modal top-layer element opened
  after the overlay (only a modal dialog was tried).
- Two injections racing for certain: test 6 shows one overlay after two
  concurrent toggles, but not that both injections reached the page.

### 8.4 The owner's steps outside Clax

Signing is local, owner-run and approval-gated (spec L15, §6.7). The private
key lives only in the owner's 1Password; Clax, its agents, its builds and CI
never read it, and both scripts refuse to run when `CI` is set. Only the
owner does these steps.

1. Create the key in 1Password without writing it to disk, for example
   `openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 | op document create - --title "Clax extension key" --file-name key.pem`
   (or generate it in the 1Password app).
2. Set `CLAX_EXTENSION_KEY_REF` to its full secret reference (1Password's
   Copy Secret Reference on the key gives it, for example
   `op://<vault>/<item>/key.pem`), in the owner's shell only; no vault or
   item name goes into the repository. `op read "$CLAX_EXTENSION_KEY_REF"`
   must print the PEM key.
3. Run `scripts/extension-pubkey.sh`. It reads the key once (1Password asks
   for approval), writes its public half to `web/extension/key/key.pub.b64`
   and prints the extension ID. Commit that file. The extension's ID then
   becomes the key's, once: run `clax init`, which writes the new native-host
   registration, and each person loads `~/.clax/extension` unpacked again.
4. For the Chrome Web Store listing's first upload only, run
   `scripts/pack-extension.sh --first-upload`. It builds the release
   extension and writes a zip holding the build and the key as `key.pem`,
   which fixes the listed ID to the committed key's. That zip goes to a new
   owner-only directory under `$TMPDIR`, outside the repository; delete it
   once uploaded. Every later release is `scripts/pack-extension.sh`, which
   writes `dist/clax-extension-<version>.zip` without the key (the Web Store
   re-signs each release itself). `--crx` also writes a signed
   `dist/clax-extension-<version>.crx` with Chromium (`CLAX_CHROMIUM` names
   the binary).
5. Upload the zip by hand in the Chrome Web Store developer dashboard.

`scripts/pack-extension.sh` refuses a build whose manifest `key` is not the
1Password key's public half, so a zip never carries a key other than the
committed one. The key passes through a pipe (`extension-pubkey.sh`) or a
mode-0600 temporary file removed on exit, interrupt or termination
(`pack-extension.sh`).

`scripts/test-extension-signing.sh` (scripts lane of
`scripts/quality_gates.sh`) tests both scripts with a fake `op` serving a
throwaway key: the public key and the ID it prints (checked against the rule
`crates/clax-core/src/extension.rs` pins), exit 2 without
`CLAX_EXTENSION_KEY_REF` or under `CI` with no `op` call, the zip without
and with `key.pem`, a mismatched manifest key, no copy of the key left in
the repository or `$TMPDIR` (also after SIGINT mid-run) and none printed,
and a `.crx` signed with the key when a Chromium is found (skipped under
`CI`). No gate calls 1Password.

## Appendix A: the browser-and-shim loop script

The script behind section 2.2, kept here so the run can be repeated. Save it
as `web/loop-verify.mjs` in a scratch checkout (it imports `@playwright/test`
from `web/node_modules`), build with `cargo build -p clax-cli` and the web UI
with `just web`, then run
`node loop-verify.mjs <checkout>/target/debug/clax <scratch-dir>`. It never
touches `~/.clax` and binds an ephemeral port.

```js
import { spawn, execFileSync } from "node:child_process";
import { readFileSync, existsSync, mkdtempSync } from "node:fs";
import { createInterface } from "node:readline";
import { join } from "node:path";
import { chromium } from "@playwright/test";

const BIN = process.argv[2];
const scratch = mkdtempSync(join(process.argv[3], "clax-verify-"));
const HOME = join(scratch, "home");
const env = { ...process.env, CLAX_HOME: HOME, CLAX_NO_OPEN: "1", CLAX_CODEX_BIN: "", CLAUDE_CODE_SESSION_ID: "verify-loop-1", RUST_LOG: "error" };
delete env.CLAUDE_PROJECT_DIR; delete env.CLAX_SESSION_ID;
const ok = (m) => console.log(`PASS: ${m}`);
const die = (m) => { console.log(`FAIL: ${m}`); cleanup(); process.exit(1); };

const shim = spawn(BIN, ["--port", "0", "mcp", "--agent", "claude"], { env, cwd: scratch, stdio: ["pipe", "pipe", "ignore"] });
const lines = createInterface({ input: shim.stdout });
const waiters = new Map();
lines.on("line", (l) => { const m = JSON.parse(l); if (m.id && waiters.has(m.id)) { waiters.get(m.id)(m); waiters.delete(m.id); } });
let n = 0;
const rpc = (method, params) => new Promise((res) => { const id = ++n; waiters.set(id, res); shim.stdin.write(JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n"); });
async function call(name, args) {
  const m = await rpc("tools/call", { name, arguments: args });
  if (m.error || m.result.isError) die(`${name}: ${JSON.stringify(m.error ?? m.result)}`);
  const t = m.result.content.filter((c) => c.type === "text").map((c) => c.text);
  return [JSON.parse(t[0]), t[1] ?? null];
}
function cleanup() { try { shim.stdin.end(); } catch {} try { execFileSync(BIN, ["stop"], { env }); } catch {} }

await rpc("initialize", { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "verify", version: "0" } });
shim.stdin.write(JSON.stringify({ jsonrpc: "2.0", method: "notifications/initialized" }) + "\n");

const [pub] = await call("publish", { html: "<title>Quarterly Review</title><main><h2>Quarterly goals</h2><ul><li>Ship</li><li>Grow</li><li>Drop this</li></ul></main>" });
const info = JSON.parse(readFileSync(join(HOME, "daemon.json"), "utf8"));
ok(`published through the stdio shim: ${pub.url} (v${pub.version}); daemon port ${info.port}, CLAX_HOME ${HOME}`);
const [st] = await call("status", {});
ok(`status names the session: harness=${st.session?.harness ?? JSON.stringify(st.session)}`);

const browser = await chromium.launch();
const page = await browser.newPage();
await page.goto(pub.url);
await page.getByRole("button", { name: "People and agents" }).click();
const field = page.getByRole("dialog", { name: "People and agents" }).getByLabel("Your name");
await field.fill("Alex"); await field.press("Enter"); await page.keyboard.press("Escape");
const re = new RegExp(`(${pub.artifact_id}\\.localhost:\\d+/v/1/|/c/${pub.artifact_id}/v/1/)$`);
for (let i = 0; i < 100 && !page.frame({ url: re }); i++) await page.waitForTimeout(100);
const frame = page.frame({ url: re }) ?? die("content frame never loaded");
ok(`browser loaded the shell and the content frame at ${frame.url()}`);
await page.getByRole("button", { name: "Comment", exact: true }).click();
await frame.locator("h2").hover();
await frame.locator("clax-overlay .o").waitFor({ state: "visible" });
await frame.locator("h2").click();
const composer = page.locator(".composer");
await composer.locator("img.clip").waitFor({ state: "visible" });
await composer.locator("textarea").fill("Make this a two-column layout and drop the third bullet.");
await composer.getByRole("button", { name: "Post comment" }).click();
const card = page.locator(".section-open .thread-card").first();
const sendBtn = card.getByRole("button", { name: /^Send to / });
const sendLabel = await sendBtn.innerText();
await sendBtn.click();
await card.locator(".waiting").waitFor({ state: "visible" });
const tid = await card.getAttribute("data-thread");
ok(`browser: picked the h2 in comment mode, posted with a clip, pressed "${sendLabel.trim()}"; card reads "${(await card.locator(".waiting").innerText()).trim()}"`);

const [listed, trailing] = await call("list", {});
if (!trailing || listed.feedback?.length !== 1 || listed.feedback[0].thread_id !== tid) die(`no piggyback: ${JSON.stringify(listed.feedback)} ${trailing}`);
console.log(trailing.split("\n").map((l) => "  | " + l).join("\n"));
const clip = listed.feedback[0].clip_path;
if (!existsSync(clip) || readFileSync(clip).subarray(0, 4).toString("hex") !== "89504e47") die(`clip ${clip}`);
ok(`agent session: the next tool result (list) carried thread ${tid} with author Alex and a PNG clip from the browser's capture`);
await card.locator(".st.ag").filter({ hasText: "is working on it" }).waitFor({ timeout: 20000 });
ok(`browser: the card shows "${(await card.locator(".st.ag").innerText()).trim()}"`);

const [read] = await call("comments_read", { url_or_id: pub.artifact_id, thread_id: tid });
ok(`comments_read returned the thread (${JSON.stringify(read).length} bytes)`);
await call("comments_reply", { url_or_id: pub.artifact_id, thread_id: tid, text: "Done: two columns, third bullet removed." });
await card.locator(".msg.agent").filter({ hasText: "Done: two columns" }).waitFor({ timeout: 20000 });
ok(`browser: the agent's reply appeared live, authored "${(await card.locator(".msg.agent .author").first().innerText()).trim()}"`);
await call("comments_resolve", { url_or_id: pub.artifact_id, thread_id: tid });
const done = page.locator(".section-resolved .thread-card");
await done.filter({ hasText: "Done: two columns" }).waitFor({ timeout: 20000 });
ok(`browser: the thread moved to Resolved; history reads "${(await done.locator(".hist").innerText()).trim().replace(/\s+/g, " ")}"`);
const t = await (await fetch(`http://127.0.0.1:${info.port}/api/artifacts/${pub.artifact_id}/threads?include_resolved=true`)).json();
const th = (t.threads ?? t).find((x) => x.id === tid);
ok(`daemon: thread status=${th.status}, feedback_state=${JSON.stringify(th.feedback_state)}`);

await browser.close();
cleanup();
await new Promise((r) => shim.on("exit", r));
let alive = true; try { process.kill(info.pid, 0); } catch { alive = false; }
for (let i = 0; i < 50 && alive; i++) { await new Promise((r) => setTimeout(r, 100)); try { process.kill(info.pid, 0); } catch { alive = false; } }
if (alive) die(`daemon pid ${info.pid} still running`);
ok(`shim exited, daemon pid ${info.pid} stopped`);
console.log("loop verify passed");
```
