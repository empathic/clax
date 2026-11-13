//! The end of a run: the run loop's part of the terminal step, which is
//! the same whatever surfaces the run has
//! ([`crate::surface`], "A run's end, through the two hooks").
//!
//! The run loop decides that a run ends and how ([`End`]) and says so
//! with an [`Ending`]: the end, the kind's result of a run that
//! finished, the run loop's own fields of the terminal step, and what
//! the step charges and records beside them. [`Tick::end`] then:
//!
//! 1. begins the terminal step;
//! 2. asks every binding's provider ([`SurfaceProvider::ending`]), the
//!    surface that declared the run's kind first and the others in the
//!    order they were bound, and joins the fields each answers to the
//!    step's payload ([`StepJoin`]);
//! 3. counts an `ending` that failed, or fields that clash, against
//!    the step as a commit that failed, and fails the tick: the step
//!    stays open and is begun again. In a `failed` step, which nothing
//!    follows, the failure is logged, that binding's fields are left
//!    out and the step commits;
//! 4. commits the step, which ends the run;
//! 5. tells every binding's provider that the run has ended
//!    ([`SurfaceProvider::ended`]), when the commit was made.
//!
//! A provider is told `ended` again when a driver starts
//! ([`redeliver`]), for each run it says it still awaits the end of
//! ([`SurfaceProvider::awaiting_ended`]): a tick that died between its
//! terminal commit and the call left the provider untold.

use crate::policy;
use crate::run::{Deps, Observed, Tick, TickOutcome, fenced, step_usage};
use crate::script::Surface;
use crate::surface::{Claim, End, Fields, RunInfo, StepCx, StepJoin, StepParts, SurfaceProvider};
use clax_core::audit::AuditRecord;
use clax_core::{CoreError, Result};
use clax_store::bot_pending::EndFor;
use clax_store::bot_runs::{Run, RunStatus, Step, StepCommit};
use clax_store::credits::Metered;
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// A run's end as the run loop decided it, and what its terminal step
/// holds, charges and records of the run loop's own.
pub(crate) struct Ending {
    pub end: End,
    /// The kind's result, of a run that finished: given to the surface
    /// that declared the run's kind.
    pub result: Option<Value>,
    /// The run loop's own fields of the terminal step's payload, each
    /// named in [`crate::surface::PAYLOAD_KEYS`].
    pub core: Fields,
    /// What the step charges beside its per-step lines.
    pub usage: Vec<Metered>,
    /// The step's audit records.
    pub events: Vec<AuditRecord>,
    /// The places of the step's calls that `events` reports: a stop
    /// records every other call the step's journal holds.
    pub reported: BTreeSet<i64>,
    /// How the model usage the step's attempts observed is written.
    pub observed: Observed,
    /// What the sessions of a cancelled script's step answered, which
    /// the bindings' `ending` fields join.
    pub join: StepJoin,
    /// The stop of a cancelled script, which is taken whatever a
    /// binding answers: the run was told to stop, and what the script
    /// used is charged. `Some` says whether a session's fields already
    /// clashed in this attempt.
    pub cancelled_script: Option<bool>,
}

impl Ending {
    fn new(end: End, result: Option<Value>, core: Fields) -> Ending {
        Ending {
            end,
            result,
            core,
            usage: Vec::new(),
            events: Vec::new(),
            reported: BTreeSet::new(),
            observed: Observed::default(),
            join: StepJoin::new(),
            cancelled_script: None,
        }
    }

    /// The model called `finish` with `summary`: the result of the
    /// run's kind is `{summary}`.
    pub(crate) fn finished(summary: &str) -> Ending {
        let mut core = Fields::new();
        core.insert("summary".into(), json!(summary));
        Ending::new(End::Finished, Some(json!({"summary": summary})), core)
    }

    /// The run gave up, for `reason`.
    pub(crate) fn gave_up(code: &str, reason: &str) -> Ending {
        let mut core = Fields::new();
        core.insert("code".into(), json!(code));
        core.insert("reason".into(), json!(reason));
        Ending::new(
            End::GaveUp {
                code: code.into(),
                reason: reason.into(),
            },
            None,
            core,
        )
    }

    /// The run is stopped as `end` says, an [`End::Stopped`].
    pub(crate) fn stopped(end: End) -> Ending {
        let mut core = Fields::new();
        core.insert("reason".into(), json!(end.reason()));
        Ending::new(end, None, core)
    }

    /// The run cannot go on, for `code`; `error` says how, when
    /// something does, cut to the bound of an error's text
    /// ([`policy::error_text`]).
    pub(crate) fn failed(code: &str, error: Option<&str>) -> Ending {
        let mut core = Fields::new();
        core.insert("reason".into(), json!(code));
        if let Some(e) = error {
            core.insert("error".into(), json!(policy::error_text(e)));
        }
        Ending::new(End::Failed { code: code.into() }, None, core)
    }

    /// This ending, its step's `error` saying `error`.
    pub(crate) fn with_error(mut self, error: Value) -> Ending {
        self.core.insert("error".into(), error);
        self
    }
}

/// The kind's result for the binding `surface` is of: `result` for the
/// surface that declared the run's kind, and nothing for any other.
fn result_for<'v>(
    info: &RunInfo,
    surface: &Surface<'_>,
    result: Option<&'v Value>,
) -> Option<&'v Value> {
    result.filter(|_| surface.binding.surface == info.kind_surface())
}

impl Tick<'_> {
    /// Tells each binding's provider that the run is being worked on:
    /// before a turn or a script.
    pub(crate) async fn working(&self) {
        let bound = &self.surfaces;
        for surface in bound.surfaces() {
            surface.provider.working(&bound.info).await;
        }
    }

    /// Ends the run as `ending` says: see the module documentation.
    ///
    /// # Errors
    /// The store's; a binding's `ending` that failed, and fields that
    /// clash, each counted against the step as a commit that failed
    /// (but in a `failed` step, which commits without them).
    pub(crate) async fn end(&self, ending: Ending) -> Result<TickOutcome> {
        let Ending {
            end,
            result,
            core,
            usage,
            events,
            reported,
            observed,
            mut join,
            cancelled_script,
        } = ending;
        let kind = end.step_kind();
        let s = self.begin(kind)?;
        let bound = &self.surfaces;
        let surfaces = bound.surfaces();
        let claim = Claim::new(self.store, self.driver, &self.run.id);
        // Every binding is asked, whatever another answered.
        let mut refused: Option<CoreError> = None;
        for surface in &surfaces {
            let name = &surface.binding.surface;
            let cx = StepCx::new(
                &bound.info,
                surface.binding,
                self.n,
                s.attempt,
                &self.steps,
                claim,
                &self.opts.cancel,
            );
            let asked = surface
                .provider
                .ending(cx, &end, result_for(&bound.info, surface, result.as_ref()))
                .await;
            let joined = asked.and_then(|fields| {
                let parts = StepParts {
                    payload: fields,
                    ..StepParts::default()
                };
                join.add(name, parts)
                    .map_err(|clash| CoreError::invalid("surface_field_clash", clash.to_string()))
            });
            if let Err(e) = joined {
                tracing::warn!(
                    "clax-bot: the ending of run {} for its surface {name}, at step {}: {e}",
                    self.run.id,
                    self.n
                );
                refused.get_or_insert(e);
            }
        }
        // Nothing follows a `failed` step: it commits without the
        // fields of a binding that failed. A cancelled script's stop
        // does too, the failure counted against the step, once for the
        // attempt with its sessions'.
        let last = matches!(end, End::Failed { .. });
        match (refused, cancelled_script) {
            (Some(e), None) if !last => {
                if !fenced(&e) {
                    let _ = self.store.note_commit_failure(&self.run.id, self.n);
                }
                return Err(e);
            }
            (Some(_), Some(_)) | (None, Some(true)) => {
                let _ = self.store.note_commit_failure(&self.run.id, self.n);
            }
            _ => {}
        }
        let payload = join.payload(core);
        let (mut usage, mut events) = (usage, events);
        if !last {
            usage.extend(step_usage(
                &self.settings,
                self.n,
                0,
                &payload,
                self.key_source(),
            ));
        }
        if matches!(end, End::Stopped { .. }) {
            events.extend(self.unreported_calls(&reported));
        }
        // How the run ends for each registration a binding's provider
        // names, kept with the commit.
        let ends: Vec<EndFor> = surfaces
            .iter()
            .filter_map(|surface| {
                let result = result_for(&bound.info, surface, result.as_ref());
                surface.provider.end_for(&bound.info, &end, result)
            })
            .collect();
        let commit = || StepCommit {
            payload: payload.clone(),
            usage: usage.clone(),
            events: events.clone(),
            draft: Vec::new(),
            reason: end.reason().map(str::to_string),
        };
        let c = match self.commit_ending(s.attempt, kind, commit(), observed, &ends) {
            // A `failed` step absorbs the model calls its attempts
            // made; when even that cannot be written, the run ends
            // without them.
            Err(e) if last && !fenced(&e) => self.commit_ending(
                s.attempt,
                kind,
                commit(),
                Observed {
                    none: true,
                    ..Observed::default()
                },
                &ends,
            )?,
            other => other?,
        };
        if c.is_ok() {
            for surface in &surfaces {
                let result = result_for(&bound.info, surface, result.as_ref());
                told_ended(&bound.info, surface.provider, &end, result).await;
            }
        }
        Ok(self.outcome(kind, c))
    }
}

/// Tells `provider` that `run` ended as `end` says; what it answers is
/// logged, since the run has ended whatever it is.
async fn told_ended(
    run: &RunInfo,
    provider: &dyn SurfaceProvider,
    end: &End,
    result: Option<&Value>,
) {
    if let Err(e) = provider.ended(run, end, result).await {
        tracing::warn!("clax-bot: telling a surface that run {} ended: {e}", run.id);
    }
}

/// The end of `run`, which has ended, as the store keeps it, and the
/// kind's result when the run finished: what a provider is told when
/// `ended` is called again after a restart
/// ([`SurfaceProvider::ended`], "What a repeat carries"). `last` is the
/// run's terminal step. `None` for a run that has not ended.
///
/// The kind of the end, its code and its reason are the run's row's and
/// the step's; a stop's `limit` and `credits_reset` are kept nowhere
/// and are `None`. The kind's result is the step's `result`, which a
/// kind of a surface other than `clax` finishes with
/// ([`crate::finish`]), else `{summary}`.
pub(crate) fn stored_end(run: &Run, last: Option<&Step>) -> Option<(End, Option<Value>)> {
    let payload = last.and_then(|s| s.payload.as_ref());
    let text = |key: &str| {
        payload
            .and_then(|p| p[key].as_str())
            .unwrap_or_default()
            .to_string()
    };
    let reason = run.reason.clone().unwrap_or_default();
    Some(match run.status {
        RunStatus::Finished => {
            let result = payload.and_then(|p| p.get("result")).cloned();
            let result = result.unwrap_or_else(|| json!({"summary": text("summary")}));
            (End::Finished, Some(result))
        }
        RunStatus::GaveUp => (
            End::GaveUp {
                code: reason,
                reason: text("reason"),
            },
            None,
        ),
        RunStatus::Stopped => (End::stopped(reason), None),
        RunStatus::Failed => (End::Failed { code: reason }, None),
        _ => return None,
    })
}

/// Tells each provider `ended` again for the runs it still awaits the
/// end of ([`SurfaceProvider::awaiting_ended`]) and that have ended:
/// what a driver does when it starts, so that an end whose tick died
/// between its terminal commit and the call is not lost. The end is
/// rebuilt from the store ([`stored_end`]), and the run from its row
/// ([`crate::registry::Providers::bind`]). A provider that fails, or a
/// run that cannot be read, is logged and does not keep the others from
/// being told. Returns how many calls were made.
pub(crate) async fn redeliver(deps: &Deps) -> usize {
    let mut told = 0;
    for (surface, provider) in deps.providers.iter() {
        let awaiting = match provider.awaiting_ended().await {
            Ok(ids) => ids,
            Err(e) => {
                tracing::warn!("clax-bot: asking a surface for the runs whose end it awaits: {e}");
                continue;
            }
        };
        for id in awaiting {
            let ended = deps.store.run_with_steps_as_made(&id).and_then(|found| {
                let Some((run, made, steps)) = found else {
                    return Ok(None);
                };
                let Some((end, result)) = stored_end(&run, steps.last()) else {
                    return Ok(None);
                };
                Ok(Some((deps.providers.bind(&run, &made)?.info, end, result)))
            });
            match ended {
                Ok(Some((info, end, result))) => {
                    let result = result.filter(|_| info.kind_surface() == surface);
                    told_ended(&info, &**provider, &end, result.as_ref()).await;
                    told += 1;
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!("clax-bot: reading run {id}, whose end a surface awaits: {e}")
                }
            }
        }
    }
    told
}

#[cfg(test)]
mod tests {
    /// The run loop's part of a run's end reads nothing of Clax's own
    /// surface: no module of its provider, neither the host nor the web
    /// source of the tick's deps, and none of its nouns. This holds
    /// only what the file names; that a run ends with nothing of Clax's
    /// run is held by the runs of `tests/generic.rs`, whose one surface
    /// is the fake's.
    #[test]
    fn the_run_loops_ending_names_nothing_of_claxs_surface() {
        let source = include_str!("ending.rs");
        let code = source
            .split("#[cfg(test)]\nmod tests")
            .next()
            .expect("the code before the tests");
        let code: String = code
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        for named in [
            "crate::clax",
            "crate::web",
            "deps.host",
            "deps.web",
            "thread",
            "artifact",
            "notice",
            "roster",
            "sources",
            "posted",
        ] {
            assert!(!code.contains(named), "ending.rs names `{named}`");
        }
    }
}
