//! What Clax's own surface does of a run's end
//! ([`crate::surface::SurfaceProvider::working`], `ending`, `ended`):
//! the roster on the run's threads, the notice a run that did not
//! finish leaves on them, what a run that finished posts there (its
//! summary, by the finish rule, and the pages it read), and the
//! follow-ups the threads' other people queued.
//!
//! # Inside the terminal step
//!
//! | The run | Posted on each of the run's threads | Journal place | Answered for the step's payload |
//! |---|---|---|---|
//! | Finished | The summary (the kind's result, `{summary}`), with the pages the run read listed after it ([`sources_markdown`]), **when no thread was replied on since a script last changed the draft or published** ([`Tally::replied`]) **and the summary is not empty**. When it is not posted and the run read pages, the list alone | The summary on thread `i` at notice place `i` ([`StepCx::notice`]); a list that stands alone at `1 + i` | `posted`: whether the rule held, whatever came of the posts. `sources`: each page read, `{title, url}` |
//! | Gave up | `I gave up: <reason> <what I did>` | `i` | `notice`: the text |
//! | Stopped | The notice of [`notice`], by the stop's reason (a failed model call on the instance's key: [`INSTANCE_PROVIDER_LEAD`], never the provider's words); **none for `cancelled` and `request_gone`** | `i` | `notice`: the text, or `null` for a stop that has none |
//! | Failed | `<lead> <what I did>` | `i` | `notice` |
//!
//! Each post is a reply planned by the host ([`BotHost::plan_reply`])
//! and run with the journal entry of its place ([`Claim::entry`]), in
//! the reply's transaction: an attempt of the terminal step made again
//! is answered from the entry and posts nothing more. A post that
//! cannot be made is logged and the hook answers all the same, with the
//! notice's text. The one failure the hook answers is the read of the
//! pages a finished run fetched (`Store::web_pages`): the run loop
//! counts it against the step, and a `finish` whose pages cannot be
//! read `max_commit_failures` times ends the run `failed`.
//!
//! What the run did ([`what_i_did`]) is folded from the committed steps
//! the hook is given ([`Tally::of`] over [`StepCx::history`]).
//!
//! # Outside it
//!
//! `working` shows ClaxBot as working on the run's threads and `ended`
//! stops showing it ([`BotHost::working`]); from the second the server
//! starts the follow-ups queued on the run's threads. Told twice of one
//! end, the host stops showing a run it no longer shows and finds no
//! follow-up left to start. The provider awaits the end of the last run
//! of each thread that has a follow-up queued and no run at work
//! ([`ClaxProvider::awaiting`]).
//!
//! [`BotHost::plan_reply`]: crate::host::BotHost::plan_reply
//! [`BotHost::working`]: crate::host::BotHost::working
//! [`Claim::entry`]: crate::surface::Claim::entry

use super::provider::ClaxProvider;
use super::tally::Tally;
use crate::host::{OpCtx, run_plan};
use crate::policy;
use crate::run::bot_ctx;
use crate::surface::{End, Fields, RunInfo, StepCx};
use crate::web::page::{sources as web_sources, sources_markdown};
use chrono::{DateTime, Utc};
use clax_bot_sandbox::HostError;
use clax_store::bot_runs::Run;
use clax_store::credits::{week_end, week_start};
use serde_json::{Value, json};

/// What a notice opens with, for the reason of a stop or the code of a
/// failure.
fn lead(reason: &str) -> &'static str {
    match reason {
        "no_flag" => "I stopped: ClaxBot is not turned on for you.",
        "disabled" => "I stopped: ClaxBot is turned off on this Clax.",
        "not_configured" => "I stopped: no model key is set up for ClaxBot.",
        "model_not_priced" => "I stopped: the model this run uses has no price set.",
        "key_rejected" => "I stopped: the model provider refused the key.",
        "model_error" => "I stopped: the model provider answered with an error.",
        "internal_error" => "I stopped: I could not save my work.",
        crate::run::HISTORY_UNREADABLE => "I stopped: this Clax could not read what I had done.",
        policy::POLICY_ERROR => "I stopped: this Clax could not work out what this run may do.",
        policy::MODEL_TIMEOUT => "I stopped: the model took longer to answer than this run allows.",
        r if r.starts_with("policy_history_") => {
            "I stopped: this Clax could not put together what to send the model."
        }
        _ => "I stopped before finishing.",
    }
}

/// What the run did, for the notice of a run that did not finish: the
/// scripts it ran, the draft paths it changed and the version it
/// published last.
fn what_i_did(tally: &Tally) -> String {
    let mut s = match tally.scripts {
        0 => "I had not run any scripts yet.".to_string(),
        1 => "I ran 1 script".to_string(),
        k => format!("I ran {k} scripts"),
    };
    if tally.scripts > 0 {
        if tally.changed.is_empty() {
            s.push('.');
        } else {
            let files: Vec<&str> = tally.changed.iter().map(String::as_str).collect();
            s.push_str(&format!(" and edited {}.", files.join(", ")));
        }
    }
    if let Some(v) = tally.published {
        s.push_str(&format!(" I published version {v}."));
    }
    if !tally.changed.is_empty() {
        s.push_str(" My draft is saved, unpublished: open it from the run panel.");
    }
    s
}

/// What a notice opens with for a run on the instance's key that stopped
/// because its model call failed (`key_rejected`, `model_error`): Clax's
/// own words, never the provider's nor what about the key failed, which
/// the provider failure log keeps for admins.
pub const INSTANCE_PROVIDER_LEAD: &str = "I stopped: I could not use this Clax's model \
     provider. This is a problem with how this Clax is set up, not with your request, and it \
     has been recorded for the people who run this Clax. Try again later.";

/// The notice a run that ends as `end` says leaves on its threads,
/// after what `tally` says it did; `None` for a run that finished (it
/// posts its summary) and for a stop that tells its threads nothing: a
/// cancel, and a request that is gone. `now` is when the notice is
/// made, for a stop that does not say when the person's credits reset.
/// `instance`: the run ran on the instance's key, so a failed model
/// call's stop says [`INSTANCE_PROVIDER_LEAD`].
pub(super) fn notice(
    end: &End,
    tally: &Tally,
    now: DateTime<Utc>,
    instance: bool,
) -> Option<String> {
    let did = what_i_did(tally);
    Some(match end {
        End::Finished => return None,
        End::GaveUp { reason, .. } => format!("I gave up: {reason} {did}"),
        End::Failed { code } => format!("{} {did}", lead(code)),
        End::Stopped {
            reason,
            limit,
            credits_reset,
        } => match (reason.as_str(), limit) {
            ("cancelled" | "request_gone", _) => return None,
            ("run_cap", Some(limit)) => {
                format!("I stopped before finishing: this run reached its limit of {limit}. {did}")
            }
            ("credits_exhausted", _) => {
                let reset = credits_reset
                    .unwrap_or_else(|| week_end(week_start(now)))
                    .format("on %A %-d %B %Y at %H:%M UTC");
                format!(
                    "I stopped before finishing: you have used this week's ClaxBot credits. {did} \
                     Your credits reset {reset}."
                )
            }
            ("key_rejected" | "model_error", _) if instance => {
                format!("{INSTANCE_PROVIDER_LEAD} {did}")
            }
            (reason, _) => format!("{} {did}", lead(reason)),
        },
    })
}

impl ClaxProvider {
    /// Shows ClaxBot as working on the run's threads.
    pub(super) async fn show_working(&self, run: &RunInfo) {
        match self.row(&run.id) {
            Ok(row) => self.host.working(&row, true).await,
            Err(e) => tracing::warn!("clax-bot: showing run {} at work: {e}", run.id),
        }
    }

    /// Posts `markdown` on each of the run's threads (a brief's run, on
    /// the brief's thread), the `i`th thread's post journaled at the
    /// binding's notice place `first + i`. A post that cannot be made
    /// is logged.
    async fn post(&self, cx: &StepCx<'_>, run: &Run, first: i64, markdown: &str) {
        let audit = bot_ctx(&run.person);
        for (i, t) in run.thread_ids.iter().enumerate() {
            let place = i64::try_from(i).ok().and_then(|i| cx.notice(first + i));
            let Some(place) = place else {
                tracing::warn!("clax-bot: run {} has no notice place for {t}", run.id);
                continue;
            };
            let op = OpCtx::new(run, &audit, place.step, place.attempt, place.seq);
            let res = match (
                cx.claim.entry(&place, "reply", &json!([t, markdown])),
                self.host.plan_reply(&op, t, markdown).await,
            ) {
                (Ok(entry), Ok(plan)) => run_plan(plan, entry).await.map(drop),
                (Err(e), _) | (_, Err(e)) => Err(e),
            };
            if let Err(e) = res {
                tracing::warn!("clax-bot: run {} could not reply on {t}: {e}", run.id);
            }
        }
    }

    /// The surface's part of the terminal step: see the module
    /// documentation.
    pub(super) async fn run_ending(
        &self,
        cx: StepCx<'_>,
        end: &End,
        result: Option<&Value>,
    ) -> clax_core::Result<Fields> {
        let run = self.row(&cx.run.id)?;
        let tally = Tally::of(cx.history);
        let mut fields = Fields::new();
        if *end != End::Finished {
            let instance = run.key_source == clax_store::credits::KeySource::Instance;
            let notice = notice(end, &tally, self.clock.now(), instance);
            if let Some(text) = &notice {
                self.post(&cx, &run, 0, text).await;
            }
            fields.insert("notice".into(), json!(notice));
            return Ok(fields);
        }
        let summary = result.and_then(|r| r["summary"].as_str()).unwrap_or("");
        let post = tally.replied.is_empty() && !summary.is_empty();
        let pages = self.store.web_pages(&run.id)?;
        match (sources_markdown(&pages), post) {
            (Some(list), true) => self.post(&cx, &run, 0, &format!("{summary}{list}")).await,
            (None, true) => self.post(&cx, &run, 0, summary).await,
            // The run replied itself: the list stands alone, after the
            // summary's place.
            (Some(list), false) => self.post(&cx, &run, 1, list.trim_start()).await,
            (None, false) => {}
        }
        fields.insert("posted".into(), json!(post));
        let sources: Vec<Value> = web_sources(&pages)
            .iter()
            .map(|s| json!({"title": s.title.as_deref().unwrap_or(&s.url), "url": s.url}))
            .collect();
        fields.insert("sources".into(), json!(sources));
        Ok(fields)
    }

    /// Stops showing ClaxBot as working on the run's threads, from
    /// which the host starts the follow-ups queued on them.
    pub(super) async fn run_ended(&self, run: &RunInfo) -> Result<(), HostError> {
        let row = self.row(&run.id).map_err(|e| crate::host::host_error(&e))?;
        self.host.working(&row, false).await;
        Ok(())
    }

    /// The runs whose end the surface still has something to do for:
    /// the last run of each thread that has a follow-up queued and no
    /// run at work, when that run has ended.
    pub(super) fn awaiting(&self) -> clax_core::Result<Vec<String>> {
        let mut runs: Vec<String> = Vec::new();
        for thread in self.store.waiting_follow_up_threads()? {
            if let Some(last) = self.store.runs_for_thread(&thread, 1)?.into_iter().next()
                && last.status.is_terminal()
                && !runs.contains(&last.id)
            {
                runs.push(last.id);
            }
        }
        Ok(runs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn at() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-11-11T10:00:00Z")
            .unwrap()
            .to_utc()
    }

    fn did(scripts: u32, changed: &[&str], published: Option<u32>) -> Tally {
        Tally {
            scripts,
            changed: changed
                .iter()
                .map(|s| (*s).to_string())
                .collect::<BTreeSet<_>>(),
            published,
            ..Tally::default()
        }
    }

    #[test]
    fn what_a_run_did_counts_its_scripts_and_names_its_edits_and_its_version() {
        let cases = [
            (did(0, &[], None), "I had not run any scripts yet."),
            (did(1, &[], None), "I ran 1 script."),
            (did(2, &[], None), "I ran 2 scripts."),
            (
                did(2, &["b.css", "a.html"], None),
                "I ran 2 scripts and edited a.html, b.css. My draft is saved, unpublished: open \
                 it from the run panel.",
            ),
            (
                did(1, &[], Some(3)),
                "I ran 1 script. I published version 3.",
            ),
            (
                did(3, &["a.html"], Some(2)),
                "I ran 3 scripts and edited a.html. I published version 2. My draft is saved, \
                 unpublished: open it from the run panel.",
            ),
        ];
        for (tally, text) in cases {
            assert_eq!(what_i_did(&tally), text);
        }
    }

    #[test]
    fn a_stop_has_the_notice_of_its_reason_and_two_reasons_have_none() {
        let none = Tally::default();
        let stopped = |reason: &str| notice(&End::stopped(reason), &none, at(), false);
        assert_eq!(stopped("cancelled"), None);
        assert_eq!(stopped("request_gone"), None);
        assert_eq!(notice(&End::Finished, &none, at(), false), None);
        let did = "I had not run any scripts yet.";
        for (reason, opens) in [
            ("no_flag", "I stopped: ClaxBot is not turned on for you."),
            ("disabled", "I stopped: ClaxBot is turned off on this Clax."),
            (
                "not_configured",
                "I stopped: no model key is set up for ClaxBot.",
            ),
            (
                "model_not_priced",
                "I stopped: the model this run uses has no price set.",
            ),
            (
                "key_rejected",
                "I stopped: the model provider refused the key.",
            ),
            (
                "model_error",
                "I stopped: the model provider answered with an error.",
            ),
            (
                "model_timeout",
                "I stopped: the model took longer to answer than this run allows.",
            ),
            ("run_cap", "I stopped before finishing."),
            ("anything_else", "I stopped before finishing."),
        ] {
            assert_eq!(stopped(reason), Some(format!("{opens} {did}")), "{reason}");
        }
        for (code, opens) in [
            ("internal_error", "I stopped: I could not save my work."),
            (
                "policy_error",
                "I stopped: this Clax could not work out what this run may do.",
            ),
            (
                "policy_history_3",
                "I stopped: this Clax could not put together what to send the model.",
            ),
        ] {
            let failed = End::Failed { code: code.into() };
            assert_eq!(
                notice(&failed, &none, at(), false),
                Some(format!("{opens} {did}")),
                "{code}"
            );
        }
        let gave_up = End::GaveUp {
            code: "model".into(),
            reason: "The page is gone.".into(),
        };
        assert_eq!(
            notice(&gave_up, &none, at(), false),
            Some(format!("I gave up: The page is gone. {did}"))
        );
    }

    #[test]
    fn a_failed_model_call_on_the_instances_key_is_told_in_claxs_words_alone() {
        let none = Tally::default();
        let did = "I had not run any scripts yet.";
        for reason in ["key_rejected", "model_error"] {
            let told = notice(&End::stopped(reason), &none, at(), true).unwrap();
            assert_eq!(told, format!("{INSTANCE_PROVIDER_LEAD} {did}"), "{reason}");
            assert!(!told.contains("key"), "{told}");
        }
        // Other stops read as they do on any key.
        assert_eq!(
            notice(&End::stopped("model_timeout"), &none, at(), true),
            notice(&End::stopped("model_timeout"), &none, at(), false)
        );
    }

    #[test]
    fn a_cap_says_its_limit_and_spent_credits_say_when_they_reset() {
        let none = Tally::default();
        let capped = End::Stopped {
            reason: "run_cap".into(),
            limit: Some("40 steps".into()),
            credits_reset: None,
        };
        assert_eq!(
            notice(&capped, &none, at(), false).unwrap(),
            "I stopped before finishing: this run reached its limit of 40 steps. I had not run \
             any scripts yet."
        );
        let spent = |credits_reset| End::Stopped {
            reason: "credits_exhausted".into(),
            limit: None,
            credits_reset,
        };
        let told = DateTime::parse_from_rfc3339("2026-12-07T00:00:00Z")
            .unwrap()
            .to_utc();
        assert_eq!(
            notice(&spent(Some(told)), &none, at(), false).unwrap(),
            "I stopped before finishing: you have used this week's ClaxBot credits. I had not \
             run any scripts yet. Your credits reset on Monday 7 December 2026 at 00:00 UTC."
        );
        // Not told when, the surface works it out from its own clock:
        // the Monday after 11 November 2026, a Wednesday.
        assert_eq!(
            notice(&spent(None), &none, at(), false).unwrap(),
            "I stopped before finishing: you have used this week's ClaxBot credits. I had not \
             run any scripts yet. Your credits reset on Monday 16 November 2026 at 00:00 UTC."
        );
    }
}
