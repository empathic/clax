//! ClaxBot's ephemeral Linux container (spec
//! `docs/superpowers/specs/2026-11-06-claxbot-workspace-design.md`):
//! `clax.shell.exec`, `clax.change.propose`, and [`Workspace`], the host's
//! side of them. What provides the container is the host's business.
//!
//! As with [`crate::web::WebSource`], the run loop keeps the argument
//! checks, the journal and the rule of who may call; a host provides only a
//! [`Workspace`]. A run's calls answer from the journal when a step is
//! executed again, so a command is not run twice by a replay.
//!
//! A run has one container, made at its first call and ended by
//! [`Workspace::finish`] when the run ends. [`Offer`] says what a run gets:
//! a shell is useful alone, and a repository to change is a further offer
//! on top of it. The container holds no credential; for a run that may
//! propose a change, what leaves it is a patch, which the [`Workspace`]
//! checks and pushes itself in [`Workspace::propose`].
//!
//! A workspace may hold material a report's reporter must not read
//! ([`Offer::confidential`]). The model reads it while following the
//! reporter's words, so what the run writes is limited by the run loop and
//! its host, never by a rule the model is asked to keep: every reply and
//! notice goes to the queue's members alone ([`crate::host::OpCtx`]), a
//! fix says only [`FIX_PROPOSED`], with its preview's link where a person
//! approves previews and otherwise held ([`Offer::fix`]), and the calls that
//! carry text elsewhere are refused
//! ([`CODE_CONFIDENTIAL`], [`refused_when_confidential`]).
//!
//! No reporter is asked about a fix until there is something to check, on a
//! public target too: a fix with no link is held, and says [`FIX_PROPOSED`].
//! Its preview's link reaches the reporter when it comes up
//! ([`PreviewSink::preview_ready`]); if the wait ends with none seen the
//! queue's members are told ([`PreviewSink::preview_missed`]).
//!
//! A host whose tick has a budget shorter than a command's timeout (the
//! Worker's alarm) offers a [`Workspace::resumable`] one: its calls go on
//! elsewhere while the run does not wait. The run loop gives such a call up
//! when the tick's time is out, leaves the step open ([`CODE_YIELDED`]), and
//! joins it again in the next tick: [`Workspace::resume_exec`] under the
//! key of [`Workspace::exec_keyed`], [`Workspace::propose`] under
//! [`Proposal::key`].

use crate::host::ProposedFix;
use clax_bot_sandbox::{HostError, MaybeSendSync};
use clax_store::bot_runs::Run;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

pub const OP_EXEC: &str = "shell.exec";
pub const OP_PROPOSE: &str = "change.propose";

/// No workspace on this host, or none for this run.
pub const CODE_SHELL_DISABLED: &str = "shell_disabled";
/// The container could not run the command; a command that fails is an
/// answer with its exit code, not this.
pub const CODE_SHELL_UNAVAILABLE: &str = "shell_unavailable";
/// The change may not be pushed: it touches a protected path, or nothing.
/// A proposal answered with this left nothing, so a run's words go on to its
/// reporter ([`Workspace::propose`] says when it may be answered).
pub const CODE_CHANGE_REFUSED: &str = "change_refused";
/// A step executed again found this command begun and never answered: it
/// may have run, whole or in part, and the container may be another one.
pub const CODE_SHELL_INTERRUPTED: &str = "shell_interrupted";
/// The tick's time ran out while a [`Workspace::resumable`] call went on:
/// the step stays open and joins the call in the next tick. A script that
/// catches this gets nothing more from this attempt, and what it returns is
/// not kept.
pub const CODE_YIELDED: &str = "yielded";
/// A preview's word came for a run still between a proposal and its fix: ask
/// again ([`PreviewSink`]).
pub const CODE_FIX_COMING: &str = "fix_coming";
/// The run's time is up: no more workspace calls.
pub const CODE_RUN_CAP: &str = "run_cap";
/// A run holding confidential material may not make this call.
pub const CODE_CONFIDENTIAL: &str = "confidential";

/// What a reporter reads of any fix from a run, whatever the target and
/// whether or not a preview is up: nothing the model wrote and no pull
/// request. With no link for them the fix is held: none yet, or, from a
/// confidential run, one no person has approved, which stays with the queue's
/// members. The words hold when a link comes, as what a reporter acts on
/// ([`PREVIEW_READY`]).
pub const FIX_PROPOSED: &str = "A fix has been proposed.";
/// What a reporter reads after a reply a confidential run cannot answer.
pub const REPLY_RECEIVED: &str = "Thank you. The people of this queue will read this.";
/// What the queue's members read when a report's automatic runs stop.
pub const AUTO_RUNS_STOPPED: &str = "ClaxBot stopped following this report on its own: it ran \
     the most times it may without a person. Send it to ClaxBot to go on.";
/// What the queue's members read when the wait for a held fix's preview ends
/// with none seen: its reporter has no link, and hears nothing. It says what
/// ClaxBot did and not that no preview is up, which it cannot know: its last
/// look may have failed, its target may have gone, or the link may be on no
/// host it allows.
pub const PREVIEW_MISSED: &str = "ClaxBot stopped watching for this fix's preview without seeing \
     it up, so its reporter has no link to check and has not been asked. Look at its pull \
     request. To hand it over, propose a fix of your own with a link to check.";
/// The first of what [`Offer::preview_missed`] says.
const PREVIEW_NOT_UP: &str = "ClaxBot stopped watching for this fix's preview without seeing it \
     up, so its reporter has no link to check and has not been asked.";
/// What the queue's members read when a step ends with a proposal begun and
/// unanswered, or a run comes back to one the report may no longer take.
pub const PROPOSAL_STRANDED: &str = "ClaxBot was interrupted while proposing a change, and no \
     fix was recorded here. Its branch or pull request may be in the repository with nothing \
     here pointing at it.";
/// What the queue's members read when word of a fix's preview could not be
/// told, however often it was tried.
pub const PREVIEW_LOST: &str = "Word of this fix's preview could not be recorded here, so its \
     reporter may have nothing to check. Look at its pull request.";
/// What a run reads when a reply of its script went to the queue's people
/// alone ([`crate::host::OpCtx::members_only`]).
pub const REPLY_KEPT: &str = "Your reply reached the queue's people only: the report's reporter \
     did not get it and will not. The host tells the reporter what they need.";
/// What a run reads when a pull request of a public repository would carry
/// what the queue's members were told.
pub const PROPOSAL_PUBLIC: &str = "this run holds confidential material and this repository is \
     public, so a pull request would publish it. Nothing was pushed, and proposing again will \
     not change that. Tell the queue's members what you found with clax.threads.reply.";
/// What the queue's members read when that happens.
pub const PROPOSAL_WITHHELD: &str = "ClaxBot made a change here and did not propose it: this \
     run read the queue's notes, and its repository is public, so a pull request would have \
     published what it read. Its reply to you says what it found; a person can make the change.";
/// What a reporter reads when a fix's preview comes up after the fix, before
/// its link ([`crate::host::BotHost::preview_ready`]).
pub const PREVIEW_READY: &str = "The preview of this fix is ready:";

/// The most an [`Offer::guidance`] may hold: it is in every turn's prompt.
pub const MAX_GUIDANCE_BYTES: usize = 4 * 1024;
/// The most an [`Offer::preview_gate`] may hold.
pub const MAX_PREVIEW_GATE_BYTES: usize = 256;
/// What an [`Offer::preview_gate`] writes for the commit's full SHA.
pub const COMMIT_MARK: &str = "{commit}";

/// The least of a tick a resumable call is begun with: less, and it waits for
/// the next tick, unless the run's own time is what is short.
pub const MIN_CALL_SLACK: std::time::Duration = std::time::Duration::from_secs(5);

/// Seconds a command gets by default, and at most.
pub const DEFAULT_TIMEOUT_SECS: u64 = 120;
pub const MAX_TIMEOUT_SECS: u64 = 600;

/// The most of a command's output a script reads.
pub const MAX_OUTPUT_BYTES: usize = 32 * 1024;

/// A change to propose.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proposal {
    /// The pull request's title.
    pub title: String,
    /// Plain text: what was wrong and what changed. It is the pull
    /// request's body, which reaches the repository's people; a reporter
    /// reads it only from a run whose offer is not
    /// [`Offer::confidential`], on a fix with a preview to check.
    pub summary: String,
    /// The same on every attempt of this call: a step executed again sends
    /// it again ([`Workspace::propose`]).
    pub key: String,
}

/// What a run gets of a [`Workspace`]. The default is nothing, and a run
/// with nothing has the prompt and API of a host with no workspace.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct Offer {
    /// A container: `clax.shell.exec`.
    pub shell: bool,
    /// A repository checked out in it, and `clax.change.propose`.
    pub change: bool,
    /// What the container holds is not for a report's reporter (see the
    /// module documentation). This stays true of a run that lost its right
    /// to the shell: what it read is in its context.
    pub confidential: bool,
    /// The target's previews deploy only once a person has approved the
    /// commit, so a preview that is up is one a person let out, and its
    /// link may go to a reporter. Without it, a confidential target's
    /// preview link stays with the queue's members.
    pub reviewed_previews: bool,
    /// A reporter's plain reply to a waiting fix starts a run that may
    /// answer it. Without it only a reopen starts a run.
    pub wake_on_reply: bool,
    /// No reporter's word starts a run on this target: not a reopen, and not
    /// a reply, whatever [`Offer::wake_on_reply`] says. A person sends each
    /// run. False is the default, so an offer kept from before this was known
    /// reads as it always did.
    pub no_follow_ups: bool,
    /// Set by the run loop for a run a reply started: it may read and
    /// answer, and may not change what the reporter is checking, so no
    /// `change` comes with it.
    pub answers_only: bool,
    /// What the target says could not be read. Such an offer gives nothing
    /// and reads as confidential for now, and says nothing lasting of the
    /// run ([`crate::run::Deps::confidential`]).
    pub unsure: bool,
    /// How to work in the target's repository, in its operator's words:
    /// its build, its tests, how its commit titles read. A run that works
    /// on the repository reads it in its system prompt, so only an operator
    /// writes it. At most [`MAX_GUIDANCE_BYTES`]; the prompt leaves out more.
    pub guidance: Option<String>,
    /// What a person does to deploy the preview of a commit, where the
    /// target's previews wait for one: a clause in its operator's words that
    /// follows "To deploy its preview, ", with [`COMMIT_MARK`] for the
    /// commit's full SHA. At most [`MAX_PREVIEW_GATE_BYTES`].
    pub preview_gate: Option<String>,
}

impl Offer {
    pub const SHELL: Offer = Offer {
        shell: true,
        change: false,
        confidential: false,
        reviewed_previews: false,
        wake_on_reply: false,
        no_follow_ups: false,
        answers_only: false,
        unsure: false,
        guidance: None,
        preview_gate: None,
    };
    pub const CHANGE: Offer = Offer {
        shell: true,
        change: true,
        confidential: false,
        reviewed_previews: false,
        wake_on_reply: false,
        no_follow_ups: false,
        answers_only: false,
        unsure: false,
        guidance: None,
        preview_gate: None,
    };

    /// What a run gets when its target could not be read.
    #[must_use]
    pub const fn unsure() -> Offer {
        Offer {
            shell: false,
            change: false,
            confidential: true,
            reviewed_previews: false,
            wake_on_reply: false,
            no_follow_ups: false,
            answers_only: false,
            unsure: true,
            guidance: None,
            preview_gate: None,
        }
    }

    /// This offer, of confidential material.
    #[must_use]
    pub fn confidential(self) -> Offer {
        Offer {
            confidential: true,
            ..self
        }
    }

    /// This offer, of a target whose previews a person approves.
    #[must_use]
    pub fn reviewed_previews(self) -> Offer {
        Offer {
            reviewed_previews: true,
            ..self
        }
    }

    /// This offer, of a target whose reporters' replies start runs.
    #[must_use]
    pub fn wake_on_reply(self) -> Offer {
        Offer {
            wake_on_reply: true,
            ..self
        }
    }

    /// What a person does to deploy the preview of `commit`, in a sentence,
    /// where the target says and `commit` is a full SHA.
    fn preview_step(&self, commit: Option<&str>) -> Option<String> {
        let gate = self.preview_gate.as_deref();
        let gate = gate.filter(|g| g.len() <= MAX_PREVIEW_GATE_BYTES)?;
        let commit =
            commit.filter(|c| c.len() >= 40 && c.bytes().all(|b| b.is_ascii_hexdigit()))?;
        let gate = gate.replace(COMMIT_MARK, commit);
        Some(format!("To deploy its preview, {gate}."))
    }

    /// What the queue's members read when the wait for the preview of `commit`
    /// ended with none seen: [`PREVIEW_MISSED`], or, where the target says what a
    /// person does ([`Offer::preview_gate`]), that. No word of a preview that
    /// comes up after this reaches anyone, so a person hands it over.
    #[must_use]
    pub fn preview_missed(&self, commit: Option<&str>) -> String {
        self.preview_step(commit).map_or_else(
            || PREVIEW_MISSED.to_string(),
            |step| {
                format!(
                    "{PREVIEW_NOT_UP} {step} Once it is up, nothing tells its reporter: to hand \
                     it over, propose a fix of your own with its link."
                )
            },
        )
    }

    /// What a run with this offer proposes on its report, for `proposal`
    /// and `done`, the answer of [`Workspace::propose`]. Any fix is the fixed
    /// message [`FIX_PROPOSED`]: no text of the model's, which the report's
    /// words can steer, and no link into the repository reaches a reporter;
    /// the queue's members read them in [`ProposedFix::members_note`]. A
    /// preview's link does reach them, though from a confidential run only
    /// where a person approves what a preview serves
    /// ([`Offer::reviewed_previews`]), since a preview is built from what the
    /// model wrote. With no link the fix is held, and a preview it does have
    /// stays with those members ([`ProposedFix::preview_url`]).
    #[must_use]
    pub fn fix(&self, proposal: &Proposal, done: &Value) -> ProposedFix {
        let text = |k: &str| done[k].as_str().map(str::to_string);
        let verify_url = text("verify_url");
        let mut note = format!("{}\n\n{}", proposal.title, proposal.summary);
        for (name, v) in [
            ("Pull request", "url"),
            ("Commit", "commit"),
            ("Preview", "verify_url"),
        ] {
            if let Some(t) = done[v].as_str() {
                note.push_str(&format!("\n{name}: {t}"));
            }
        }
        let step = verify_url
            .is_none()
            .then(|| self.preview_step(done["commit"].as_str()))
            .flatten();
        if let Some(step) = &step {
            note.push_str(&format!("\n{step}"));
        }
        let (verify_url, preview_url) = if self.reviewed_previews || !self.confidential {
            (verify_url, None)
        } else {
            (None, verify_url)
        };
        ProposedFix {
            summary: FIX_PROPOSED.to_string(),
            url: None,
            commit: None,
            held: verify_url.is_none(),
            verify_url,
            key: proposal.key.clone(),
            members_note: Some(note),
            preview_url,
            pull_request: text("url"),
            pushed_commit: text("commit"),
            preview_step: step,
        }
    }
}

/// Whether a run holding confidential material is refused `op`: the calls
/// whose text reaches a page's viewers or a third party. `threads.reply`
/// is not one: it reaches only the queue's members.
#[must_use]
pub fn refused_when_confidential(op: &str) -> bool {
    matches!(
        op,
        "draft.publish"
            | "artifacts.create"
            | "db.set"
            | "db.update"
            | "db.delete"
            | crate::web::OP_SEARCH
            | crate::web::OP_FETCH
    )
}

/// Told when a fix's preview comes up, for a [`Workspace`] whose
/// [`Workspace::propose`] answers before it does. Made by
/// [`crate::run::Deps::with_workspace`] over the run's host.
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait PreviewSink: MaybeSendSync {
    /// The preview of the fix run `run` proposed serves it at `verify_url`.
    /// `reviewed` is [`Offer::reviewed_previews`] of what the run was offered,
    /// which the target may have changed from since. The run may have ended.
    /// Called again it adds nothing. A word about a run whose proposal on a
    /// [`Workspace::resumable`] workspace is begun and unanswered is refused
    /// with [`CODE_FIX_COMING`], here and in
    /// [`PreviewSink::preview_missed`]: its fix is not there yet to take it,
    /// so the workspace asks again.
    async fn preview_ready(
        &self,
        run: &str,
        verify_url: &str,
        reviewed: bool,
    ) -> Result<(), HostError>;

    /// The wait for the preview of the fix run `run` proposed ended with
    /// none seen, which does not say none is up. Called again it adds
    /// nothing.
    async fn preview_missed(&self, _run: &str) -> Result<(), HostError> {
        Ok(())
    }

    /// Word of the preview of the fix run `run` proposed could not be told
    /// and will not be tried again. Called again it adds nothing.
    async fn preview_lost(&self, _run: &str) -> Result<(), HostError> {
        Ok(())
    }
}

/// A container for each run, and the way a change leaves it.
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
pub trait Workspace: MaybeSendSync {
    /// What run `run`, on a report of `queue` if it is one, gets. A run that
    /// has begun, holding a container, keeps what it got until it ends,
    /// whatever becomes of its target, but for [`Offer::confidential`], which
    /// a target that now says so turns on and nothing turns off. A run that
    /// has not begun, or has ended, gets what the target says now, so a target
    /// gone stops new runs and a run that had not reached its first call. The
    /// prompt is written from it once. [`Offer::unsure`] when the target could
    /// not be read.
    fn offer(&self, run: &Run, queue: Option<&str>) -> Offer;

    /// Whether `run`'s target says for certain that what it holds is private,
    /// so that a pull request of it reaches nobody a report's words should
    /// not. Apart from [`Workspace::offer`], where not knowing reads as
    /// confidential: here not knowing is false, and a caller publishing
    /// nothing it cannot be sure of takes that as public.
    fn target_private(&self, run: &Run, queue: Option<&str>) -> bool {
        let offer = self.offer(run, queue);
        offer.confidential && !offer.unsure
    }

    /// Runs `command` in the run's container, making it on the first call;
    /// answers `{exit_code, output}`.
    async fn exec(
        &self,
        run: &Run,
        queue: Option<&str>,
        command: &str,
        cwd: Option<&str>,
        timeout_secs: u64,
    ) -> Result<Value, HostError>;

    /// Whether a call may be dropped while it goes on elsewhere. The run loop
    /// then bounds [`Workspace::exec_keyed`], [`Workspace::resume_exec`] and
    /// [`Workspace::propose`] by what is left of the tick, and joins them in
    /// the next one; otherwise it waits for each to its end, however long.
    /// What such a call answers is final, so it answers only with the
    /// outcome: not being able to reach what runs a begun call is no answer,
    /// and the call waits, to be dropped.
    fn resumable(&self) -> bool {
        false
    }

    /// [`Workspace::exec`] under `key`, the same on every attempt of this
    /// call, which is what the run loop calls. A resumable workspace goes on
    /// with the command if this is dropped, and a key seen before joins it
    /// rather than running it again. Others need no key.
    async fn exec_keyed(
        &self,
        run: &Run,
        queue: Option<&str>,
        _key: &str,
        command: &str,
        cwd: Option<&str>,
        timeout_secs: u64,
    ) -> Result<Value, HostError> {
        self.exec(run, queue, command, cwd, timeout_secs).await
    }

    /// Joins the command begun under `key`, waiting for it, and answers as
    /// [`Workspace::exec`] does. `None` when no such command is known, begun
    /// or ended: it never starts one, so the run loop answers
    /// [`CODE_SHELL_INTERRUPTED`]. A workspace that is not resumable knows
    /// none.
    async fn resume_exec(&self, _run: &Run, _key: &str) -> Result<Option<Value>, HostError> {
        Ok(None)
    }

    /// Takes the container's change, pushes it and opens or updates the
    /// pull request; answers `{pr, url, commit, verify_url}`, `verify_url`
    /// null while the preview is not up.
    ///
    /// A step executed again after a crash calls this again with the same
    /// [`Proposal::key`] when it cannot tell how far the first call went:
    /// a key seen before pushes nothing more and answers with the pull
    /// request it made, so one branch holds one change.
    ///
    /// [`CODE_CHANGE_REFUSED`] says that this call pushed nothing, and nothing
    /// else does: an answer that cannot say so, as when a push may have
    /// landed or the proposal was sent again, has another code. Whatever is
    /// not that keeps the run's words from its reporter, as a change may be
    /// out.
    ///
    /// This may wait for the preview, as long as it likes: the run keeps
    /// its claim meanwhile. Or it may answer at once and tell the sink
    /// [`Workspace::previews`] gave it when the preview is up.
    async fn propose(&self, run: &Run, proposal: &Proposal) -> Result<Value, HostError>;

    /// Hands over where to say a fix's preview is up, once, when the
    /// workspace joins a run's deps. A workspace that waits in
    /// [`Workspace::propose`] keeps nothing.
    fn previews(&self, _sink: Arc<dyn PreviewSink>) {}

    /// Ends whatever it holds for a run not in `active`, the runs that may
    /// still go on: a driver starting calls this, so a crash between a run's
    /// end and [`Workspace::finish`] leaks nothing.
    async fn reap(&self, _active: &[String]) {}

    /// Ends the run's container. Called however the run ended, and more
    /// than once is fine.
    async fn finish(&self, run: &Run);
}

/// What a command answers that was begun and can no longer be found.
#[must_use]
pub fn shell_interrupted() -> HostError {
    HostError::new(
        CODE_SHELL_INTERRUPTED,
        "the run was interrupted while this command ran: its state is unknown. It may have run \
         whole, in part or not at all, and the container may be a new one. Look before you run \
         it again.",
    )
}

/// `text` cut to [`MAX_OUTPUT_BYTES`], keeping its start and its end, where
/// a failure usually is.
#[must_use]
pub fn clip_output(text: &str) -> String {
    if text.len() <= MAX_OUTPUT_BYTES {
        return text.to_string();
    }
    let half = MAX_OUTPUT_BYTES / 2;
    let mut head = half;
    while !text.is_char_boundary(head) {
        head -= 1;
    }
    let mut tail = text.len() - half;
    while !text.is_char_boundary(tail) {
        tail += 1;
    }
    let cut = tail - head;
    format!(
        "{}\n… [{cut} bytes cut] …\n{}",
        &text[..head],
        &text[tail..]
    )
}

/// What a [`FakeWorkspace`] was asked.
#[derive(Debug, Default)]
pub struct FakeCalls {
    pub commands: Vec<String>,
    /// Each change pushed: a [`Proposal::key`] seen before pushes nothing.
    pub proposals: Vec<Proposal>,
    pub finished: Vec<String>,
}

/// A workspace that runs nothing, for tests. It offers
/// [`FakeWorkspace::offered`], [`Offer::CHANGE`] when that is `None`.
#[derive(Debug, Default)]
pub struct FakeWorkspace {
    pub calls: Mutex<FakeCalls>,
    pub offered: Option<Offer>,
    /// The commit a proposal answers, `abc` when `None`.
    pub commit: Option<String>,
    /// Its proposals answer with no preview, as one that does not wait.
    pub no_preview: bool,
}

impl FakeWorkspace {
    /// A fake offering `offer`.
    #[must_use]
    pub fn offering(offer: Offer) -> FakeWorkspace {
        FakeWorkspace {
            offered: Some(offer),
            ..FakeWorkspace::default()
        }
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
impl Workspace for FakeWorkspace {
    fn offer(&self, _run: &Run, _queue: Option<&str>) -> Offer {
        self.offered.clone().unwrap_or(Offer::CHANGE)
    }

    async fn exec(
        &self,
        _run: &Run,
        _queue: Option<&str>,
        command: &str,
        _cwd: Option<&str>,
        _timeout_secs: u64,
    ) -> Result<Value, HostError> {
        let mut c = self.calls.lock().expect("fake workspace");
        c.commands.push(command.to_string());
        Ok(json!({"exit_code": 0, "output": format!("ran {}", c.commands.len())}))
    }

    async fn propose(&self, _run: &Run, proposal: &Proposal) -> Result<Value, HostError> {
        let mut c = self.calls.lock().expect("fake workspace");
        if !c.proposals.iter().any(|p| p.key == proposal.key) {
            c.proposals.push(proposal.clone());
        }
        Ok(json!({
            "pr": 1,
            "url": "https://forge.example/pr/1",
            "commit": self.commit.as_deref().unwrap_or("abc"),
            "verify_url": (!self.no_preview).then_some("https://pr-1.preview.example"),
        }))
    }

    async fn finish(&self, run: &Run) {
        let mut c = self.calls.lock().expect("fake workspace");
        c.finished.push(run.id.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "1111111111111111111111111111111111111111";

    fn gated(offer: Offer) -> Offer {
        Offer {
            preview_gate: Some("post `/go {commit}` on the pull request".to_string()),
            ..offer
        }
    }

    fn proposal() -> Proposal {
        Proposal {
            title: "fix(x): y".to_string(),
            summary: "It works.".to_string(),
            key: "k".to_string(),
        }
    }

    fn note(offer: &Offer, done: &Value) -> String {
        offer.fix(&proposal(), done).members_note.unwrap()
    }

    #[test]
    fn a_fix_with_no_preview_carries_what_deploys_one_where_the_target_says() {
        let done = json!({"url": "https://forge.example/pr/1", "commit": SHA, "verify_url": null});
        let step = gated(Offer::CHANGE).fix(&proposal(), &done).preview_step;
        assert_eq!(
            step,
            Some(format!(
                "To deploy its preview, post `/go {SHA}` on the pull request."
            ))
        );
        assert_eq!(Offer::CHANGE.fix(&proposal(), &done).preview_step, None);
        let short = json!({"url": "https://forge.example/pr/1", "commit": "abc"});
        let step = gated(Offer::CHANGE).fix(&proposal(), &short).preview_step;
        assert_eq!(step, None, "no full commit, nothing to name");
    }

    #[test]
    fn a_fix_whose_preview_is_up_carries_no_step() {
        let done = json!({"commit": SHA, "verify_url": "https://pr-1.preview.example/"});
        for offer in [
            gated(Offer::CHANGE),
            gated(Offer {
                confidential: true,
                ..Offer::CHANGE
            }),
        ] {
            assert_eq!(offer.fix(&proposal(), &done).preview_step, None);
        }
    }

    #[test]
    fn a_held_fix_tells_the_queues_members_what_deploys_its_preview() {
        let done = json!({"url": "https://forge.example/pr/1", "commit": SHA, "verify_url": null});
        let step = format!("To deploy its preview, post `/go {SHA}` on the pull request.");
        for offer in [
            gated(Offer::CHANGE),
            gated(Offer::CHANGE.reviewed_previews()),
            gated(Offer::CHANGE.confidential()),
        ] {
            let fix = offer.fix(&proposal(), &done);
            assert!(fix.held);
            assert!(fix.members_note.unwrap().ends_with(&step));
        }
        // The reporter reads none of it.
        let fix = gated(Offer::CHANGE).fix(&proposal(), &done);
        assert_eq!(fix.summary, FIX_PROPOSED);
    }

    #[test]
    fn a_target_with_no_gate_or_a_fix_with_a_preview_names_no_step() {
        let done = json!({"commit": SHA, "verify_url": null});
        assert!(!note(&Offer::CHANGE, &done).contains("To deploy"));
        let up = json!({"commit": SHA, "verify_url": "https://pr-1.example"});
        assert!(!note(&gated(Offer::CHANGE).confidential(), &up).contains("To deploy"));
    }

    #[test]
    fn only_a_full_commit_makes_a_step() {
        for commit in [json!(null), json!("abc123"), json!(format!("{SHA}`@all"))] {
            let done = json!({"commit": commit, "verify_url": null});
            assert!(!note(&gated(Offer::CHANGE), &done).contains("To deploy"));
            let missed = gated(Offer::CHANGE).preview_missed(commit.as_str());
            assert_eq!(missed, PREVIEW_MISSED);
        }
    }

    #[test]
    fn a_gate_over_the_limit_makes_no_step() {
        let long = format!("{{commit}}{}", "x".repeat(MAX_PREVIEW_GATE_BYTES));
        let offer = Offer {
            preview_gate: Some(long),
            ..Offer::CHANGE
        };
        assert_eq!(offer.preview_missed(Some(SHA)), PREVIEW_MISSED);
    }

    #[test]
    fn a_missed_preview_says_what_a_person_does_where_the_target_says() {
        assert_eq!(Offer::CHANGE.preview_missed(Some(SHA)), PREVIEW_MISSED);
        let told = gated(Offer::CHANGE).preview_missed(Some(SHA));
        assert!(told.starts_with(PREVIEW_NOT_UP), "{told}");
        assert!(PREVIEW_MISSED.starts_with(PREVIEW_NOT_UP));
        assert!(
            told.contains(&format!(
                "To deploy its preview, post `/go {SHA}` on the pull request."
            )),
            "{told}"
        );
        // The wait is over: a preview that comes up now is a person's to hand on.
        assert!(
            told.contains("propose a fix of your own with its link"),
            "{told}"
        );
    }

    #[test]
    fn an_offer_kept_from_before_the_targets_words_reads_as_having_none() {
        let old = json!({"shell": true, "change": true});
        assert_eq!(serde_json::from_value::<Offer>(old).unwrap(), Offer::CHANGE);
        let told = Offer {
            guidance: Some("Say it plainly.".to_string()),
            ..gated(Offer::CHANGE)
        };
        let back = serde_json::to_value(&told).unwrap();
        assert_eq!(serde_json::from_value::<Offer>(back).unwrap(), told);
    }

    #[test]
    fn clipped_output_keeps_both_ends_on_character_boundaries() {
        let s = format!("start{}end", "é".repeat(MAX_OUTPUT_BYTES));
        let c = clip_output(&s);
        assert!(c.starts_with("start"));
        assert!(c.ends_with("end"));
        assert!(c.contains("bytes cut"));
        assert_eq!(clip_output("short"), "short");
    }
}
