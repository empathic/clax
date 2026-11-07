# ClaxBot's workspace: a shell in an ephemeral Linux container

Status: built, and proved on one machine by `node demo/claxbot-workspace/run.ts`
against a private remote and a fake GitHub App. It has not yet run against
Daytona or live GitHub: `deploy/claxbot-workspace/README.md`, "First live run".

## 1. What it adds

ClaxBot is the agent. Its run loop, model calls, journal, credits and thread
replies are unchanged. This adds tools it can drive: a run may hold an
ephemeral Linux container with a repository checked out, run shell commands in
it, and propose the result as a pull request.

A report on a queue is a thread on an artifact, so a ClaxBot `Thread` run on
that thread is the request. The run reads the report, works in the container,
calls `clax.change.propose`, and the fix lands on the report with the pull
request's preview as its `verify_url`, which is what the reporter's overlay
offers as "Check it here".

```
 report (a thread) ──▶ ClaxBot run ── clax.shell.exec ─────▶ Workspace ─▶ ContainerRuntime
                            │                                    │          ├ Daytona
                            └────── clax.change.propose ────────▶│          └ Docker
                                                                 ├▶ host clone ─▶ push
                                                                 ├▶ forge ─▶ pull request
                                                                 └▶ preview ─▶ https://pr-<N>.<zone>
```

## 2. Layers

- `clax-bot` knows it may have an ephemeral Linux container, and nothing about
  what provides it: the operations, their journal entries, the prompt's rule,
  and the `Workspace` trait, beside `WebSource`. No runtime is named there or
  in anything a model reads.
- `clax-bot-workspace` implements `Workspace`. Its `ContainerRuntime` and
  `Container` traits are the hooks a runtime provides: create, attach, exec,
  destroy. Daytona and Docker implement them, `RuntimeConfig` picks one, and
  `runtime::conformance` is what any implementation passes.
- `clax-server` wires a workspace into a native daemon's deps, in one place,
  `driver_for_home`. A Worker has none of its own: with a workspace service
  (`crates/clax-bot-remote`, `crates/clax-worker/src/workspace.rs`) its runs
  have what that service's operator's file offers, and without one they have no
  `shell` in their shape. A queue's repository link is not served there: the
  service holds no store, and the panel says so (`RuntimeFacts::operator_only`).

## 3. Operations

`clax.shell.exec(command, {cwd?, timeout?})` answers `{exit_code, output}`. A
failing command is an answer; `shell_unavailable` means it did not run. The
first call makes the run's container and checks out the repository.

`clax.change.propose({title, summary})` answers `{pr, url, commit,
verify_url}`. `change_refused` says why a change may not leave.

Both are journaled with their answers, so a step executed again reads the
journal and does not repeat a command or a push. A crash can still fall between
an effect and its answer, so each call's intent is journaled before its effect,
at a place of its own. A step executed again that finds an intent and no answer
repeats nothing blindly: `shell.exec` answers `shell_interrupted`, for the model
to look at what the command left, and `change.propose` goes to the workspace
again with the same `Proposal::key`, which pushes nothing twice, and to
`record_fix`, which leaves a fix of ClaxBot's already waiting as it is.

A call may take as long as it takes: a step renews the run's claim however long
its script waits on a host call, the time counts against no script limit, and
one step is one step. The run's own time is looked at between steps and before
each workspace call, which is refused (`run_cap`) once it is up and has its
timeout cut to what is left; a call that outlasts it keeps its answer, and the
run stops at the cap after it. A workspace may wait for the preview in `propose`, or answer at once with
`verify_url` null and call the `PreviewSink` that `Workspace::previews` gave it,
which reaches `BotHost::preview_ready`, or `BotHost::preview_missed` when the wait
ends with none. Either applies only while the fix is the latest and the run's own:
a later run's fix has its own preview to wait for.

What a run gets is an `Offer`, asked of the workspace from the run and the queue
of its report: a shell, or a shell and a repository to change. A container is
useful with no repository, so `shell.exec` and the prompt's rule for it say
nothing of one; `change.propose` and its rule come only with a target. A run with
no offer has the prompt and API of a host with no workspace, byte for byte. That
it may hold confidential material is on the run (`bot_runs.confidential`, §4).

A thread's run with a repository has no page to work on, so its prompt leaves out
the draft's rules and the page contract. It is told to read the repository's own
instructions first. A target may add `Offer::guidance`, its operator's words on
working there, and `Offer::preview_gate`, what a person does to deploy a preview,
which a held fix's note and the note that the wait ended repeat with the full SHA.

A run that has begun, which is to say holds a container, keeps its offer until it
ends, whatever becomes of its target: the workspace's ledger holds the offer, so a
restart keeps it too, and a call whose intent is journaled goes on to its end
whatever the offer says by then. New runs, and a run that had not reached its first
call, take the target as it is. One thing does reach a run that has begun: a target
that now says its material is confidential says so of the run, usable or not, and
nothing loosens it. Credentials are held in memory alone, so a run that lost them, to
a restart or a container that stopped answering, opens its target again; with the
target gone, or standing on another repository than the one the ledger recorded
(`Entry::repository`: the forge and both URLs), it keeps its answers and its shell and
can publish nothing more, and is told what stands on its branch.

A report's repository is for the people of its queue, so a run acting for anyone
else gets no shell, whatever the workspace would give. A driver that starts
hands the workspace the runs that may go on (`Workspace::reap`), so a crash
between a run's end and its container's leaks nothing.

That rule is `Store::run_reads_queue`, asked at every call, not only at the
run's start: the owner or a person still connected, with a role in the queue,
and, when an agent asked for the run, a live grant that holds Comments `read`, as
the queue's own routes ask of a member's agent. A reporter or a service grant is
no person and starts no run.

`change.propose` on a report also proposes the fix there
(`BotHost::record_fix`), which is what tells the reporter. What the report could
not take is refused first (`BotHost::check_fix`: it is closed, or someone's claim
holds it), so nothing is pushed for it; a fix that fails to record after the push
leaves `fix_recorded: false` in the answer and a notice, so the pull request stays
in sight. The fix is an agent's, kept with the run that made it
(`ReportWriter::Run`, `report_fixes.proposer_run_id`), recorded as ClaxBot acting
for the run's person, and `Proposal::key` makes one proposal one attempt. A fix
still waiting is superseded: a verdict is on what the reporter was shown.

A preview outlives the run that waits for it. `Workspace::propose` answers at once,
and what is told of the preview is held until the fix is on the report, or will not
be, however the call ended, a Stop that drops it included. The ledger keeps a wait
until its time is up, through a finished run's reap too, so a restart takes it up, and
a proposal answered again from the ledger takes it up as well: what was told before
its fix existed went nowhere. Who may read the preview goes by what the run was
offered, not by what its target says by then.

ClaxBot takes no feedback rows. `report_follow_up` is asked at every change of a
report, every run's end and a driver's start, and only for a queue whose target
offers a change. `docs/feedback.md` has its rules: a reopen starts a run, a reply
only where the target opts in (`Offer::wake_on_reply`), never past a person's
Stop, a claim or a refusal, at most three times since a person last acted, and
always for the person and agent of the run that proposed the fix. A run a reopen
starts begins from the branch, `claxbot/<thread>`, which is the report's and not
the run's, while that branch has an open pull request. A pull request merged or
closed finishes its branch: a later run starts from the target's current base on
the next of `claxbot/<thread>-N` and opens a new pull request. The branch of a run
is in the ledger. A run a reply starts may answer and may not propose: its reporter
is checking the code as it stands. The thread is the run's report: a request that
names a plain thread and a report leads with the report (`start_run_woken`), so the
queue, the branch and the fix all stand on the one thread.

## 4. Trust boundaries

A report is written by whoever the product lets file one, and it reaches a
model driving a shell. So the container's contents are treated as hostile by
the time the run finishes.

- **The container holds no write credential and no model key.** The run loop
  stays on the host; only commands cross.
- **The read token crosses once**, in the `envs` of the clone command, as
  `http.extraHeader`, so it is not written to `.git/config`, before any model
  output runs. An App's is minted for that one clone, `contents: read` on that
  one repository, and revoked when the clone returns.
- **The host pushes.** A binary diff comes out and is applied to the host's
  own clone; paths and modes are read from that git, never from the container.
- **Protected paths.** Every target protects `.github/`: on this repository a
  pull request from a branch runs in the `preview` environment with its
  secrets (`.github/workflows/preview.yml`). It protects `.gitmodules`, and
  `.gitattributes` and `.lfsconfig` at any depth, which change what git does with
  a file. Symlinks and submodules are refused for the same reason, and so is any
  mode in a patch's header that is no plain file's, since `git apply` takes what
  the header says.
- **Nothing merges.** There is no merge call. A person reviews.
- **A run does not give its own verdict.** `threads.resolve` is refused
  (`fix_waiting`) on a report whose fix waits: closed, it would take no verdict,
  and no person's fix either.
- **A reporter is not told what the repository holds.** With a private target
  the model reads the source while following a reporter's words, and what it
  writes on the report reaches that reporter: a way out with no network at all.
  So an offer may be `confidential`, and then the run loop and its host limit
  what leaves, whatever the model is told or asked. The run keeps it
  (`bot_runs.confidential`, set at its first tick, never cleared), so a target
  edited out, a workspace gone from the host or a failed lookup cannot lift it.
  A run that touched a workspace which no longer answers for it is confidential.
  So is a run on a thread where an earlier run was, or that holds a person's note
  for the queue's members, on any queue: a run of the queue's reads those notes.
  A note a run wrote says no more than that run could. A target that turns
  confidential during a turn marks the run, and the tick's copy, before the
  workspace call's first effect. A failed lookup, the store's or the target's
  (`Offer::unsure`), reads as yes for that tick, and is kept only for a run that
  touched one: a run that touched none holds nothing. A mark that cannot be
  written fails the tick.
  - `OpCtx::members_only` is set on every reply and notice (`threads.reply`,
    `finish`, `give_up`, a stop's notice with its file names, the sources list).
    Each is a note for the queue's members (`triage_only`): on the report for
    them and for a run that may read the queue, and in no thread view, event,
    inbox or reporter's view, and the audit trail records that it was written
    and not what it says.
  - The fix says a fixed message and carries no summary, pull request link or
    commit; the members' note beside it, written in the fix's transaction, has
    those. Its `verify_url` is a URL the workspace built, and is on the fix only
    where `Offer::reviewed_previews` says each deploy needs a person's approval
    of the commit: a preview is built from what the model wrote, so it is a way
    out until a person has looked. With no link the fix is held
    (`report_fixes.held`): it says a fix has been proposed and takes no verdict
    of the reporter's, who has nothing to check. A link ends that, as a person's
    own fix does by superseding it, which needs a `verify_url` and stays with the
    held fix's run, so a reopen wakes ClaxBot as on any of its fixes. A public
    target's fix with no link yet is held the same way, its words in the same note;
    there the preview's link is the reporter's when it comes, and only the link: the
    fixed message stands, and the words stay in the note. A preview no person let
    out is kept on the fix for the queue's members (`preview_url`, never served to a
    reporter), and a wait that ends with none is kept too (`preview_missed`) and
    leaves them a note in fixed words.
  - `draft.publish`, `artifacts.create`, `db.*` writes and `web.*` are refused
    (`confidential`), and the prompt declares no web calls.
  - The run itself is for the queue. `GET /api/bot/runs` and its siblings show
    anyone with no role there its status alone, the instance's admin and the
    artifact's maker included, and its audit records keep no title, error or
    target. Its reporter's reply gets a fixed acknowledgement.
  - It holds for a run that lost its shell: what it read is in its context.

  The summary and title of `change.propose` still go to the pull request. That
  is the repository, whose people may read the source already, not the
  reporter.

- **Where a person's addresses may go.** A store link's URLs are a person's,
  so unless its owner may reach their own machine (`local_forge`,
  `local_preview`) they are held to public addresses at the connection, by
  `crates/clax-bot-workspace/src/net.rs`:
  - The forge's and a preview's clients resolve in `PublicOnly`, connect only
    to the public answers and follow no redirect. A later answer cannot change
    that: the check is the connection's own lookup.
  - Host-side `git` resolves for itself, so each command is resolved and
    checked first, and pinned to those answers with
    `-c http.curloptResolve=host:port:addr`, with `http.followRedirects=false`.
    That needs a git of 2.37 or later; an older one ignores the setting and is
    left with the check before it, and a small race.
  - **A container's clone is none of this.** It runs on the runtime's network,
    not the daemon's. What holds it is the runtime's own egress. On a third
    party's runtime it cannot reach the daemon's machine or network at all; on
    Docker it has whatever the engine's bridge gives it, which can include the
    host. It carries one read token, for one repository, ended when it returns.
  - A proxy in the daemon's environment resolves names itself, past all of
    this: an operator who sets one is trusting it with the rule.

## 5. Runtimes

Daytona, from `https://www.daytona.io/docs/openapi.json` and
`toolbox-openapi.json`: `POST /sandbox`, `GET` and `DELETE /sandbox/{id}`, and
`POST {toolboxProxyUrl}/{id}/process/execute` with `{command, cwd, envs,
timeout}`, answering `{exitCode, result}`. Its `timeout` defaults to 10 seconds
and `command` is not given to a shell, so the client always names a timeout and
sends `sh -c 'echo <base64> | base64 -d | sh'`. Those details stay in
`daytona.rs`.

Docker is the `docker` command: a machine's own containers, and the second
implementation that keeps the seam honest.

`ContainerRuntime::third_party` says whether anyone but this daemon's operator
runs what holds a container, and it answers yes unless a runtime knows better.
Docker asks `docker context inspect` for its engine's endpoint, so a `docker`
pointed at another machine answers yes. A person connecting a repository reads
this answer: the daemon computes it once at boot from the workspace it built and
keeps it (`clax_http::bot::RuntimeFacts`), and no route reads `workspace.toml`.
A file that is there and failed to load says so, and never that nothing is
cloned.

## 6. Turning it on

A daemon with `<home>/workspace.toml` has a workspace: `[runtime]` picks the
runtime, `[[target]]` maps a queue to a repository, and `shell_for_all` gives
every other run a bare container. Without the file nothing changes, and a
repository a queue's admin connected stays dormant: there is no runtime for it,
and the panel says so.

`node demo/claxbot-workspace/run.ts` runs the loop on one machine, on the Docker
runtime, with a model through `claude -p`, and proves the private route: a local
forge demanding credentials for every git and API call, with a fake GitHub App,
configured through Clax's HTTP API, and it fails unless the run keeps every
promise of section 4. `--public` runs a public repository an operator's file
names.

## 7. Targets

A queue's target comes from one of two places, and the operator's wins:

- `workspace.toml`, at boot. An operator's targets, credentials from
  environment variables.
- A queue admin's link in the store (`crates/clax-store/src/queue_repos.rs`),
  looked up per run through `TargetSource`. `describe` answers from metadata,
  opening nothing, for the synchronous `Workspace::offer`; `open` opens the
  credentials when a run needs them, and the run holds that target until it
  ends. A link gets the runtime and spec of the file: its egress is what they do
  (`Egress`), and a confidential link is offered nowhere else.

| Link | Access | Forge | Commits |
|---|---|---|---|
| `github_app` | `GitHubAppAccess`, minting per use | `GitHub::committing`, through the Git Data API | signed by GitHub, `UnsignedCommits::Refuse` |
| `token` | `StaticAccess` | `git push`, pull requests by API | unsigned, `UnsignedCommits::Allow`: a repository that requires signatures refuses the push, and the panel says so beside the token |

A link's `status` says what it waits for. Besides `ready` and the owner's
`needs_approval`, a private link on token access with no read token is
`needs_read_token`, and a private link whose queue admin has not set `ci_accepted`,
which says the repository's CI may run ClaxBot's branch with that CI's secrets and
network before anyone reviews it, is `needs_ci_acceptance`. A change that rebinds
the repository, its URLs or the installation stores `ci_accepted` false whatever it
was sent with. Clax cannot see what a repository's CI runs, so nothing looks.

What became of a link's credentials is told with the stamp of the credentials the
run opened (`ReadyTarget::stamp`), so a run on old ones says nothing of new ones.
Only a publish sets `last_ok_at`: a clone works with a token that cannot write, and
clears what a clone failed at and not `PUBLISH_FAILED`.

A link is confidential unless `public`, and adds its protected paths to
`Policy`'s defaults. Clearing a token, disconnecting, or the owner withdrawing an
approval stops new runs and takes nothing from a run that holds its target. A
lookup that fails leaves a run confidential, with no shell. What became of the
credentials goes back to the link: a real clone or commit marks them as having
worked, and a failure shows on the panel in fixed words, never a remote's own.

An App link's installation is never typed. `PUT /api/queues/<qid>/repo` asks the
forge with the App's JWT (`GitHubApp::installation_of`) and keeps the account it
names, which is what the owner approves by. A repository the App is not on is
refused with the install link.

A target names the repository, image, test command, preview template and
protected paths. Clax's preview is `https://pr-{pr}.<PREVIEW_HOST_ZONE>`, and
`/healthz` names the build's commit, so a stale deployment is not mistaken for
this one. Changelist's and Pathbase's are `https://<app>-pr-{pr}.fly.dev`.
