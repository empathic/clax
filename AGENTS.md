# Working on Clax as an agent

The README's "Developing Clax" section covers the loops, the gates and the
test layout. This file adds the rules an agent must follow in this
repository.

## Quality annotations: qualifier

This repository keeps findings, risks, deferred work, paths not taken and
questions for the owner in `.qual` files next to the code, written with
[qualifier](https://github.com/empathic/qualifier). Read its guide whole
before the first record of a session: `qualifier agents`, then every topic
it lists (`qualifier agents <topic>`). With the qualifier plugin installed,
run it through the command the plugin's session context gives.

- **Before editing a file**, run `qualifier threads <path>` and read what is
  open on it.
- **Record** what the next person to touch the code would want to know,
  pinned to the lines it concerns (`qualifier record concern
  path:start:end "…"`): a review finding left for later, an accepted risk
  (`waiver`), an option not taken (`alternative` with a `revisit:` tag).
  Not scratch notes, not what belongs in the commit message.
- **A `blocker`** is only a defect a user would hit before merge; its
  detail opens with a `Failure:` line naming the inputs that break.
- **A question for the owner** is a record tagged `status:needs-decision`;
  `qualifier threads --status needs-decision` lists them.
- **Close** only a record your own session wrote, or one your own commit
  fixed, with evidence (a passing test, or a command and its output) and
  `--ref git:<sha>`, in a separate commit. Propose every other close to
  the owner. `wontfix` and design decisions are the owner's.
- **Records stand alone:** cite repository paths, line ranges, record IDs
  and command output, never a brief, a prompt or a conversation.
- More than two writes go in one `qualifier record --stdin` batch, written
  to a file outside the repository and dry-run first.
- Before merging a branch, `qualifier diff main` lists what it added,
  closed or left drifted.

## Code

- No `unsafe`: the workspace lints forbid it (`unsafe_code = "forbid"` in
  `Cargo.toml`).
- Never create hard links, in code, tests or scripts.
- Tests are fast and load-tolerant: no sleeps standing in for a condition,
  no wall-clock budgets in functional tests (time is a perf gate's job),
  injected clocks for timing rules. The whole check suite stays under two
  minutes on a warm cache.
- Never wait on processes by `pgrep -f`/`pkill -f` patterns: use the PID
  you started, or an exit file, with a timeout.
- Doc comments and commit messages describe the contract or the change,
  never the conversation that produced it or a rating of its quality.
- Write "ID" in prose, never "id" (except as a literal symbol in code).

## Builds, tests and gates on macOS

- macOS assesses every new executable on its first run, one at a time.
  Never `cp` over an existing binary (it is killed on exec); give tests a
  binary kept by content: `CLAX_TEST_BIN="$(scripts/stable-bin.sh
  target/debug/clax target/clax-bin/debug)"`. Install test helper programs
  once (`clax-fake-exe` in Rust, `fake-exe.ts` in `web/e2e` and
  `plugins/pi/test`) rather than
  writing a fresh executable per run.
- While working, run the tests of the crates and packages you changed;
  run `scripts/quality_gates.sh` (or `just check`) once before the branch
  merges. A lane that fails in code you did not touch is rerun alone and
  both results reported.
- Run anything longer than a minute or two in the background, log it to a
  file inside your own worktree (`target/…`), never a shared scratch
  directory, and wait in bounded stretches.

## Homes, daemons and git

- Never run an unmerged build against the owner's `~/.clax` or its daemon
  on 7480. Use `just watch`/`just dev` (`~/.clax-dev`), or a scratch
  `CLAX_HOME` on a free port, and stop any daemon you start by its PID.
- Migrations are numbered contiguously in
  `crates/clax-core/src/store/migrations.rs`; a branch takes the next free
  number on main and renumbers if another branch lands first.
- Don't change shared git configuration (for example `rerere`), and don't
  use bare `git stash` across worktrees.
- Commits are signed by the owner. Agents commit with
  `git -c commit.gpgsign=false commit`; never push unsigned commits, and
  never file, close or comment on issues or pull requests unless asked.
