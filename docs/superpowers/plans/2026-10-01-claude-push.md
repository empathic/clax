# Claude Code Push Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Claude Code gets delivery tier 5. A comment sent to an idle Claude Code session wakes it. There are two paths. The first is a Claude Code channel: the `clax mcp` shim declares `claude/channel` and sends one `notifications/claude/channel` event per comment, when the session was launched with `claude --dangerously-load-development-channels plugin:clax@clax`. The second, for every other session, is a fallback: the skill has Claude run `clax feedback follow --once` as a background Bash command after it publishes. The command exits when a comment arrives, and Claude Code wakes the session when a background command exits. Both paths send a notice, which points at the comment. Neither delivers it. The comment still arrives exactly once, through tier 1, 2 or 4.

**Architecture:** The notices pipeline comes from the Grok plan (`docs/superpowers/plans/2026-10-01-grok-build.md`, Task 6): `feedback.notified_at`, `Store::take_notices`, `GET /api/sessions/<sid>/notices`, the follower registry, and `clax feedback follow`. This plan reuses them unchanged and adds one flag, `--once`. If the Grok plan has not run yet, Task 2 builds those pieces exactly as that plan specifies them, so whichever plan runs second only consumes them. In the shim, a new module, `crates/clax-mcp/src/channel.rs`, does three things under `--agent claude`. It declares `capabilities.experimental["claude/channel"] = {}` with channel `instructions`. It caps the protocol revisions it supports at `2025-11-25`, so negotiation never reaches `2026-07-28`, a revision under which Claude Code does not register a channel. It reads its parent's command line (the `claude` process) to learn whether a launch flag names `plugin:clax@<marketplace>`. Claude Code sends a server no registration signal, so the shim can learn no more than that. Only when the flag is present does the shim long-poll the notices route and forward each notice as a channel event. Otherwise it leaves the notices to `clax feedback follow`. `status` reports the channel state, and so does a new `channel` check in `clax doctor --agent claude`. The plugin manifest gains `"channels": [{"server": "clax"}]`. Permission relay (`claude/channel/permission`) is never declared.

**Tech Stack:** Rust 2024 (rmcp 3.5: `ServerCapabilities.experimental`, `CustomNotification`, `ServerHandler::supported_protocol_versions`, `ProtocolVersion::known_up_to`; axum 0.8, clap 4, serde), Bash 3.2-compatible shell, Python 3 (`scripts/test-plugins.sh`'s checks). The channel contract comes from code.claude.com/docs/en/channels and /channels-reference, as summarised in `.superpowers/sdd/2026-10-01-claude-push/decisions.md`. Tests use fake MCP clients and a fake `claude` only.

**Spec:** `docs/superpowers/specs/2026-09-28-clax-design.md`. That is the design spec's name on main since the Svelte merge renamed it with the product; it was written under the product's first name, which the brief uses. Task 1 amends §2 (a new decision), §10, §12, §13, §16 and §18, and `docs/contract.md`. The owner's decision, the channel facts and the rulings are in `.superpowers/sdd/2026-10-01-claude-push/decisions.md`.

**Precondition:** `git status --short -- docs/contract.md docs/superpowers/specs docs/follow-ups.md README.md plugins/claude-code scripts crates/clax-mcp crates/clax-cli/src/commands crates/clax-cli/src/main.rs crates/clax-cli/src/hooklog.rs crates/clax-server/src crates/clax-core/src/store crates/clax-core/src/feedback.rs` prints nothing. If anything shows up, stop and ask the controller. Do not stash, discard or commit someone else's changes.

## Global Constraints

- Rust edition 2024. `cargo clippy --workspace --all-targets -- -D warnings` and `RUSTFLAGS=-Dwarnings cargo check --workspace` pass.
- **Agents stage only. The controller commits.** Stage with `git add` and explicit paths, never `git add -A` or `git add .`. Never run `git commit`, `git stash`, `git reset` or `git checkout -- <path>`. Each task ends with its files staged and a proposed commit message in the report. The controller commits with plain `git commit`, which signs; never `--no-gpg-sign`.
- **Never bind or connect to port 7480 or 7481.** Tests start daemons with `--port 0`, or discover a daemon that a test started with `--port 0` in a scratch `CLAX_HOME`.
- **Never read, write or delete a real home:** `~/.clax`, `~/.clax-dev`, `~/.claude`, `~/.codex`, `~/.grok`, `~/.pi`, `~/.cargo/bin/clax`, `~/.local/bin/clax`. Every test sets `HOME`, `CLAX_HOME` and `CLAUDE_CONFIG_DIR` to scratch directories.
- **Agents never run real `claude`** (nor `codex`, `grok`, `pi`). The fake `claude` in Task 3 is a script in a temporary directory, always run by its absolute path, never found through `PATH`. `scripts/smoke-claude-push.sh` is for the owner to run. Agents write it, `bash -n` it and `shellcheck` it, but never run it.
- **Tests clear the harness environment they inherit.** Agents run inside Claude Code, which sets `CLAUDE_PID`, `CLAUDE_CODE_SESSION_ID`, `CLAUDE_PLUGIN_ROOT` and `CLAUDE_PROJECT_DIR`. Every test that runs `clax mcp`, `clax hook` or `clax feedback follow` removes those and `CLAX_SESSION_ID`, `GROK_SESSION_ID`, `GROK_HOOK_EVENT` and `GROK_PLUGIN_ROOT`, then sets only what the case needs.
- No test reaches the network beyond `127.0.0.1`.
- In prose, comments, doc comments and commit messages, write "ID", never "id", except as a literal symbol in code (`thread_id`, `"id"`).
- Doc comments and commit messages describe the contract or the change. They never describe the conversation that led to it, the history of a name, or how good the work is.
- The product's previous name must not appear literally in any file this plan creates or edits (the name gate in `scripts/test-plugins.sh`). The one exception is the spec's own path, which this plan does not change.
- **Never declare `claude/channel/permission`**, under any name, flag or configuration. People who comment must never be able to approve tool use. Task 3 asserts that the experimental capabilities are exactly `{"claude/channel": {}}`. Task 4 adds a gate that fails if the string `claude/channel/permission` appears anywhere under `crates/` or `plugins/`.
- **A notice is never a delivery.** Nothing in this plan sets `delivered_at` or `acknowledged_at`, or calls `take_feedback`, on the notice path.
- `scripts/ensure-clax.sh` and its plugin copies stay byte-identical. This plan does not edit them.
- Every task ends with `bash scripts/quality_gates.sh; echo "exit=$?"` printing `exit=0`. Check the status itself, not only the last line.

## Review Focus

1. **Exactly-once delivery is unchanged.** A channel event and a follow line are notices. Each comment is announced at most once per target session across both (`notified_at`). It is never announced after a tier has delivered it. It is delivered once, by tier 1, 2 or 4. Tests: `crates/clax-mcp/tests/channel.rs` (`a_comment_is_announced_once_across_channel_and_follow`, `a_channel_notice_delivers_nothing`), plus Task 2's store and route tests.
2. **No notice is spent on a session that cannot receive it.** The shim forwards notices only when its parent's command line names a Clax channel entry. Otherwise it never polls the notices route, so `clax feedback follow` still gets every notice. Test: `sends_nothing_without_the_launch_flag` (the follow command announces the comment afterwards).
3. **The shim tells the truth about the channel.** Claude Code has no registration signal. `status` reports `registered: null` and says why, reports the launch flag it saw (`present`, `absent` or `unknown`), and gives the launch command. It never says the channel works. Tests: `channel::tests` and `status_reports_the_channel_state`.
4. **Negotiation stays channel-capable.** Under `--agent claude` the shim supports revisions up to `2025-11-25`. A client asking for `2026-07-28` gets `2025-11-25`, and `server/discover` does not list `2026-07-28`. The Codex shim and the daemon's `/mcp` are unchanged. Test: `a_2026_07_28_client_gets_a_channel_capable_revision`.
5. **No permission relay.** The experimental capabilities are exactly `{"claude/channel": {}}`, and a gate forbids the relay capability's name in the tree.
6. **Notices pause inside `wait_for_feedback`.** While the session waits, no channel event is sent and no follow line is printed, and the wait returns the comment. Test: `no_channel_event_while_waiting_for_feedback`.
7. **Meta keys are identifiers.** Every key of an event's `meta` matches `^[A-Za-z0-9_]+$`, and `content` is the follow line, never the comment text. Tests: `channel::tests::an_event_is_the_follow_line_with_identifier_meta_keys` and the end-to-end case.

---

## Design decisions

### The two wake paths

| Session launched with | What wakes it | Who polls `/notices` | `status.push.tier` |
|---|---|---|---|
| `--dangerously-load-development-channels plugin:clax@<m>` (or `--channels plugin:clax@<m>` with an org allowlist entry) | a `notifications/claude/channel` event, which Claude Code turns into a `<channel source="plugin:clax:clax" …>` message that starts a turn when idle, or joins the next turn when busy | the shim | `"channel"` |
| anything else | the exit of a background `clax feedback follow --once`, which the skill starts after a publish | `clax feedback follow` | `"follow"` while one runs, else `null` with `follow_command` |

In both cases the message is the line that `clax_core::feedback::render_notice` produces:

> `[clax] New comment on "<title>" (<url>), thread <thread ID>. Call comments_read with url_or_id "<artifact ID>" and thread_id "<thread ID>" to read it; if you have already handled it, do nothing.`

A channel event's `meta` is `{artifact_id, thread_id, comment_id}`, all strings. Claude Code drops a key that is not an identifier without telling the server, so the keys use only letters, digits and underscores.

### Why the shim reads its parent's command line

Claude Code gives a channel server no acknowledgement and no registration signal. When the session did not load the server as a channel, or when policy blocks it, Claude Code "drops the events silently and returns no error to your server" (channels-reference, Notification format). It does not report the client's channel state in `initialize`, and it sets no environment variable for it. The only evidence a server can reach is how `claude` was launched. `ensure-clax.sh` execs `clax mcp`, so the shim's parent is the `claude` process. On Linux the shim reads `/proc/<ppid>/cmdline`. On macOS it runs `ps -ww -o args= -p <ppid>` and splits on whitespace, which is safe because channel entries contain no spaces. Each value after `--dangerously-load-development-channels` or `--channels` (also in the `--flag=value` form), up to the next option, is a channel entry. An entry `plugin:clax@<marketplace>` makes the launch flag `present`.

The flag being present is necessary, not sufficient. `channelsEnabled` being off (claude.ai Team and Enterprise until an Owner turns it on), a `--channels` entry that no allowlist covers, or claude.ai or Console authentication being missing can each still block registration. Claude Code reports these on its own startup screen. `status` therefore reports `registered: null`, with a note naming the startup screen as the place to look.

When the flag is `present`, the shim forwards notices. If registration was in fact blocked, those notices are lost as wake-ups: the shim stamps `notified_at`, Claude Code drops the event, and the follow fallback will not announce the comment again. The comment itself is not lost. Tiers 1, 2 and 4 still deliver it. This is a known limitation, with a remedy: relaunch without the flag. When the flag is `absent` or `unknown`, the shim never polls, so `clax feedback follow` sees every notice.

### Protocol revision

rmcp 3.5 supports every known revision by default, `2026-07-28` included, and advertises that one through `server/discover`. The docs say that under `MCP_PROTOCOL_NEGOTIATION=auto`, Claude Code does not register a channel server that negotiates `2026-07-28`. The Claude shim therefore overrides `ServerHandler::supported_protocol_versions` to `ProtocolVersion::known_up_to(&ProtocolVersion::V_2025_11_25)`. rmcp then answers an `initialize` that names `2026-07-28` with `2025-11-25`, and `server/discover` lists nothing newer. The cap applies only to a `ClaxTools` with a channel, which is the shim under `--agent claude`.

### Accounting

- `take_notices` (Grok plan, Task 6) is the only source of notices. It stamps `notified_at` with `UPDATE … WHERE notified_at IS NULL AND delivered_at IS NULL`, so the shim's loop and any number of `clax feedback follow` processes announce each row at most once between them.
- The notices route answers empty while the session is inside `wait_for_feedback`, so neither path announces then. The wait delivers instead.
- A notice can reach a busy session after the Stop hook has already delivered the same comment. Claude Code queues channel events and background-command completions until the turn ends. The line ends `if you have already handled it, do nothing`.
- The daemon's `push` for a Claude Code session says whether a notice follower is connected (`{"tier": "notice", …}`). The daemon cannot tell the shim's poll from a follow command's poll, and it does not need to. The shim refines the tier to `"channel"` or `"follow"` in `status`.

### The fallback in the skill

After its first publish in a session (publishing arms a watch), Claude calls `status`. If `push.tier` is `"channel"` or `"follow"`, it does nothing. Otherwise it runs `push.follow_command` with the Bash tool and `run_in_background: true`. That command is `"<binary.path>" feedback follow --once --agent claude --harness-session <session.harness_session_id>`, already shell-quoted. Claude Code re-invokes the model when a background command exits, including in an idle session. The output then holds one or more notice lines, or nothing when the session ended. Claude handles each comment through `comments_read`, then starts the same command again in the background. It keeps one running at a time, and stops restarting it when the person says to stop watching.

## File Structure

| Path | Responsibility |
|---|---|
| `crates/clax-core/src/store/migrations.rs`, `store/feedback.rs`, `feedback.rs` | `notified_at`, `take_notices`, `Notice`, `render_notice` (Grok plan, Task 6; built here only if absent) |
| `crates/clax-server/src/feedback.rs`, `state.rs`, `routes/feedback.rs`, `routes/mod.rs` | `Followers`, `GET /api/sessions/<sid>/notices` (same) |
| `crates/clax-server/src/routes/sessions.rs` | `push_info(…, following)`; Claude Code's `push` reports a notice follower |
| `crates/clax-cli/src/commands/feedback.rs` | `clax feedback follow` (same), plus `--once` |
| `crates/clax-mcp/src/channel.rs` (new) | Launch-flag detection, channel state, event params, the status overlay, the instructions |
| `crates/clax-mcp/src/client.rs` | `DaemonClient::notices` |
| `crates/clax-mcp/src/shim.rs` | The notice-forwarding loop; `run` takes the channel state |
| `crates/clax-mcp/src/tools.rs` | `with_channel`; capability, instructions, protocol cap; `status` overlay |
| `crates/clax-cli/src/commands/mcp.rs` | Detects the channel state under `--agent claude`; logs a `channel` line |
| `crates/clax-cli/src/commands/doctor_agent.rs` | The `channel` check for `--agent claude` |
| `plugins/claude-code/.claude-plugin/plugin.json` | `"channels": [{"server": "clax"}]` |
| `plugins/claude-code/skills/clax/SKILL.md`, `plugins/claude-code/README.md` | The wake paths and the fallback |
| `scripts/test-plugins.sh` | The manifest's `channels`; the no-relay gate |
| `crates/clax-mcp/tests/channel.rs` (new) | A fake `claude` drives the shim end to end |
| `scripts/smoke-claude-push.sh` (new) | The owner's live check against real `claude` |

---

### Task 1: Spec and contract amendments

**Files:**
- Modify: `docs/superpowers/specs/2026-09-28-clax-design.md` (§2, §10, §12, §13, §16, §18)
- Modify: `docs/contract.md` (status, Delivery tiers per harness, `clax doctor --agent`, Known limitations, and a new "Notices" subsection if the Grok plan has not added one)

**Interfaces:**
- Consumes: nothing.
- Produces: the contract that Tasks 2 to 5 implement.

- [ ] **Step 1: Find out what the Grok plan has already written**

Run `grep -n "^### Notices" docs/superpowers/specs/2026-09-28-clax-design.md docs/contract.md` and `grep -n "^| D1[78] " docs/superpowers/specs/2026-09-28-clax-design.md`. The new decision takes the next free number: D17 when no D17 row exists, else D18. If a Notices subsection exists (the Grok plan's "Notices (Grok's monitor)"), extend it as Step 3 says. Otherwise add the harness-neutral one given there.

- [ ] **Step 2: §2 Decisions: one new row**

```markdown
| D17 | Claude Code's tier 5 is a notice, not a delivery: a Claude Code channel event from the shim when the session was launched with the channel (`--dangerously-load-development-channels plugin:clax@<marketplace>`), else a background `clax feedback follow --once` that the skill starts after a publish. The shim never declares permission relay | Channels are the only way a third-party plugin can start a turn in an idle Claude Code session, but they are a research preview behind a launch flag. The background command works in every session. A notice that points at a comment cannot double-deliver it. Commenters must never approve tool use |
```

(Use D18 if D17 is taken.)

- [ ] **Step 3: §10 Comments and the feedback loop**

In the tier table, replace the Claude Code clause of row 5, `Claude Code: none available to third-party plugins.`, with:

```markdown
Claude Code: a notice, through a channel (opt-in launch flag, research preview) or the follow fallback (below).
```

In the same row's Latency cell, append `; Claude Code: at once when idle, with the next turn when busy (both paths)`. In its Failure modes cell, append `Claude Code: a channel that the launch flag names but policy blocks drops its events silently, and the comment then waits for tiers 1, 2 and 4.`

If no Notices subsection exists, add this one after "### Delivery and acknowledgement". If the Grok plan's subsection exists, keep its text and append the last two paragraphs below to it.

```markdown
### Notices

A notice tells an agent session that a comment was sent to it, and names
the artifact and thread. It never carries the comment. `GET
/api/sessions/<sid>/notices` returns the session's rows that no tier has
delivered and no follower has announced, on watches with replies armed,
and stamps `notified_at` on each. A row is announced at most once per
target session, however many followers poll, and never after a tier has
delivered it. Retargeting a row clears `notified_at`. While the session is
inside `wait_for_feedback`, the route answers empty at once. A notice is
not a delivery: the row still waits for tiers 1, 2 and 4. `clax feedback
follow` prints one line per notice. With `--once`, it exits 0 after the
first poll that printed any.

Claude Code receives notices in one of two ways. When the session was
launched with `--dangerously-load-development-channels
plugin:clax@<marketplace>` (or `--channels`, with an organization
allowlist entry), the shim, which declares `claude/channel`, polls the
notices route and sends each line as a `notifications/claude/channel`
event, with `meta` `{artifact_id, thread_id, comment_id}`. Otherwise the
skill has the agent run `clax feedback follow --once` in the background
after it publishes, and restart it after each exit. Claude Code wakes an
idle session when a background command exits.

Claude Code tells a channel server nothing about registration, and drops
events it does not accept. The shim polls only when its parent's command
line names a Clax channel entry. It reports `registered: null` because it
cannot know more. It never declares `claude/channel/permission`, so
nobody who comments can approve tool use.
```

- [ ] **Step 4: §12 MCP tool surface**

After `Server: \`status\` (daemon URL, version, this session's ID and watches, and \`push\`: whether tier 5 reaches this session and why not).`, add:

```markdown
Under Claude Code, `push` also carries `channel` (whether the launch flag
names the channel, and how to launch with it) and, when no channel
forwards notices, `follow_command`, the background command the skill runs.
```

- [ ] **Step 5: §13 Claude Code plugin**

In the `### Claude Code (\`plugins/claude-code\`)` list, change the first bullet to:

```markdown
- `.claude-plugin/plugin.json`: name `clax`, keywords, version, and
  `channels: [{"server": "clax"}]`, which lets a session launched with
  `--dangerously-load-development-channels plugin:clax@clax` register the
  `clax` server as a channel (§10, Notices).
```

Append to the `skills/clax/SKILL.md` bullet: `, and the tier 5 fallback (start \`push.follow_command\` in the background after a publish when \`push.tier\` is null)`.

- [ ] **Step 6: §16 Testing and §18 Open questions**

In §16, add a bullet:

```markdown
- Claude Code channel: a fake `claude` (a script whose command line
  carries the launch flags, and which runs `clax mcp --agent claude` as its
  child) drives the shim over raw JSON-RPC. The tests check the capability,
  the protocol cap, one event per comment, silence without the flag, one
  announcement across the channel and `clax feedback follow`, and the pause
  inside `wait_for_feedback`. `scripts/smoke-claude-push.sh` is the owner's
  live check.
```

In §18, add:

```markdown
- **Channel registration is invisible to the server.** Claude Code neither
  acknowledges a channel event nor says whether it registered the server.
  The shim infers the channel from the launch flag. If Claude Code ever
  exposes registration (a client capability in `initialize`, or an
  acknowledgement), `status` should report it in `registered`, and the
  shim should poll only when registration is confirmed.
- **Channels are a research preview.** The flag syntax and the
  notification contract may change. Clax's channel code is in
  `crates/clax-mcp/src/channel.rs`, so a change lands in one place.
```

- [ ] **Step 7: `docs/contract.md`**

1. **status** (the `push` paragraph, after `Under Claude Code it is as shown above.`): replace the example `push` in the status JSON with:

```json
  "push": {
    "tier": null,
    "available": false,
    "reason": "nothing wakes this session while it is idle: launch Claude Code with `claude --dangerously-load-development-channels plugin:clax@clax`, or run follow_command in the background after publishing; meanwhile comments arrive at the end of a turn (Stop hook), with the next prompt, on the next clax tool call, or during wait_for_feedback",
    "follow_command": "'/Users/alex/.cargo/bin/clax' feedback follow --once --agent claude --harness-session '6b1f0c2e-9d4a-4c1e-8f3b-2a7d5e9c0b14'",
    "channel": {
      "declared": true,
      "launch_flag": "absent",
      "flag": null,
      "entry": null,
      "registered": null,
      "launch": "claude --dangerously-load-development-channels plugin:clax@clax",
      "note": "Claude Code does not tell the server whether it registered the channel; its startup screen says so"
    }
  },
```

Then add this paragraph after the `push` paragraph:

```markdown
Under Claude Code, `push.tier` is `"channel"` (`available: true`) when the
launch flag names the Clax channel and the shim forwards notices,
`"follow"` (`available: true`) while a `clax feedback follow` polls for the
session, and otherwise `null`, with `follow_command` (the shell-quoted
command the skill runs in the background; absent when the session has no
harness session ID). `channel` reports `declared` (always `true` under
Claude Code), `launch_flag` (`present`, `absent`, or `unknown` when the
parent's command line could not be read), the `flag` and `entry` seen,
`registered` (always `null`: Claude Code does not say), `launch` (the
command that enables the channel), and `note`. The daemon's own `push` for
a Claude Code session is `{"tier": "notice", "available": true, "reason":
null}` while any notice follower polls, else `tier: null` with the reason
above. The shim refines it.
```

2. **Delivery tiers per harness**: replace the Claude Code cell of row 5 with:

```markdown
channel (opt-in launch flag, research preview) or follow fallback: a notice, never the payload. Launched with `--dangerously-load-development-channels plugin:clax@clax`, the shim sends a `notifications/claude/channel` event per comment, which starts a turn when idle and joins the next turn when busy. Otherwise the skill runs `clax feedback follow --once` in the background, and its exit wakes the session. Either way the comment is then delivered by tier 1, 2 or 4
```

After the paragraph that begins `Tier 1 applies to every successful tool result`, add:

```markdown
Tier 5 for Claude Code announces and never delivers (see "Notices"). The
shim forwards notices only when its parent's command line names a
`plugin:clax@<marketplace>` entry of `--dangerously-load-development-channels`
or `--channels`. It supports MCP revisions up to `2025-11-25`, because
Claude Code does not register a channel server that negotiates
`2026-07-28` (under `MCP_PROTOCOL_NEGOTIATION=auto`). It never declares
`claude/channel/permission`.
```

3. **`clax feedback follow`**: if the contract already documents it (Grok plan), add `--once` to its synopsis and this sentence: `With \`--once\`, it exits 0 after the first poll that printed at least one line, or when the session ends.` If it does not, add the Notices subsection text from Step 3 under "Delivery tiers per harness", followed by the command's synopsis exactly as the Grok plan's Task 6 "Produces" gives it, with `[--once]` added.

4. **`clax doctor --agent`**: add a bullet after `feedback`:

```markdown
- `channel` (Claude Code only): whether the installed plugin's manifest
  declares the channel (failed when it does not: the plugin predates it),
  and how the latest Claude Code session was launched, from the shim's
  `channel` line in `hooks.log`, with the launch command. The channel is
  opt-in, so a session launched without it passes.
```

5. **Known limitations**: add:

```markdown
- Claude Code channels are a research preview: CLI only, with claude.ai or
  Console authentication (not Bedrock, Google Cloud or Foundry). Clax is
  not on the `--channels` allowlist, so it needs
  `--dangerously-load-development-channels plugin:clax@clax` (with a warning
  screen at every launch) or an organization `allowedChannelPlugins` entry.
  On claude.ai Team and Enterprise an Owner must turn on `channelsEnabled`.
  Claude Code never tells Clax whether the channel registered. When the
  flag is given but policy blocks the channel, comments still arrive
  through tiers 1, 2 and 4, but an idle session is not woken. Relaunch
  without the flag to use the background fallback.
- The background fallback needs the agent to start `clax feedback follow
  --once` after a publish and to restart it after each wake-up. A session
  that has not published or watched anything in this run is not woken.
- `just dev claude` loads the checkout's plugin with `--plugin-dir`, which
  has no `plugin:<name>@<marketplace>` entry, so dev sessions use the
  background fallback.
```

- [ ] **Step 8: Check and stage**

Run `bash scripts/quality_gates.sh; echo "exit=$?"` (the docs gates and the name gate). Stage both files.

Proposed commit message: `Specify tier 5 for Claude Code: channel notices when launched with the channel, a background clax feedback follow otherwise`

---

### Task 2: Notices pipeline, `--once`, and Claude Code's `push`

**Files:**
- Either consume or create (see Step 1) the Grok plan's Task 6 files: `crates/clax-core/src/store/migrations.rs`, `crates/clax-core/src/store/feedback.rs`, `crates/clax-core/src/feedback.rs`, `crates/clax-server/src/feedback.rs`, `crates/clax-server/src/state.rs`, `crates/clax-server/src/routes/feedback.rs`, `crates/clax-server/src/routes/mod.rs`, `crates/clax-server/tests/api_notices.rs`, `crates/clax-cli/src/commands/feedback.rs`, `crates/clax-cli/src/commands/mod.rs`, `crates/clax-cli/src/main.rs`, `crates/clax-cli/tests/follow.rs`
- Modify: `crates/clax-server/src/routes/sessions.rs` (`push_info` gets `following`; Claude Code's arm)
- Modify: `crates/clax-cli/src/commands/feedback.rs` (`--once`)

**Interfaces:**
- Consumes: the Grok plan's Task 6 when it has landed.
- Produces, with names and shapes identical to the Grok plan's Task 6:
  - `Store::take_notices(&self, session_id: &str, browser_base: &str) -> Result<Vec<Notice>>`
  - `clax_core::feedback::{Notice, render_notice}`
  - `GET /api/sessions/<sid>/notices?wait=<s>` (token) → `{notices: [{feedback_id, comment_id, thread_id, artifact_id, title, url}], lines: [<string>], waited_s}`
  - `crate::feedback::Followers` with `enter` and `is_following`, held in `AppState.followers`
  - `clax feedback follow [--session <ID> | --agent <claude|codex|grok> --harness-session <ID>]`
- Produces (new): `clax feedback follow … --once`; Claude Code's daemon `push`: `{"tier": "notice", "available": true, "reason": null}` while following, else `{"tier": null, "available": false, "reason": <CLAUDE_NO_PUSH>}`.

- [ ] **Step 1: Find out whether the Grok plan's Task 6 has landed**

```bash
git grep -q "fn take_notices" -- crates/clax-core/src/store/feedback.rs; echo "store=$?"
git grep -q "/notices" -- crates/clax-server/src/routes/mod.rs; echo "route=$?"
test -f crates/clax-cli/src/commands/feedback.rs; echo "follow=$?"
```

- All three print `0`: **path A**. The pipeline exists. Go to Step 3.
- All three print `1`: **path B**. Build it in Step 2.
- Anything else: stop and ask the controller. A half-built pipeline must not be finished from two plans.

- [ ] **Step 2 (path B only): Build the Grok plan's Task 6, Steps 1 to 6, as written there**

Open `docs/superpowers/plans/2026-10-01-grok-build.md`, "### Task 6", and implement Steps 1 to 6 in order, test-first, with the code exactly as given there: the migration, `Notice`, `render_notice`, `take_notices` and the clearing of `notified_at` on untarget and retarget, `Followers` and `FollowGuard`, the `notices` handler and its route, and `crates/clax-cli/src/commands/feedback.rs` (with `FollowAgent::Grok` and the `GROK_SESSION_ID` default kept, since neither needs anything from the daemon). Make only these substitutions, because the `grok` harness does not exist yet:

1. In the store tests, register the sessions as `session(&st, "claude", "c1")` (and `"c2"`) instead of `"grok"`.
2. In `api_notices.rs`, register `{"harness": "claude", "harness_session_id": "c1", …}` (name the helper `claude_session`), and replace `grok_push_reports_whether_a_follower_is_connected` with the Claude Code test in Step 4 below.
3. In `follow.rs`, follow with `--agent claude --harness-session c1`. Replace `it_finds_the_grok_session_from_the_environment` with a unit test of `target` (`GROK_SESSION_ID=g1` alone gives `Target::Harness("grok", "g1")`; flags win over it; nothing gives `None`).
4. In `push_info`, add the `following: bool` parameter and pass `s.followers.is_following(&session.id)` from the call site, but add no `"grok"` arm. Step 4 adds the Claude Code arm.

When the Grok plan runs later, its Task 6 finds these pieces present and adds only its Grok parts: the `"grok"` push arm, the Grok tests, and the rename of its Task 2 test. Record this in the task report so the controller can tell that plan's executor.

Run: `cargo test -p clax-core notice && cargo test -p clax-server --test api_notices && cargo test -p clax-cli --test follow`. Expected: PASS.

- [ ] **Step 3: Write the failing `--once` tests**

Add to `crates/clax-cli/tests/follow.rs`, using its existing helpers (a daemon on `--port 0` in a scratch home, a registered session, a reader thread over the child's stdout):

```rust
#[test]
fn once_exits_after_the_first_poll_that_printed() {
    // Register {"harness": "claude", "harness_session_id": "c1"}.
    // Spawn `clax feedback follow --once --agent claude --harness-session c1 --poll-secs 2`.
    // Publish as the session (arms a watch), open two threads, send both to the agent
    // in one store call or in quick succession.
    // The process exits 0 within 5 s. Stdout holds one or two lines, each starting
    // with "[clax] New comment on ". Stderr is empty.
    // A second `--once` run then prints the remaining line, if any, and exits 0,
    // so the two runs together print exactly two lines.
}

#[test]
fn once_exits_cleanly_when_the_session_ends() {
    // `--once --grace-secs 1`; end the session with PATCH {"ended": true};
    // exit 0 within 5 s with empty stdout.
}

#[test]
fn once_does_not_exit_on_an_empty_poll() {
    // `--once --poll-secs 1`; no comment; after 3 s the process is still running. Kill it.
}
```

Write each in full. Run: `cargo test -p clax-cli --test follow once_`. Expected: FAIL (`--once` is not an argument).

- [ ] **Step 4: Write the failing Claude Code `push` test**

In `crates/clax-server/tests/api_notices.rs`:

```rust
#[tokio::test]
async fn claude_push_reports_a_notice_follower() {
    let ts = TestServer::spawn().await;
    let sid = claude_session(&ts).await;
    let get = |ts: &TestServer, sid: &str| {
        let path = format!("/api/sessions/{sid}");
        async move { ts.get_authed(&path).await.json::<Value>().await.unwrap()["push"].clone() }
    };
    let idle = get(&ts, &sid).await;
    assert_eq!(idle["tier"], Value::Null);
    assert_eq!(idle["available"], false);
    assert!(idle["reason"].as_str().unwrap().contains("--dangerously-load-development-channels plugin:clax@clax"), "{idle}");
    // A notices poll in progress counts as a follower.
    let poll = {
        let ts = ts.clone();
        let sid = sid.clone();
        tokio::spawn(async move { ts.get_authed(&format!("/api/sessions/{sid}/notices?wait=3")).await })
    };
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let following = get(&ts, &sid).await;
    assert_eq!(following, json!({"tier": "notice", "available": true, "reason": null}));
    poll.await.unwrap();
}
```

If `TestServer` is not `Clone`, share it through an `Arc`, as the other long-poll tests in `api_feedback.rs` do. Update every existing test that asserts the old Claude Code reason (`grep -rn "Claude Code has no native push" crates`) to the new text. Run: `cargo test -p clax-server --test api_notices claude_push`. Expected: FAIL.

- [ ] **Step 5: Implement `--once` and the Claude Code arm**

In `crates/clax-cli/src/commands/feedback.rs`, add to `FollowArgs`:

```rust
    /// Exit 0 after the first poll that printed at least one line, so a
    /// harness that wakes when a background command exits (Claude Code) is
    /// woken by the first comment. The agent starts it again after handling
    /// the comment.
    #[arg(long)]
    pub once: bool,
```

and in `run`, in the `Ok(v)` arm, after `stdout.flush()?;`:

```rust
                if a.once && v["lines"].as_array().is_some_and(|l| !l.is_empty()) {
                    return Ok(());
                }
```

Add to the `Follow` variant's doc comment: `With --once, it exits after the first comment, for a harness that wakes when a background command exits (Claude Code).`

In `crates/clax-server/src/routes/sessions.rs`, add a constant and a `"claude"` arm before the catch-all:

```rust
/// Why nothing wakes an idle Claude Code session that no notice follower
/// polls for.
const CLAUDE_NO_PUSH: &str = "nothing wakes this session while it is idle: launch Claude Code with `claude --dangerously-load-development-channels plugin:clax@clax`, or run follow_command in the background after publishing; meanwhile comments arrive at the end of a turn (Stop hook), with the next prompt, on the next clax tool call, or during wait_for_feedback";
```

```rust
        "claude" if following => json!({"tier": "notice", "available": true, "reason": null}),
        "claude" => json!({"tier": null, "available": false, "reason": CLAUDE_NO_PUSH}),
```

Leave the catch-all arm for any other harness as it is.

Run: `cargo test -p clax-cli --test follow && cargo test -p clax-server`. Expected: PASS.

- [ ] **Step 6: Gates and stage**

Run the quality gates. Stage the Task 2 files (path B: every file listed above; path A: `sessions.rs`, `feedback.rs`, `follow.rs`, `api_notices.rs`).

Proposed commit message (path A): `Add clax feedback follow --once, and report a notice follower in Claude Code's push`
Proposed commit message (path B): `Add comment notices (feedback.notified_at, GET /api/sessions/<sid>/notices, clax feedback follow --once); report a notice follower in Claude Code's push`

---

### Task 3: The channel in the shim

**Files:**
- Create: `crates/clax-mcp/src/channel.rs`
- Modify: `crates/clax-mcp/src/lib.rs` (`pub mod channel;`)
- Modify: `crates/clax-mcp/src/client.rs` (`notices`)
- Modify: `crates/clax-mcp/src/shim.rs` (`run` takes `Option<ChannelState>`; the forwarding loop; a shared `run_with_timeout`)
- Modify: `crates/clax-mcp/src/tools.rs` (`with_channel`; `get_info`; `supported_protocol_versions`; the `status` overlay)
- Modify: `crates/clax-cli/src/commands/mcp.rs` (detect and log under `--agent claude`)
- Create: `crates/clax-mcp/tests/channel.rs`

**Interfaces:**
- Consumes: Task 2 (`/notices`, `render_notice` lines, `--once`, Claude Code's daemon `push`).
- Produces:
  - `clax_mcp::channel::{ChannelState, LaunchFlag, launch_flag, process_argv, event_params, push_for_status, CAPABILITY, METHOD, ENTRY, LAUNCH, INSTRUCTIONS}`
  - `shim::run(harness, home, refresh, discover, heartbeat, upgrade_hold, channel: Option<ChannelState>)`
  - `ClaxTools::with_channel(self, ChannelState) -> Self`
  - `DaemonClient::notices(&self, wait_s: u64) -> Result<Value>`
  - The `hooks.log` line `<time> channel agent=claude launch_flag=<present|absent|unknown> flag="<flag>" entry="<entry>" parent_pid=<pid>`, which Task 4's doctor check reads.

- [ ] **Step 1: Write the failing unit tests**

Create `crates/clax-mcp/src/channel.rs` with only its tests module first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn argv(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn the_development_flag_naming_clax_is_present() {
        assert_eq!(
            launch_flag(&argv("claude --dangerously-load-development-channels plugin:clax@clax")),
            LaunchFlag::Present {
                flag: "--dangerously-load-development-channels".into(),
                entry: "plugin:clax@clax".into()
            }
        );
        assert_eq!(
            launch_flag(&argv("node /x/cli.js --channels plugin:telegram@claude-plugins-official plugin:clax@acme --model opus")),
            LaunchFlag::Present { flag: "--channels".into(), entry: "plugin:clax@acme".into() }
        );
        assert_eq!(
            launch_flag(&argv("claude --dangerously-load-development-channels=plugin:clax@clax")),
            LaunchFlag::Present {
                flag: "--dangerously-load-development-channels".into(),
                entry: "plugin:clax@clax".into()
            }
        );
    }

    #[test]
    fn other_entries_options_and_positionals_are_absent() {
        for line in [
            "claude",
            "claude --dangerously-load-development-channels server:clax",
            "claude --channels plugin:clax-grok@clax",
            "claude --channels plugin:telegram@x --resume plugin:clax@clax",
            "claude plugin:clax@clax",
            "claude --dangerously-load-development-channels plugin:clax@",
        ] {
            assert_eq!(launch_flag(&argv(line)), LaunchFlag::Absent, "{line}");
        }
    }

    #[test]
    fn this_process_argv_is_read() {
        let me = process_argv(std::process::id()).unwrap();
        assert!(!me.is_empty());
    }

    #[test]
    fn an_event_is_the_follow_line_with_identifier_meta_keys() {
        let notice = json!({"feedback_id": "f", "comment_id": "c1", "thread_id": "t1",
            "artifact_id": "7q3k9mzx2b4t", "title": "T", "url": "http://h/a/7q3k9mzx2b4t"});
        let p = event_params(&notice, "[clax] New comment on \"T\" …");
        assert_eq!(p["content"], "[clax] New comment on \"T\" …");
        assert_eq!(p["meta"], json!({"artifact_id": "7q3k9mzx2b4t", "thread_id": "t1", "comment_id": "c1"}));
        for k in p["meta"].as_object().unwrap().keys() {
            assert!(k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'), "{k}");
        }
    }

    #[test]
    fn status_says_channel_only_when_the_flag_is_present() {
        let present = ChannelState { launch_flag: launch_flag(&argv("claude --dangerously-load-development-channels plugin:clax@clax")) };
        let daemon = json!({"tier": "notice", "available": true, "reason": null});
        let p = push_for_status(&present, daemon, Some("hs"), "/b/clax");
        assert_eq!(p["tier"], "channel");
        assert_eq!(p["available"], true);
        assert_eq!(p["channel"]["launch_flag"], "present");
        assert_eq!(p["channel"]["registered"], Value::Null);
        assert!(p.get("follow_command").is_none());

        let absent = ChannelState { launch_flag: LaunchFlag::Absent };
        let idle = json!({"tier": null, "available": false, "reason": "r"});
        let p = push_for_status(&absent, idle, Some("6b1f'0c"), "/b dir/clax");
        assert_eq!(p["tier"], Value::Null);
        assert_eq!(p["reason"], "r");
        assert_eq!(p["follow_command"], "'/b dir/clax' feedback follow --once --agent claude --harness-session '6b1f'\\''0c'");
        assert_eq!(p["channel"]["launch"], LAUNCH);

        let following = json!({"tier": "notice", "available": true, "reason": null});
        let p = push_for_status(&absent, following, Some("hs"), "/b/clax");
        assert_eq!(p["tier"], "follow");
        assert!(p["follow_command"].is_string());

        assert_eq!(push_for_status(&absent, Value::Null, None, "/b/clax"), Value::Null);
        let unknown = ChannelState { launch_flag: LaunchFlag::Unknown("ps failed".into()) };
        let p = push_for_status(&unknown, json!({"tier": null, "available": false, "reason": "r"}), None, "/b/clax");
        assert_eq!(p["channel"]["launch_flag"], "unknown");
        assert!(p.get("follow_command").is_none(), "no harness session ID, no command");
    }
}
```

Add `pub mod channel;` to `lib.rs`. Run: `cargo test -p clax-mcp channel`. Expected: FAIL (nothing defined).

- [ ] **Step 2: Implement `channel.rs`**

Above the tests:

```rust
//! Claude Code channels: tier 5 under Claude Code. The shim declares the
//! `claude/channel` capability. When its session was launched with the
//! channel, it forwards each comment notice (`GET /api/sessions/<sid>/notices`)
//! as a `notifications/claude/channel` event. A notice points at a comment
//! and delivers nothing. The comment still arrives through tier 1, 2 or 4.
//!
//! Claude Code tells a server neither whether it registered it as a channel
//! nor whether it accepted an event: it drops unaccepted events silently.
//! The shim can only read how its parent, the `claude` process, was launched.
//! A channel can register only when `--dangerously-load-development-channels`
//! or `--channels` names this plugin. Organization policy, the allowlist or
//! the authentication method can still refuse it, and the shim cannot see
//! that. Permission relay (`claude/channel/permission`) is never declared.

use serde_json::{Value, json};

/// The experimental capability that makes an MCP server a channel.
pub const CAPABILITY: &str = "claude/channel";
/// The notification method of a channel event.
pub const METHOD: &str = "notifications/claude/channel";
/// The launch-flag entry for the plugin as `clax init` installs it
/// (plugin `clax` from marketplace `clax`).
pub const ENTRY: &str = "plugin:clax@clax";
/// The command that launches Claude Code with the Clax channel.
pub const LAUNCH: &str = "claude --dangerously-load-development-channels plugin:clax@clax";
/// What `status` says about registration, which the shim cannot observe.
const NOTE: &str = "Claude Code does not tell the server whether it registered the channel; its startup screen says so";
/// The launch flags whose values are channel entries.
const FLAGS: &[&str] = &["--dangerously-load-development-channels", "--channels"];

/// Added to the server instructions when the channel is declared.
pub const INSTRUCTIONS: &str = "Clax sends comment notices as <channel source=\"plugin:clax:clax\" \
artifact_id=\"...\" thread_id=\"...\" comment_id=\"...\">. A notice says only that a person sent \
you a comment on a Clax artifact; it does not contain the comment. Call comments_read with \
url_or_id set to artifact_id and thread_id set to thread_id, act on the comment, answer with \
comments_reply, and call comments_resolve when done. If you have already handled that thread, do \
nothing. Nothing is sent back over the channel.";

/// Whether the parent's command line names a Clax channel entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchFlag {
    /// `flag` names `entry`, a `plugin:clax@<marketplace>` entry.
    Present { flag: String, entry: String },
    /// The command line names no Clax channel entry.
    Absent,
    /// The command line could not be read, for this reason.
    Unknown(String),
}

/// Whether `entry` names the Clax plugin from some marketplace.
fn is_clax(entry: &str) -> bool {
    entry
        .strip_prefix("plugin:clax@")
        .is_some_and(|m| !m.is_empty())
}

/// The Clax channel entry that `argv` passes to a channel launch flag. Each
/// value after the flag, up to the next option, is an entry; `--flag=value`
/// passes one.
pub fn launch_flag(argv: &[String]) -> LaunchFlag {
    let present = |flag: &str, entry: &str| LaunchFlag::Present {
        flag: flag.to_string(),
        entry: entry.to_string(),
    };
    let mut current: Option<&str> = None;
    for arg in argv {
        if let Some((flag, value)) = arg.split_once('=')
            && FLAGS.contains(&flag)
        {
            if is_clax(value) {
                return present(flag, value);
            }
            current = None;
        } else if FLAGS.contains(&arg.as_str()) {
            current = Some(arg);
        } else if arg.starts_with('-') {
            current = None;
        } else if let Some(flag) = current
            && is_clax(arg)
        {
            return present(flag, arg);
        }
    }
    LaunchFlag::Absent
}

/// The command line of process `pid`.
#[cfg(target_os = "linux")]
pub fn process_argv(pid: u32) -> Result<Vec<String>, String> {
    let raw = std::fs::read(format!("/proc/{pid}/cmdline")).map_err(|e| e.to_string())?;
    let argv: Vec<String> = raw
        .split(|b| *b == 0)
        .filter(|a| !a.is_empty())
        .map(|a| String::from_utf8_lossy(a).into_owned())
        .collect();
    if argv.is_empty() { Err("empty command line".into()) } else { Ok(argv) }
}

/// The command line of process `pid`, from `ps -ww -o args=`, split on
/// whitespace. `ps` joins the arguments with spaces. Channel entries contain
/// none, so they come back whole.
#[cfg(not(target_os = "linux"))]
pub fn process_argv(pid: u32) -> Result<Vec<String>, String> {
    let out = crate::shim::run_with_timeout(
        std::process::Command::new("ps").args(["-ww", "-o", "args=", "-p", &pid.to_string()]),
        std::time::Duration::from_secs(2),
    )
    .ok_or_else(|| "ps did not answer within 2 s".to_string())?;
    let argv: Vec<String> = out.split_whitespace().map(str::to_string).collect();
    if argv.is_empty() { Err(format!("ps printed nothing for process {pid}")) } else { Ok(argv) }
}

/// What the shim knows about its channel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelState {
    pub launch_flag: LaunchFlag,
}

impl ChannelState {
    /// Reads the launch flag from the command line of `parent_pid`.
    pub fn detect(parent_pid: u32) -> ChannelState {
        let launch_flag = match process_argv(parent_pid) {
            Ok(argv) => launch_flag(&argv),
            Err(e) => LaunchFlag::Unknown(e),
        };
        ChannelState { launch_flag }
    }

    /// Whether the shim forwards notices as channel events.
    pub fn forwards(&self) -> bool {
        matches!(self.launch_flag, LaunchFlag::Present { .. })
    }

    /// `status.push.channel`.
    pub fn to_json(&self) -> Value {
        let (state, flag, entry, why) = match &self.launch_flag {
            LaunchFlag::Present { flag, entry } => ("present", Some(flag.as_str()), Some(entry.as_str()), None),
            LaunchFlag::Absent => ("absent", None, None, None),
            LaunchFlag::Unknown(why) => ("unknown", None, None, Some(why.as_str())),
        };
        let mut v = json!({"declared": true, "launch_flag": state, "flag": flag, "entry": entry,
            "registered": null, "launch": LAUNCH, "note": NOTE});
        if let Some(why) = why {
            v["unknown_reason"] = json!(why);
        }
        v
    }

    /// The key=value fields of the shim's `channel` line in hooks.log.
    pub fn log_fields(&self, parent_pid: u32) -> String {
        let (state, flag, entry) = match &self.launch_flag {
            LaunchFlag::Present { flag, entry } => ("present", flag.as_str(), entry.as_str()),
            LaunchFlag::Absent => ("absent", "", ""),
            LaunchFlag::Unknown(_) => ("unknown", "", ""),
        };
        format!("launch_flag={state} flag=\"{flag}\" entry=\"{entry}\" parent_pid={parent_pid}")
    }
}

/// The params of the channel event for one notice: the follow line as
/// `content`, and the notice's IDs as `meta`. Meta keys use only letters,
/// digits and underscores; Claude Code drops any other key.
pub fn event_params(notice: &Value, line: &str) -> Value {
    let id = |k: &str| notice[k].as_str().unwrap_or_default().to_string();
    json!({"content": line, "meta": {
        "artifact_id": id("artifact_id"), "thread_id": id("thread_id"), "comment_id": id("comment_id"),
    }})
}

/// `s` as one single-quoted shell word.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// `status.push` under Claude Code: the daemon's `push`, refined. The tier is
/// `channel` when the shim forwards notices, and `follow` when the daemon sees
/// a notice follower and the shim is not one. Without forwarding,
/// `follow_command` is the background command the skill runs (present when
/// the harness session ID is known). `null` (no session) stays `null`.
pub fn push_for_status(ch: &ChannelState, daemon: Value, harness_session_id: Option<&str>, bin: &str) -> Value {
    let Value::Object(_) = daemon else { return daemon };
    let mut push = daemon;
    if ch.forwards() {
        push["tier"] = json!("channel");
        push["available"] = json!(true);
        push["reason"] = Value::Null;
    } else {
        if push["tier"] == "notice" {
            push["tier"] = json!("follow");
        }
        if let Some(hs) = harness_session_id {
            push["follow_command"] = json!(format!(
                "{} feedback follow --once --agent claude --harness-session {}",
                shell_quote(bin),
                shell_quote(hs)
            ));
        }
    }
    push["channel"] = ch.to_json();
    push
}
```

In `shim.rs`, move the wait loop out of `process_cwd` into a shared helper, and make `process_cwd` call it:

```rust
/// Runs `cmd` with no stdin and no stderr and returns its stdout, or `None`
/// when it fails to start, exits non-zero, or runs past `timeout` (then it
/// is killed).
#[cfg_attr(target_os = "linux", allow(dead_code))]
pub(crate) fn run_with_timeout(cmd: &mut std::process::Command, timeout: Duration) -> Option<String>
```

The body is the existing `lsof` loop, generalised. It also returns `None` when the exit status is not success. `this_process_cwd_is_found` must still pass.

Run: `cargo test -p clax-mcp channel && cargo test -p clax-mcp --lib shim`. Expected: PASS.

- [ ] **Step 3: Write the failing end-to-end tests with a fake `claude`**

Create `crates/clax-mcp/tests/channel.rs`. Copy `clax_bin()` from `tests/shim.rs`, along with the REST helpers that `piggyback_and_wait_through_the_shim` uses to open a thread and send it to the agent. If those helpers are local to `shim.rs`, move them into `tests/common/mod.rs` and use them from both files.

The fake `claude` is a script written into the test's temporary directory. Its command line carries the flags under test, and it runs the shim as a child, not through `exec`, so the shim's parent is the script and `ps` shows the flags:

```rust
/// A stand-in for `claude`: `<dir>/claude <flags…>` runs `clax mcp --agent
/// claude` as its child, with stdin and stdout passed through, so the shim's
/// parent command line carries the flags. Driven over raw JSON-RPC lines.
struct FakeClaude {
    dir: tempfile::TempDir,
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    rx: std::sync::mpsc::Receiver<Value>,
    pending: Vec<Value>,
    next_id: u64,
}

const FAKE: &str = "#!/bin/bash\n\"$CLAX_TEST_BIN\" --port 0 mcp --agent claude\n";

impl FakeClaude {
    fn launch(flags: &[&str]) -> FakeClaude {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("claude");
        std::fs::write(&script, FAKE).unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        std::fs::create_dir_all(dir.path().join("work")).unwrap();
        let mut cmd = std::process::Command::new(&script);
        cmd.args(flags)
            .current_dir(dir.path().join("work"))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        for k in ["CLAUDE_PID", "CLAUDE_CODE_SESSION_ID", "CLAUDE_PLUGIN_ROOT", "CLAUDE_PROJECT_DIR",
                  "CLAX_SESSION_ID", "GROK_SESSION_ID", "GROK_HOOK_EVENT", "GROK_PLUGIN_ROOT"] {
            cmd.env_remove(k);
        }
        cmd.env("CLAX_TEST_BIN", clax_bin())
            .env("CLAX_HOME", dir.path().join("ax"))
            .env("HOME", dir.path())
            .env("CLAUDE_CONFIG_DIR", dir.path().join("claude-config"))
            .env("CLAX_CODEX_BIN", "")
            .env("CLAX_NO_OPEN", "1")
            .env("CLAUDE_CODE_SESSION_ID", "fake-sess")
            .env("RUST_LOG", "error");
        let mut child = cmd.spawn().expect("spawn the fake claude");
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            use std::io::BufRead;
            for line in std::io::BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Ok(v) = serde_json::from_str::<Value>(&line) {
                    if tx.send(v).is_err() { break; }
                }
            }
        });
        FakeClaude { dir, child, stdin, rx, pending: Vec::new(), next_id: 1 }
    }

    fn send(&mut self, v: Value) {
        use std::io::Write;
        writeln!(self.stdin, "{v}").unwrap();
        self.stdin.flush().unwrap();
    }

    /// Sends a request and returns its response, keeping notifications that
    /// arrive meanwhile.
    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.send_request(method, params);
        self.response(id, Duration::from_secs(30))
    }

    fn send_request(&mut self, method: &str, params: Value) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        id
    }

    fn response(&mut self, id: u64, within: Duration) -> Value {
        let end = Instant::now() + within;
        loop {
            let v = self.rx.recv_timeout(end.saturating_duration_since(Instant::now())).expect("response");
            if v["id"] == id { return v; }
            self.pending.push(v);
        }
    }

    /// `initialize` with `version`, then `notifications/initialized`.
    fn initialize(&mut self, version: &str) -> Value {
        let r = self.request("initialize", json!({"protocolVersion": version, "capabilities": {},
            "clientInfo": {"name": "fake-claude", "version": "0"}}));
        self.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        r
    }

    /// Every channel event received within `within`.
    fn channel_events(&mut self, within: Duration) -> Vec<Value> {
        let end = Instant::now() + within;
        while let Ok(v) = self.rx.recv_timeout(end.saturating_duration_since(Instant::now())) {
            self.pending.push(v);
        }
        let (events, rest): (Vec<Value>, Vec<Value>) = std::mem::take(&mut self.pending)
            .into_iter()
            .partition(|v| v["method"] == "notifications/claude/channel");
        self.pending = rest;
        events.into_iter().map(|v| v["params"].clone()).collect()
    }

    fn call(&mut self, tool: &str, args: Value) -> Value {
        let r = self.request("tools/call", json!({"name": tool, "arguments": args}));
        serde_json::from_str(r["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
    }
}

impl Drop for FakeClaude {
    fn drop(&mut self) {
        let _ = std::process::Command::new(clax_bin()).arg("stop").env("CLAX_HOME", self.dir.path().join("ax")).status();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
```

Use whatever `tools/call` result parsing `tests/shim.rs` uses (`body`/`ok`) if `content[0]` is not the JSON. The tests:

```rust
const FLAG: &[&str] = &["--dangerously-load-development-channels", "plugin:clax@clax"];

#[test]
fn declares_the_channel_and_never_permission_relay() {
    let mut c = FakeClaude::launch(&[]);
    let r = c.initialize("2025-06-18");
    assert_eq!(r["result"]["capabilities"]["experimental"], json!({"claude/channel": {}}));
    let i = r["result"]["instructions"].as_str().unwrap();
    assert!(i.contains("<channel source=\"plugin:clax:clax\"") && i.contains("comments_read"), "{i}");
}

#[test]
fn a_2026_07_28_client_gets_a_channel_capable_revision() {
    let mut c = FakeClaude::launch(FLAG);
    let r = c.initialize("2026-07-28");
    assert_eq!(r["result"]["protocolVersion"], "2025-11-25", "{r}");
    let mut d = FakeClaude::launch(FLAG);
    let r = d.request("server/discover", json!({}));
    if let Some(v) = r["result"]["supportedVersions"].as_array() {
        assert!(!v.iter().any(|x| x == "2026-07-28"), "{r}");
        assert!(v.iter().any(|x| x == "2025-11-25"), "{r}");
    } else {
        assert!(r.get("error").is_some(), "{r}");
    }
}

#[test]
fn the_codex_shim_declares_no_channel() {
    // Spawn `clax --port 0 mcp --agent codex` directly (scratch homes, cleared env),
    // initialize with "2026-07-28": no "experimental" in the capabilities, and the
    // instructions do not mention <channel.
}

#[test]
fn forwards_one_channel_event_per_comment_when_launched_with_the_channel() {
    let mut c = FakeClaude::launch(FLAG);
    c.initialize("2025-11-25");
    let published = c.call("publish", json!({"html": "<!doctype html><title>Push</title><p>x</p>"}));
    let aid = published["id"].as_str().unwrap().to_string();
    let tid = send_comment(&c.dir, &aid, "please make it blue");
    let events = c.channel_events(Duration::from_secs(5));
    assert_eq!(events.len(), 1, "{events:?}");
    let content = events[0]["content"].as_str().unwrap();
    assert!(content.starts_with("[clax] New comment on \"Push\"") && content.contains(&tid), "{content}");
    assert!(!content.contains("make it blue"), "a notice never carries the comment");
    assert_eq!(events[0]["meta"], json!({"artifact_id": aid, "thread_id": tid, "comment_id": events[0]["meta"]["comment_id"]}));
    for k in events[0]["meta"].as_object().unwrap().keys() {
        assert!(k.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_'), "{k}");
    }
    assert!(c.channel_events(Duration::from_secs(3)).is_empty(), "announced once");
}

#[test]
fn a_channel_notice_delivers_nothing() {
    // As above, then over REST: GET /api/sessions/<sid>/feedback?tier=stop_hook returns
    // the comment (still undelivered). Then tools/call comments_read on the thread
    // acknowledges it, and a second stop_hook poll returns nothing.
}

#[test]
fn sends_nothing_without_the_launch_flag() {
    let mut c = FakeClaude::launch(&[]);
    c.initialize("2025-11-25");
    // Publish; send a comment; no channel event within 4 s.
    // Then run `clax feedback follow --once --agent claude --harness-session fake-sess --poll-secs 2`
    // with the same CLAX_HOME and a cleared environment: it prints the notice line within
    // 5 s and exits 0, so the shim spent no notice.
}

#[test]
fn a_comment_is_announced_once_across_channel_and_follow() {
    // FakeClaude with FLAG, published. Start `clax feedback follow --once … --poll-secs 1`
    // for the same session before sending. Send one comment. After 5 s, count
    // channel events plus follow stdout lines: exactly 1. Kill the follower if it is
    // still running. Repeat with a second comment: again exactly 1 in total.
}

#[test]
fn no_channel_event_while_waiting_for_feedback() {
    let mut c = FakeClaude::launch(FLAG);
    c.initialize("2025-11-25");
    // Publish. Send (do not await) tools/call wait_for_feedback {"timeout_s": 10}.
    // After 500 ms, send a comment. The wait's response arrives within 5 s with
    // one feedback item. No channel event arrives within 3 s after it.
}

#[test]
fn status_reports_the_channel_state() {
    let mut on = FakeClaude::launch(FLAG);
    on.initialize("2025-11-25");
    let s = on.call("status", json!({}));
    assert_eq!(s["push"]["tier"], "channel");
    assert_eq!(s["push"]["channel"]["launch_flag"], "present");
    assert_eq!(s["push"]["channel"]["entry"], "plugin:clax@clax");
    assert_eq!(s["push"]["channel"]["registered"], Value::Null);

    let mut off = FakeClaude::launch(&[]);
    off.initialize("2025-11-25");
    let s = off.call("status", json!({}));
    assert_eq!(s["push"]["tier"], Value::Null);
    assert_eq!(s["push"]["channel"]["launch_flag"], "absent");
    assert_eq!(s["push"]["channel"]["launch"], "claude --dangerously-load-development-channels plugin:clax@clax");
    let cmd = s["push"]["follow_command"].as_str().unwrap();
    assert!(cmd.ends_with("feedback follow --once --agent claude --harness-session 'fake-sess'"), "{cmd}");
}

#[test]
fn the_shim_logs_its_channel_state() {
    // After launch and initialize, <CLAX_HOME>/logs/hooks.log has a line containing
    // " channel agent=claude launch_flag=present " and "entry=\"plugin:clax@clax\"".
}
```

Write every body in full. `send_comment(dir, aid, text)` opens a thread on `index.html` over REST, adds the comment and sends it to the agent, with the token from `<dir>/ax/daemon.json`. It returns the thread ID. Run: `cargo test -p clax-mcp --test channel`. Expected: FAIL.

- [ ] **Step 4: Wire the channel into the tools, the client and the shim**

`crates/clax-mcp/src/client.rs`, next to `feedback`:

```rust
    /// `GET /api/sessions/<sid>/notices?wait=<wait_s>`: `{notices, lines,
    /// waited_s}`; the deadline is `wait_s` plus 10 s. Like `heartbeat`, it
    /// finds a running daemon but never starts one.
    pub async fn notices(&self, wait_s: u64) -> Result<Value>
```

Build it the way `heartbeat` reaches the daemon (the `discover` path), with the query and deadline built as in `feedback`.

`crates/clax-mcp/src/tools.rs`:

```rust
    /// Declares the Claude Code channel: the `claude/channel` capability, the
    /// channel instructions, protocol revisions up to 2025-11-25, and the
    /// channel state in `status.push`.
    pub fn with_channel(mut self, channel: crate::channel::ChannelState) -> Self {
        self.channel = Some(channel);
        self
    }
```

Add the `channel: Option<crate::channel::ChannelState>` field, `None` in `new`. In `get_info`:

```rust
    fn get_info(&self) -> ServerConfig {
        let mut caps = ServerCapabilities::builder().enable_tools().build();
        let mut instructions = INSTRUCTIONS.to_string();
        if self.channel.is_some() {
            // Only `claude/channel`; never `claude/channel/permission`.
            caps.experimental = Some(std::collections::BTreeMap::from([(
                crate::channel::CAPABILITY.to_string(),
                serde_json::Map::new(),
            )]));
            instructions.push_str("\n\n");
            instructions.push_str(crate::channel::INSTRUCTIONS);
        }
        let mut config = ServerConfig::new(caps).with_instructions(instructions);
        config.server_info = Implementation::new("clax", env!("CARGO_PKG_VERSION"));
        config
    }

    /// Up to 2025-11-25 with a channel: Claude Code does not register a
    /// channel server that negotiates 2026-07-28.
    fn supported_protocol_versions(&self) -> std::borrow::Cow<'static, [rmcp::model::ProtocolVersion]> {
        use rmcp::model::ProtocolVersion;
        std::borrow::Cow::Borrowed(match self.channel {
            Some(_) => ProtocolVersion::known_up_to(&ProtocolVersion::V_2025_11_25),
            None => ProtocolVersion::KNOWN_VERSIONS,
        })
    }
```

Match `with_instructions`'s parameter type, and the experimental map's value type (`JsonObject`), to rmcp 3.5's signatures. In `do_status`, after `out["push"]` is set:

```rust
        if let Some(ch) = &self.channel {
            let bin = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
            let hs = session.as_ref().and_then(|s| s.harness_session_id.clone());
            out["push"] = crate::channel::push_for_status(ch, out["push"].take(), hs.as_deref(), &bin);
        }
```

`crates/clax-mcp/src/shim.rs`. Add `channel: Option<crate::channel::ChannelState>` as the last parameter of `run`, and say in its doc comment that a channel whose launch flag is present forwards notices. Apply it with `tools = tools.with_channel(ch.clone())`. Pass `client` and whether to forward into `serve`:

```rust
/// How long each notices poll waits.
const NOTICE_WAIT_S: u64 = 50;

/// Forwards the session's comment notices as Claude Code channel events
/// until the transport closes. Each poll stamps the notices it returns, so
/// no other follower announces them again. Daemon errors back off from 1 s
/// to 30 s. The loop never starts a daemon.
async fn forward_notices(client: DaemonClient, peer: rmcp::Peer<rmcp::RoleServer>) {
    use rmcp::model::{CustomNotification, ServerNotification};
    let mut backoff = Duration::from_secs(1);
    loop {
        match client.notices(NOTICE_WAIT_S).await {
            Ok(v) => {
                backoff = Duration::from_secs(1);
                let notices = v["notices"].as_array().cloned().unwrap_or_default();
                let lines = v["lines"].as_array().cloned().unwrap_or_default();
                for (n, line) in notices.iter().zip(lines.iter().filter_map(|l| l.as_str().map(str::to_string))) {
                    let event = CustomNotification::new(
                        crate::channel::METHOD,
                        Some(crate::channel::event_params(n, &line)),
                    );
                    if let Err(e) = peer.send_notification(ServerNotification::CustomNotification(event)).await {
                        tracing::info!("channel closed: {e}");
                        return;
                    }
                }
            }
            Err(e) => {
                tracing::debug!("notices poll failed: {e}");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(30));
            }
        }
    }
}
```

In `serve`, after `let service = tools.serve(stdio()).await…`, when forwarding:

```rust
    let forward = forward.then(|| tokio::spawn(forward_notices(client.clone(), service.peer().clone())));
```

Abort it after `service.waiting().await`, before the function returns. `serve` now takes `(tools, client, forward: bool)`.

`crates/clax-cli/src/commands/mcp.rs`. Under `Agent::Claude`, detect and log before starting the runtime:

```rust
    // SAFETY: getppid has no preconditions and cannot fail.
    let parent_pid = unsafe { libc::getppid() } as u32;
    let channel = matches!(a.agent, Agent::Claude).then(|| {
        let ch = clax_mcp::channel::ChannelState::detect(parent_pid);
        crate::hooklog::append(
            home,
            &format!(
                "{} channel agent=claude {}",
                chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                ch.log_fields(parent_pid)
            ),
        );
        ch
    });
```

and pass `channel` to `shim::run`. Add `libc` to `clax-cli`'s dependencies if it is not there (the workspace already uses it in `clax-mcp`). Update every other `shim::run` caller and test to pass `None`.

Run: `cargo test -p clax-mcp && cargo test -p clax-cli`. Expected: PASS, including `tests/shim.rs` unchanged. The shim tests' parent is the test binary, so their launch flag is `absent` and they never poll notices.

- [ ] **Step 5: Gates and stage**

Run the quality gates. Stage the Task 3 files.

Proposed commit message: `Declare the Claude Code channel in the shim and forward comment notices when the session was launched with it`

---

### Task 4: Plugin manifest, skill fallback and `clax doctor --agent claude`

**Files:**
- Modify: `plugins/claude-code/.claude-plugin/plugin.json`
- Modify: `plugins/claude-code/skills/clax/SKILL.md`
- Modify: `crates/clax-cli/src/commands/doctor_agent.rs`
- Modify: `scripts/test-plugins.sh`

**Interfaces:**
- Consumes: Task 3 (`status.push.channel`, `push.follow_command`, the `channel` line in `hooks.log`).
- Produces: `channel_check(manifest_root: Option<&Path>, home: &Home) -> Value`, which runs for `--agent claude` only.

- [ ] **Step 1: Write the failing checks**

In `scripts/test-plugins.sh`, next to the other manifest checks:

```bash
if python3 - plugins/claude-code/.claude-plugin/plugin.json <<'PY'
import json, sys
m = json.load(open(sys.argv[1]))
sys.exit(0 if m.get("channels") == [{"server": "clax"}] else 1)
PY
then pass "the Claude Code manifest declares the clax channel"; else fail "plugins/claude-code/.claude-plugin/plugin.json lacks channels: [{\"server\": \"clax\"}]"; fi

relay='claude/channel'"/permission"
if grep -rqF "$relay" crates plugins; then
    fail "permission relay must never be declared: $(grep -rlF "$relay" crates plugins | tr '\n' ' ')"
else
    pass "no permission relay"
fi
```

The relay string is assembled from two halves so the script does not match itself. The design text in docs is outside the two scanned directories. In `doctor_agent.rs` `mod tests`:

```rust
    #[test]
    fn channel_needs_the_manifest_entry_and_reports_the_last_launch() {
        let f = Fixture::new();
        let root = f.plugin("plugins/cache/clax/clax/0.3.0", ".claude-plugin/plugin.json", "0.3.0", DoctorAgent::Claude);
        // Without `channels` in the manifest: failed, says to reinstall.
        let v = channel_check(Some(&root), &f.clax_home());
        assert_eq!(v["ok"], false, "{v}");
        assert!(v["detail"].as_str().unwrap().contains("clax init"), "{v}");
        // With it, and no channel line: ok, gives the launch command.
        f.write_manifest_channels(&root);
        let v = channel_check(Some(&root), &f.clax_home());
        assert_eq!(v["ok"], true);
        assert!(v["detail"].as_str().unwrap().contains("--dangerously-load-development-channels plugin:clax@clax"), "{v}");
        // The latest channel line decides the text.
        crate::hooklog::append(&f.clax_home(), "2026-10-01T10:00:00Z channel agent=claude launch_flag=absent flag=\"\" entry=\"\" parent_pid=1");
        crate::hooklog::append(&f.clax_home(), "2026-10-01T10:05:00Z channel agent=claude launch_flag=present flag=\"--dangerously-load-development-channels\" entry=\"plugin:clax@clax\" parent_pid=2");
        crate::hooklog::append(&f.clax_home(), "2026-10-01T10:05:01Z hook agent=claude event=stop bin=/b/clax duration_ms=3 exit=0 stderr=\"\"");
        let d = channel_check(Some(&root), &f.clax_home())["detail"].as_str().unwrap().to_string();
        assert!(d.contains("2026-10-01T10:05:00Z") && d.contains("plugin:clax@clax") && d.contains("startup screen"), "{d}");
    }
```

Add the two `Fixture` helpers this needs (`clax_home`, `write_manifest_channels`) if the fixture lacks them. Run: `bash scripts/test-plugins.sh; cargo test -p clax-cli channel_needs`. Expected: FAIL.

- [ ] **Step 2: The manifest**

`plugins/claude-code/.claude-plugin/plugin.json` gains, after `keywords`:

```json
  "channels": [
    {
      "server": "clax"
    }
  ]
```

- [ ] **Step 3: The doctor check**

```rust
/// `channel` (Claude Code): whether the installed plugin declares the Clax
/// channel, and how the latest Claude Code session was launched (the
/// shim's `channel` line in hooks.log). The channel is opt-in, so only a
/// manifest without it fails.
pub fn channel_check(root: Option<&Path>, home: &Home) -> Value {
    let declared = root
        .and_then(|r| read_json(&r.join(".claude-plugin/plugin.json")))
        .is_some_and(|m| m["channels"] == json!([{"server": "clax"}]));
    if !declared {
        return check("channel", false,
            "the installed plugin does not declare the clax channel; run `clax init` to install the current plugin");
    }
    let launch = clax_mcp::channel::LAUNCH;
    let last = crate::hooklog::tail_for(home, "claude", 200)
        .into_iter()
        .rev()
        .find(|l| l.split_once(' ').is_some_and(|(_, rest)| rest.starts_with("channel ")));
    let detail = match last {
        None => format!("no Claude Code session has started the shim yet. To wake idle sessions through the channel, launch `{launch}`; without it the skill runs `clax feedback follow --once` in the background"),
        Some(l) if l.contains("launch_flag=present") => format!(
            "the latest session ({}) was launched with the channel: {l}. Claude Code does not tell Clax whether the channel registered; its startup screen says so. If it says the channel was blocked, relaunch without the flag to use the background fallback",
            l.split(' ').next().unwrap_or_default()),
        Some(l) => format!(
            "the latest session ({}) was launched without the channel ({l}); idle sessions wake through the skill's background `clax feedback follow --once`. For the channel, launch `{launch}` (research preview, CLI only, claude.ai or Console login; on Team and Enterprise an Owner must turn channels on)",
            l.split(' ').next().unwrap_or_default()),
    };
    check("channel", true, detail)
}
```

In `checks`, push `channel_check(root.as_deref(), home)` for `DoctorAgent::Claude` only, right after `feedback`. That needs `root` from the `plugin_check` branch, so keep it in a variable that is `None` when `HOME` is unset. `hooks_check` fails only on ` launcher ` lines, so a `channel` line never fails it. Add a test asserting that a latest `channel` line leaves `hooks` ok.

- [ ] **Step 4: The skill**

In `plugins/claude-code/skills/clax/SKILL.md`, replace the bullet `- Pushed into an idle session where the harness allows it and the watch has replies on (Codex through \`codex queue\`, Pi through the extension).` with:

```markdown
- Pushed into an idle session where the harness allows it and the watch has
  replies on (Codex through `codex queue`, Pi through the extension). In
  Claude Code an idle session is woken by a notice, which says only that a
  comment is waiting (see "Waking an idle session" below).
```

Add a section after the comment loop's `Tools:` list, before `When the person wants to iterate live`:

```markdown
### Waking an idle session

A notice is one line, either from the Clax channel as
`<channel source="plugin:clax:clax" artifact_id="…" thread_id="…">`, or
printed by `clax feedback follow`:

    [clax] New comment on "<title>" (<url>), thread <thread ID>. Call comments_read with url_or_id "<artifact ID>" and thread_id "<thread ID>" to read it; if you have already handled it, do nothing.

It does not contain the comment. Call `comments_read` as it says, then
handle the comment as above. If you have already handled that thread,
do nothing.

After your first `publish` (or `watch`) in a session, call `status` and
look at `push`:

- `tier` is `"channel"`: the session was launched with the Clax channel.
  Notices arrive on their own. Start nothing.
- `tier` is `"follow"`: a follower is already running. Start nothing.
- `tier` is null and `follow_command` is present: run `follow_command`
  with the Bash tool and `run_in_background: true`. It exits when a
  comment arrives, which wakes you, and its output is the notice lines.
  Handle each one, then run the same command again in the background.
  Keep exactly one running. Stop restarting it when the person says to
  stop watching for comments. If it exits with no output, the session has
  ended. Do not restart it.

Never start `follow_command` while `tier` is `"channel"`.
```

Run `python3 scripts/sync-skill-tools.py --check` (the tool block is unchanged) and `bash scripts/test-plugins.sh`.

- [ ] **Step 5: Gates and stage**

Run the quality gates. Stage the four files.

Proposed commit message: `Declare the clax channel in the Claude Code plugin, add the skill's background follow fallback, and report the channel in clax doctor --agent claude`

---

### Task 5: Smoke script, READMEs and follow-ups

**Files:**
- Create: `scripts/smoke-claude-push.sh`
- Modify: `README.md` ("Use from an agent")
- Modify: `plugins/claude-code/README.md` ("The comment loop")
- Modify: `docs/follow-ups.md` ("Checks only the repository owner can run")

**Interfaces:**
- Consumes: Tasks 2 to 4.
- Produces: the owner's live check.

- [ ] **Step 1: The smoke script**

`scripts/smoke-claude-push.sh` is interactive, because a channel needs an interactive `claude` session and so does a background command. It uses a scratch `CLAX_HOME` with its own daemon on a free port. It never uses 7480 or 7481. It relies on the owner's normal Claude Code login and the installed `clax` plugin, which `just install` puts in place.

```bash
#!/usr/bin/env bash
# Manual check of Claude Code tier 5 against real `claude`. Not a quality gate.
# Needs an interactive terminal, a logged-in `claude` (claude.ai or Console),
# and the clax plugin installed (`just install`). It uses a scratch CLAX_HOME and
# a daemon on a free port, never the owner's daemon.
#
# Modes:
#   --channel  launch with --dangerously-load-development-channels plugin:clax@clax;
#              an idle session must wake through the channel.
#   --follow   launch without it; the skill's background `clax feedback follow --once`
#              must wake the idle session.
#
# Usage: scripts/smoke-claude-push.sh --channel|--follow [scratch-dir]
set -euo pipefail
cd "$(dirname "$0")/.."
REPO="$PWD"
MODE="${1:-}"
case "$MODE" in --channel|--follow) shift ;; *) echo "usage: $0 --channel|--follow [scratch-dir]" >&2; exit 2 ;; esac
SCRATCH="${1:-${TMPDIR:-/tmp}/clax-smoke-claude-push}"
SCRATCH="$(mkdir -p "$SCRATCH" && cd "$SCRATCH" && pwd)"
export CLAX_HOME="$SCRATCH/home"
export CLAX_NO_OPEN=1
BIN="$REPO/target/debug/clax"
export CLAX_BIN="$BIN"
die() { echo "smoke: FAIL: $1" >&2; exit 1; }
cleanup() { "$BIN" stop >/dev/null 2>&1 || true; }
trap cleanup EXIT

command -v claude >/dev/null || die "claude is not on PATH"
cargo build --quiet -p clax-cli --bin clax
rm -rf "$CLAX_HOME"; mkdir -p "$CLAX_HOME" "$SCRATCH/cwd"
"$BIN" --port 0 serve --background >/dev/null || die "the scratch daemon did not start"
TOKEN="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["token"])' "$CLAX_HOME/daemon.json")"
BASE="$(python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print(d.get("url") or "http://127.0.0.1:%d" % d["port"])' "$CLAX_HOME/daemon.json")"
api() { curl -fsS -H "Authorization: Bearer $TOKEN" -H 'Content-Type: application/json' "$@"; }

FLAGS=""
[ "$MODE" = --channel ] && FLAGS="--dangerously-load-development-channels plugin:clax@clax"
cat <<EOF
In another terminal, run:

  cd '$SCRATCH/cwd' && CLAX_HOME='$CLAX_HOME' CLAX_BIN='$BIN' CLAX_NO_OPEN=1 claude $FLAGS

$( [ "$MODE" = --channel ] && echo "Accept the development-channels warning. The startup screen must say messages from plugin:clax@clax inject into the session." )
Then ask Claude: "Publish a page titled Push Smoke with one paragraph, then stop."
Do not type anything else in that session. Press Enter here once Claude has finished its turn.
EOF
read -r _

SID="$(api "$BASE/api/sessions?live=true" | python3 -c 'import json,sys; s=[x for x in json.load(sys.stdin)["sessions"] if x["harness"]=="claude"]; print(s[-1]["id"] if s else "")')"
[ -n "$SID" ] || die "no live Claude Code session registered"
AID="$(api "$BASE/api/artifacts" | python3 -c 'import json,sys; a=[x for x in json.load(sys.stdin)["artifacts"] if x["title"]=="Push Smoke"]; print(a[0]["id"] if a else "")')"
[ -n "$AID" ] || die "Claude did not publish Push Smoke"
# Opens a thread on index.html, comments and sends it to the agent: the same
# REST calls the shell makes. Adjust the bodies to the contract's thread API.
TID="$(api -X POST "$BASE/api/artifacts/$AID/threads" -d '{"anchor":{"file":"index.html","selector":"p"},"body":"Please add a second paragraph saying smoke ok."}' | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')"
api -X POST "$BASE/api/artifacts/$AID/threads/$TID/send" -d '{}' >/dev/null
echo "Comment sent at $(date +%T). Watch the Claude session: it must start a turn on its own."

for _ in $(seq 1 90); do
    STATE="$(api "$BASE/api/artifacts/$AID/threads/$TID" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("feedback_state",{}).get("state",""))')"
    if [ "$STATE" = acknowledged ]; then
        echo "smoke: PASS ($MODE): the idle session woke and read the comment"
        exit 0
    fi
    sleep 2
done
die "the comment was not acknowledged within 180 s; the idle session was not woken ($MODE)"
```

Before writing the REST calls, check each against `docs/contract.md` (the thread, send and session routes, and `daemon.json`'s fields), and correct the paths and bodies to match the contract. Run `bash -n scripts/smoke-claude-push.sh` and `shellcheck scripts/smoke-claude-push.sh`. Never run the script.

- [ ] **Step 2: READMEs and follow-ups**

`README.md`, "Use from an agent", append a paragraph:

```markdown
Comments wake an idle Codex or Pi session on their own. An idle Claude Code
session wakes in one of two ways. Launched with
`claude --dangerously-load-development-channels plugin:clax@clax` (Claude
Code channels, a research preview: CLI only, claude.ai or Console login,
and on Team and Enterprise an Owner must turn channels on), Clax sends it
a notice through the channel. Otherwise the skill keeps a background
`clax feedback follow --once` running after a publish, and its exit wakes
the session. A notice only points at the comment. The comment itself still
arrives once, through the next clax tool call, the end of the turn, or
`wait_for_feedback`. `status` and `clax doctor --agent claude` show which
path a session uses.
```

`plugins/claude-code/README.md`, "The comment loop": add the same facts in the README's own voice, with the launch command, and say that Clax never asks for permission relay, so commenters cannot approve tool use.

`docs/follow-ups.md`, "Checks only the repository owner can run": add

```markdown
- `scripts/smoke-claude-push.sh --channel` and `--follow`: an idle Claude
  Code session wakes on a comment through the channel and through the
  background follow fallback. Until the owner runs them, two facts are
  unverified live: that a channel event from `plugin:clax@clax` starts a
  turn, and that a background Bash command's exit starts one in an idle
  session.
```

- [ ] **Step 3: Gates and stage**

Run the quality gates. Stage the four files.

Proposed commit message: `Add the Claude Code push smoke check and document the channel and the background fallback`

---

## Steps for the owner

1. `just install`, so that the installed plugin carries `channels` and the shim declares the channel.
2. `scripts/smoke-claude-push.sh --channel`, then `scripts/smoke-claude-push.sh --follow`. Each prints PASS or FAIL.
3. If `--channel` fails while the startup screen showed the channel registered, record the Claude Code version and the debug log (`claude --debug …`, `~/.claude/debug/<session-id>.txt`) in `docs/follow-ups.md`.
